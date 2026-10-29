//! Phase 11（親 #188）型サンプル「壁紙・ロック画面の設定」（Issue #970）の effect ドライバ。
//! `fandhe_core::screens::wallpaper_set`（端末依存ゼロ）の要求を観測し、アプリが生成した PNG
//! （`photo_save::sample_png`）を `fandhe_device::photo_library::PngImage` で検査してから OS へ渡し、
//! 結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `device/photo_save.rs` と同型）。契約と守る範囲は `fandhe_device::wallpaper` のモジュール doc。
//!
//! # dispatch（最終形）
//!
//! 後続の #971（Android）は本ファイルを触らず、`android_wallpaper::begin_set` だけを埋める。
//!
//! | OS | 経路 |
//! |---|---|
//! | macOS | 同期（egui の update は main スレッド）。[`super::macos_wallpaper`] |
//! | Windows | 同期（UI スレッドで `SystemParametersInfoW` を 1 回）。[`super::windows_wallpaper`] |
//! | Android | コールバック型。pull 型 [`bridge`] + [`super::android_wallpaper::begin_set`] |
//! | iOS・Linux | `UnsupportedWallpaper`（「この端末では使えません」） |

use fandhe_core::screens::wallpaper_set::{
    finish_set, mark_setting, take_request, SetOutcome, WallpaperChoice, WallpaperSetState,
};
use fandhe_device::wallpaper::WallpaperTarget;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。`Cancelled` は UI が無く届かないが、万一届いても成功にはしない。
fn to_outcome(result: Result<WallpaperTarget, DeviceError>) -> Result<SetOutcome, String> {
    match result {
        Ok(t) => Ok(SetOutcome::Set(to_choice(t))),
        Err(DeviceError::PermissionDenied(_)) => Ok(SetOutcome::Denied),
        Err(DeviceError::Unsupported(_)) => Ok(SetOutcome::Unsupported),
        Err(DeviceError::InvalidInput(_)) => Err("画像が不正です".to_string()),
        Err(_) => Err("OS の壁紙設定に失敗しました".to_string()),
    }
}

fn to_target(choice: WallpaperChoice) -> WallpaperTarget {
    match choice {
        WallpaperChoice::Home => WallpaperTarget::Home,
        WallpaperChoice::Lock => WallpaperTarget::Lock,
    }
}

fn to_choice(target: WallpaperTarget) -> WallpaperChoice {
    match target {
        WallpaperTarget::Home => WallpaperChoice::Home,
        WallpaperTarget::Lock => WallpaperChoice::Lock,
    }
}

/// OS 側コールバックとの橋渡し（Android、pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) mod bridge {
    use fandhe_core::screens::wallpaper_set::SetOutcome;
    use fandhe_device::wallpaper::WallpaperTarget;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<SetOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(request_id: u64, result: Result<WallpaperTarget, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる（`photo_save::bridge::reset` と同じ）。
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

/// Android 版の effect ドライバ本体。
#[cfg(target_os = "android")]
fn drive_callback(state: &WallpaperSetState, sender: &CompletionSender<WallpaperSetState>) {
    use fandhe_core::screens::wallpaper_set::SetPhase;
    use fandhe_device::photo_library::PngImage;

    if let Some(req) = take_request(state) {
        let _ = sender.send(Box::new(mark_setting));
        let request_id = bridge::begin();
        // 検査を通らない画像は OS へ渡さずその場で画面へ返す。
        let accepted = PngImage::new(super::photo_save::sample_png()).and_then(|png| {
            super::android_wallpaper::begin_set(request_id, &png, to_target(req.choice))
        });
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != SetPhase::Setting {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut WallpaperSetState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != SetPhase::Idle {
                finish_set(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から配線される effect ドライバ（Android）。
#[cfg(target_os = "android")]
pub fn wallpaper_set_effect(
    state: &WallpaperSetState,
    sender: &CompletionSender<WallpaperSetState>,
) {
    drive_callback(state, sender);
}

/// 同期経路（macOS・Windows・iOS・Linux）: 検査してから [`Wallpaper`] を呼ぶ。
///
/// [`Wallpaper`]: fandhe_device::wallpaper::Wallpaper
#[cfg(not(target_os = "android"))]
fn drive(
    state: &WallpaperSetState,
    sender: &CompletionSender<WallpaperSetState>,
    wp: &dyn fandhe_device::wallpaper::Wallpaper,
) {
    use fandhe_device::photo_library::PngImage;

    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_setting));
    let result = PngImage::new(super::photo_save::sample_png())
        .and_then(|png| wp.set_wallpaper(&png, to_target(request.choice)));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut WallpaperSetState| {
        finish_set(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ（macOS・Windows・iOS・Linux）。
///
/// ponytail: Windows は `SPIF_SENDCHANGE` のブロードキャストで UI スレッドが数百 ms 止まりうる。
/// 目立つなら作業スレッドと bridge へ移す。
#[cfg(not(target_os = "android"))]
pub fn wallpaper_set_effect(
    state: &WallpaperSetState,
    sender: &CompletionSender<WallpaperSetState>,
) {
    #[cfg(target_os = "macos")]
    drive(state, sender, &super::macos_wallpaper::MacosWallpaper);
    #[cfg(target_os = "windows")]
    drive(state, sender, &super::windows_wallpaper::WindowsWallpaper);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    drive(
        state,
        sender,
        &fandhe_device::wallpaper::UnsupportedWallpaper,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::photo_library::PngImage;

    #[test]
    fn to_outcome_covers_all_paths() {
        assert_eq!(
            to_outcome(Ok(WallpaperTarget::Home)),
            Ok(SetOutcome::Set(WallpaperChoice::Home))
        );
        assert_eq!(
            to_outcome(Ok(WallpaperTarget::Lock)),
            Ok(SetOutcome::Set(WallpaperChoice::Lock))
        );
        assert_eq!(
            to_outcome(Err(DeviceError::PermissionDenied("wallpaper"))),
            Ok(SetOutcome::Denied)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("wallpaper"))),
            Ok(SetOutcome::Unsupported)
        );
        for e in [
            DeviceError::InvalidInput("photo_library png"),
            DeviceError::Internal("wallpaper set"),
            DeviceError::Cancelled("wallpaper"),
        ] {
            assert!(to_outcome(Err(e)).is_err());
        }
    }

    #[test]
    fn choice_target_roundtrip() {
        for c in [WallpaperChoice::Home, WallpaperChoice::Lock] {
            assert_eq!(to_choice(to_target(c)), c);
        }
    }

    #[test]
    fn sample_png_passes_validation() {
        assert!(PngImage::new(super::super::photo_save::sample_png()).is_ok());
    }

    #[test]
    fn unsupported_is_shown_and_never_set() {
        use fandhe_core::screens::wallpaper_set::{set_home, SetPhase};
        let mut s = WallpaperSetState::default();
        set_home(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive(&s, &sender, &fandhe_device::wallpaper::UnsupportedWallpaper);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        assert_eq!(s.phase, SetPhase::Idle);
        assert!(!s.status.contains("設定しました"));
        assert!(!s.status.is_empty());
    }

    #[test]
    fn bridge_drops_stale_id_and_second_deliver() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id + 1000, Ok(WallpaperTarget::Home)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(
            id,
            Err(DeviceError::PermissionDenied("wallpaper"))
        ));
        assert!(!bridge::deliver(id, Ok(WallpaperTarget::Home)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(SetOutcome::Denied)));
        bridge::SLOT.clear(id);
    }
}
