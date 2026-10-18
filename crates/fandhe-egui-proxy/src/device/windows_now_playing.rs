//! Windows 向けロック画面・OS のメディア操作実装（Issue #385、親 #98）。
//! `SystemMediaTransportControls`（SMTC）へ曲名と再生状態を渡し、メディアキー・SMTC フライアウトの
//! 再生・一時停止を `device::now_playing::commands()` へ積む。
//!
//! - 呼び出し元: `device/now_playing.rs::platform_now_playing` の Windows 経路（effect ドライバが UI スレッドから
//!   `publish` / `clear` を呼ぶ）。契約の SSOT は `fandhe_device::now_playing`。
//! - スレッド: `ButtonPressed` は UI 以外のスレッドで届く。handler は `commands().push` だけを行い、
//!   画面の状態や `held` のロックには触れない（契約表 11）。SMTC は `Send + Sync` なので Mutex 1 本で足りる。
//! - `unsafe` は `GetForWindow` の 1 箇所に閉じる。HRESULT・曲名はログにも画面にも出さず、
//!   エラーは固定ラベル `now_playing` に畳む（A09）。
//! - 割り切り: 再生・一時停止だけを有効にする（Stop・前後の曲・シーク・アートワーク・タイムラインは範囲外）。
//!   メディアキーは OS が `PlaybackStatus` を見て Play / Pause に振り分けるため `Toggle` は来ない。
//!   `RoInitialize` は呼ばず、winit の UI スレッドが初期化済みである前提（他の Windows 実装と同じ）。
//!   守らない範囲: 約 250ms のポーリングぶんの遅れ、最小化中の遅れ、複数プロセス・複数窓、
//!   unpackaged で出るアプリ名・ロック画面への表示（実機確認 #378）。

use std::sync::Arc;

use fandhe_device::now_playing::{NowPlaying, OsMediaCommand};

/// `SystemMediaTransportControlsButton` の列挙値を契約上の操作へ写す。
/// 範囲外（Stop・Next 等）は `None` で捨て、成功として扱わない。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn command_from_button(code: i32) -> Option<OsMediaCommand> {
    match code {
        0 => Some(OsMediaCommand::Play),
        1 => Some(OsMediaCommand::Pause),
        _ => None,
    }
}

/// 現在の OS 向け実装を返す。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Arc<dyn NowPlaying> {
    Arc::new(imp::WindowsNowPlaying::default())
}

/// 非 Windows（ホストの test ビルド）では非対応を返す。
#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn NowPlaying> {
    Arc::new(fandhe_device::now_playing::UnsupportedNowPlaying)
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::{Mutex, MutexGuard};

    use windows::core::HSTRING;
    use windows::Foundation::TypedEventHandler;
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButtonPressedEventArgs,
    };
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;

    use fandhe_device::now_playing::{NowPlaying, NowPlayingInfo, DEMO_TITLE};
    use fandhe_device::DeviceError;

    use super::command_from_button;

    const ERR: DeviceError = DeviceError::Internal("now_playing");

    /// 取得済みの SMTC と `ButtonPressed` の登録トークン。
    struct Held {
        smtc: SystemMediaTransportControls,
        token: i64,
    }

    /// 登録を解放する（失敗は無視。契約表 9）。
    fn release(held: Held) {
        let _ = held.smtc.RemoveButtonPressed(held.token);
        if let Ok(u) = held.smtc.DisplayUpdater() {
            let _ = u.ClearAll();
            let _ = u.Update();
        }
        let _ = held.smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed);
        let _ = held.smtc.SetIsEnabled(false);
    }

    #[derive(Default)]
    pub(super) struct WindowsNowPlaying {
        held: Mutex<Option<Held>>,
    }

    impl WindowsNowPlaying {
        fn lock(&self) -> MutexGuard<'_, Option<Held>> {
            self.held.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// SMTC を取得して handler と有効化を済ませる。途中で失敗したら確保済みの分を解放して返す。
        #[allow(unsafe_code)]
        fn acquire() -> Result<Held, DeviceError> {
            let hwnd = super::super::windows_share::window().ok_or(ERR)?;
            let interop = windows::core::factory::<
                SystemMediaTransportControls,
                ISystemMediaTransportControlsInterop,
            >()
            .map_err(|_| ERR)?;
            // SAFETY: `hwnd` は `remember_window` が構築時に控えた winit の窓で、`ProxyApp` の生存中は
            // 窓も生存する（windows_share.rs::present と同じ根拠）。`interop` は上で取得した有効な COM ポインタ。
            let smtc: SystemMediaTransportControls =
                unsafe { interop.GetForWindow(hwnd) }.map_err(|_| ERR)?;
            let token = smtc
                .ButtonPressed(&TypedEventHandler::<
                    SystemMediaTransportControls,
                    SystemMediaTransportControlsButtonPressedEventArgs,
                >::new(|_, args| {
                    // 画面の状態には触れず積むだけ。拒否（false）を返す口は SMTC に無いので捨てる。
                    if let Some(cmd) = args
                        .as_ref()
                        .and_then(|a| a.Button().ok())
                        .and_then(|b| command_from_button(b.0))
                    {
                        super::super::now_playing::commands().push(cmd);
                    }
                    Ok(())
                }))
                .map_err(|_| ERR)?;
            let held = Held { smtc, token };
            let enabled = held
                .smtc
                .SetIsEnabled(true)
                .and_then(|_| held.smtc.SetIsPlayEnabled(true))
                .and_then(|_| held.smtc.SetIsPauseEnabled(true));
            if enabled.is_err() {
                release(held);
                return Err(ERR);
            }
            Ok(held)
        }

        fn update(smtc: &SystemMediaTransportControls, playing: bool) -> windows::core::Result<()> {
            smtc.SetPlaybackStatus(if playing {
                MediaPlaybackStatus::Playing
            } else {
                MediaPlaybackStatus::Paused
            })?;
            let u = smtc.DisplayUpdater()?;
            u.SetType(MediaPlaybackType::Music)?;
            u.MusicProperties()?.SetTitle(&HSTRING::from(DEMO_TITLE))?;
            u.Update()
        }
    }

    impl NowPlaying for WindowsNowPlaying {
        fn publish(&self, info: NowPlayingInfo) -> Result<(), DeviceError> {
            let mut guard = self.lock();
            if guard.is_none() {
                *guard = Some(Self::acquire()?);
            }
            // ponytail: 位置・長さは渡さない。必要なら SystemMediaTransportControlsTimelineProperties を足す。
            let ok = guard
                .as_ref()
                .is_some_and(|h| Self::update(&h.smtc, info.playing).is_ok());
            if !ok {
                if let Some(h) = guard.take() {
                    release(h);
                }
                return Err(ERR);
            }
            Ok(())
        }

        fn clear(&self) {
            if let Some(h) = self.lock().take() {
                release(h);
            }
        }

        fn shutdown(&self) {
            self.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_play_and_pause_are_mapped() {
        assert_eq!(command_from_button(0), Some(OsMediaCommand::Play));
        assert_eq!(command_from_button(1), Some(OsMediaCommand::Pause));
        for c in [2, 3, 4, 9, -1, i32::MAX] {
            assert_eq!(command_from_button(c), None);
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn host_build_is_unsupported() {
        use fandhe_device::now_playing::NowPlayingInfo;
        assert!(implementation()
            .publish(NowPlayingInfo::new(true, None, None))
            .is_err());
    }
}
