//! iOS 向け音声セッション実装（Issue #841、親 #172）。`device/audio_session.rs` の iOS 経路
//! （`platform_session`）から呼ばれ、`ios/fandhe_ios_audio_session.m`（`AVAudioSession`。`build.rs` が
//! iOS 分岐で feature `device` 有効時にコンパイルする）を `unsafe extern "C"` で呼ぶ。`unsafe` は
//! その呼び出しと受け口に閉じ `// SAFETY:` を付ける。契約は `fandhe_device::audio_session` のモジュール doc
//! （方式・結果の種類・解放・守る範囲 / 守らない範囲）で、本ファイルは重複して書かない。
//!
//! # 流れ
//!
//! `start` が id を採番し、**`.m` を呼ぶ前に**購読の登録表（[`SLOT`]）へ `OnSignal` を入れる
//! （`setActive` の中で同期に届く通知も落とさない）。`.m` が category と有効化を行い、通知を
//! [`fandhe_rust_audio_session_signal`] で届ける。受け口は id を照合して配送し、直後に
//! `crate::ios::request_repaint` で次フレームを起こす。停止は登録解除 → `.m` の購読解除・無効化・復元の順。
//! `.m` の呼び出し中は [`SLOT`] のロックを持たない（main で同期に届く通知が同じロックを取るため）。
//!
//! 写像と読み戻しの判定は純関数で、ホストでもテストできるようモジュールは `cfg(any(ios, test))`。
//!
//! # 既知の天井
//!
//! - シミュレータでは割り込み・経路変更をほぼ起こせない。受信の実機確認は #838
//! - 停止時に他機能が音声オブジェクトを動かしていると `setActive(NO)` が `isBusy` を返し、
//!   `RestoreFailed` として見える（OS の結果をそのまま写す）
//! - category が Playback 以外のときの読み戻しは `Other`（既定の soloAmbient を含む）。推測で 3 種に寄せない

use fandhe_device::audio_session::{CurrentState, ReadKind, StartFailure, Support};
use fandhe_device::DeviceError;

/// `AVAudioSessionCategoryOptionMixWithOthers`（SDK の `AVAudioSessionTypes.h`）。
const OPT_MIX_WITH_OTHERS: u64 = 0x1;
/// `AVAudioSessionCategoryOptionDuckOthers`（同上）。
const OPT_DUCK_OTHERS: u64 = 0x2;
/// `.m` の category コード: playback。
const CATEGORY_PLAYBACK: i32 = 2;

/// iOS の対応範囲。種別 3 つ・割り込みの開始 / 終了・経路変更を受け、category と他アプリの再生中を読める。
fn ios_support() -> Support {
    let mut events = [false; 8];
    events[0] = true; // InterruptionBegan
    events[1] = true; // InterruptionEnded
    events[2] = true; // RouteChanged
    Support {
        kinds: [true; 3],
        events,
        kind_readable: true,
        other_playing_readable: true,
        audio_mode_readable: false,
        music_active_readable: false,
    }
}

/// category と options の生の値を読み戻し種別へ写す。Playback 以外は本機能が設定した値ではないので `Other`。
fn read_kind_from_raw(category: i32, options: u64) -> ReadKind {
    if category != CATEGORY_PLAYBACK {
        ReadKind::Other
    } else if options & OPT_DUCK_OTHERS != 0 {
        ReadKind::DuckOthers
    } else if options & OPT_MIX_WITH_OTHERS != 0 {
        ReadKind::MixWithOthers
    } else {
        ReadKind::StopOthers
    }
}

/// `.m` の start の戻り値を結果へ写す。未知のコードを成功にしない。
fn start_result(code: i32) -> Result<(), StartFailure> {
    match code {
        0 => Ok(()),
        5 => Err(StartFailure::KindUnsupported),
        3 => Err(StartFailure::Failed { restored: false }),
        _ => Err(StartFailure::Failed { restored: true }),
    }
}

/// `.m` の stop の戻り値を結果へ写す。0（無効化と復元の両方が成功）だけを成功にする。
fn stop_result(code: i32) -> Result<(), fandhe_device::audio_session::RestoreFailed> {
    if code == 0 {
        Ok(())
    } else {
        Err(fandhe_device::audio_session::RestoreFailed)
    }
}

/// `.m` の read の結果を現在の状態へ写す。失敗に OS のコードやメッセージは載せない。
fn read_result(
    code: i32,
    category: i32,
    options: u64,
    other_playing: i32,
) -> Result<CurrentState, DeviceError> {
    if code != 0 {
        return Err(DeviceError::Internal("audio_session read"));
    }
    Ok(CurrentState {
        kind: Some(read_kind_from_raw(category, options)),
        other_audio_playing: Some(other_playing != 0),
        audio_mode: None,
        music_active: None,
    })
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use fandhe_device::audio_session::{
        AudioSession, AudioSessionGuard, OnSignal, RestoreFailed, SessionKind, StartFailure,
        Support,
    };
    use fandhe_device::DeviceError;

    use super::{ios_support, read_result, start_result, stop_result, CurrentState};

    extern "C" {
        fn fandhe_ios_audio_session_start(id: u64, kind: i32) -> i32;
        fn fandhe_ios_audio_session_stop(id: u64) -> i32;
        fn fandhe_ios_audio_session_read(
            category: *mut i32,
            options: *mut u64,
            other_playing: *mut i32,
        ) -> i32;
    }

    /// 購読の登録表。1 プロセスで画面は 1 つ、という守る範囲に合わせて 1 枠。
    static SLOT: Mutex<Option<(u64, OnSignal)>> = Mutex::new(None);
    /// 1 から採番する（0 は `.m` で「なし」）。
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    /// 枠が `id` のときだけ消す（後から入った別の購読を消さない）。
    fn clear_slot(id: u64) {
        let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|(i, _)| *i == id) {
            *slot = None;
        }
    }

    /// `.m` の通知を届ける受け口。任意のスレッドから呼ばれ、panic しない。解除後に遅れて届いた
    /// 通知（id 不一致）・未知のコードは捨てる。ロックの外で配送する。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_audio_session_signal(id: u64, code: i32) {
        let cb = {
            let slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_ref() {
                Some((i, cb)) if *i == id => Arc::clone(cb),
                _ => return,
            }
        };
        if let Some(signal) = fandhe_device::audio_session::signal_from_ios_code(code) {
            cb(signal);
            crate::ios::request_repaint();
        }
    }

    struct IosAudioSession;

    pub(crate) fn implementation() -> Arc<dyn AudioSession> {
        Arc::new(IosAudioSession)
    }

    impl AudioSession for IosAudioSession {
        fn support(&self) -> Support {
            ios_support()
        }

        fn start(
            &self,
            kind: SessionKind,
            on_signal: OnSignal,
        ) -> Result<Box<dyn AudioSessionGuard>, StartFailure> {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            *SLOT.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, on_signal));
            // SAFETY: 整数を渡すだけ。`.m` は Rust のポインタを保持しない。ロックは持たずに呼ぶ。
            #[allow(unsafe_code)]
            let code = unsafe { fandhe_ios_audio_session_start(id, kind.code()) };
            match start_result(code) {
                Ok(()) => Ok(Box::new(IosAudioSessionGuard { id, done: false })),
                Err(e) => {
                    clear_slot(id);
                    Err(e)
                }
            }
        }

        fn read_current(&self) -> Result<CurrentState, DeviceError> {
            let (mut category, mut options, mut other) = (-1_i32, 0_u64, 0_i32);
            // SAFETY: 3 つの出力先はこのスタック上の有効な変数で、呼び出しの間だけ使われる。
            #[allow(unsafe_code)]
            let code =
                unsafe { fandhe_ios_audio_session_read(&mut category, &mut options, &mut other) };
            read_result(code, category, options, other)
        }
    }

    struct IosAudioSessionGuard {
        id: u64,
        done: bool,
    }

    impl IosAudioSessionGuard {
        /// 登録解除 → `.m` の購読解除・無効化・復元。一度だけ行う。
        fn teardown(&mut self) -> Result<(), RestoreFailed> {
            self.done = true;
            clear_slot(self.id);
            // SAFETY: 整数を渡すだけ。
            #[allow(unsafe_code)]
            let code = unsafe { fandhe_ios_audio_session_stop(self.id) };
            stop_result(code)
        }
    }

    impl AudioSessionGuard for IosAudioSessionGuard {
        fn stop(mut self: Box<Self>) -> Result<(), RestoreFailed> {
            self.teardown()
        }
    }

    impl Drop for IosAudioSessionGuard {
        fn drop(&mut self) {
            if !self.done {
                let _ = self.teardown();
            }
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_is_ios_shape() {
        let s = ios_support();
        assert_eq!(s.kinds, [true; 3]);
        assert_eq!(
            s.events,
            [true, true, true, false, false, false, false, false]
        );
        assert!(s.kind_readable && s.other_playing_readable);
        assert!(!s.audio_mode_readable && !s.music_active_readable);
    }

    #[test]
    fn read_kind_maps_playback_options_only() {
        assert_eq!(read_kind_from_raw(2, 0), ReadKind::StopOthers);
        assert_eq!(read_kind_from_raw(2, OPT_DUCK_OTHERS), ReadKind::DuckOthers);
        assert_eq!(
            read_kind_from_raw(2, OPT_MIX_WITH_OTHERS),
            ReadKind::MixWithOthers
        );
        // soloAmbient 既定・record 系・未知は 3 種へ寄せない。
        for c in [0, 1, 3, 4, 5, -1] {
            assert_eq!(read_kind_from_raw(c, OPT_DUCK_OTHERS), ReadKind::Other);
        }
    }

    #[test]
    fn start_result_never_folds_failures_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(5), Err(StartFailure::KindUnsupported));
        assert_eq!(
            start_result(3),
            Err(StartFailure::Failed { restored: false })
        );
        for c in [1, 2, 4, 99, -1] {
            assert_eq!(
                start_result(c),
                Err(StartFailure::Failed { restored: true })
            );
        }
    }

    #[test]
    fn stop_result_only_zero_is_ok() {
        assert!(stop_result(0).is_ok());
        assert!(stop_result(1).is_err());
        assert!(stop_result(2).is_err());
    }

    #[test]
    fn read_result_maps_and_fails_closed() {
        let s = read_result(0, 2, OPT_DUCK_OTHERS, 1).unwrap();
        assert_eq!(s.kind, Some(ReadKind::DuckOthers));
        assert_eq!(s.other_audio_playing, Some(true));
        assert_eq!(s.audio_mode, None);
        assert_eq!(s.music_active, None);
        assert!(read_result(1, 2, 0, 0).is_err());
    }
}
