//! Phase 11（親 #1121）型サンプル「写真の保存」（Issue #1254）の effect ドライバ。
//! `fandhe_core::screens::photo_save`（端末依存ゼロ）の要求を観測し、アプリが生成した PNG
//! （[`sample_png`]）を `fandhe_device::photo_library::PngImage` で検査してから OS へ渡し、
//! 結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `device/calendar.rs` と同型）。契約と守る範囲は `fandhe_device::photo_library` の
//! モジュール doc を参照。
//!
//! - macOS（#1254）/ iOS（#1255）/ Android（#1256）: コールバック型のため pull 型 [`bridge`]
//!   （`RequestSlot` の facade）で毎フレーム [`drive_callback`] が結果を反映する。
//!   Android は #1256 が埋めるまでスタブが `Unsupported` を返す
//! - Windows（#1254）/ それ以外: [`PhotoLibrary`] を同期に呼ぶ。Windows は Pictures フォルダへ
//!   書き、Linux は `UnsupportedPhotoLibrary` で「非対応」を表示する

use fandhe_core::screens::photo_save::{
    finish_save, mark_saving, take_request, PhotoSaveState, SaveOutcome, SavedPlace,
};
use fandhe_device::photo_library::{PngImage, SavedTo};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 保存する見本画像（256x256 のグラデーション PNG）。
pub(crate) fn sample_png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(256, 256, |x, y| image::Rgba([x as u8, y as u8, 160, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    // メモリ上への PNG 書き出しは失敗しない。万一失敗しても空バイト列は
    // `PngImage::new` が入力不正として弾くため、成功は作られない。
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<SavedTo, DeviceError>) -> Result<SaveOutcome, String> {
    match result {
        Ok(SavedTo::PhotoLibrary) => Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary)),
        Ok(SavedTo::MediaStore) => Ok(SaveOutcome::Saved(SavedPlace::Gallery)),
        Ok(SavedTo::PicturesFolder) => Ok(SaveOutcome::Saved(SavedPlace::PicturesFolder)),
        Err(DeviceError::PermissionDenied(_)) => Ok(SaveOutcome::Denied),
        Err(DeviceError::Unsupported(_)) => Ok(SaveOutcome::Unsupported),
        Err(DeviceError::InvalidInput(_)) => Err("画像が不正です".to_string()),
        Err(_) => Err("OS の保存処理に失敗しました".to_string()),
    }
}

/// `.m` 側の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | 保存確定 | `Ok(PhotoLibrary)` |
/// | 1 | 拒否・制限・未決定のまま・入力の受け渡し不正 | `PermissionDenied` |
/// | 2 | macOS 11 / iOS 14 未満 | `Unsupported` |
/// | 3 | `performChanges` の失敗 | `Internal("photo_library save")` |
/// | 4 | 認可要求の異常 | `Internal("photo_library access")` |
/// | 他 | 未知 | `Internal("photo_library outcome")` |
///
/// ホストでもテストできるよう cfg は割かない。呼び出し元は macOS / iOS。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) fn outcome_from_apple_code(code: i32) -> Result<SavedTo, DeviceError> {
    match code {
        0 => Ok(SavedTo::PhotoLibrary),
        1 => Err(DeviceError::PermissionDenied("photo_library")),
        2 => Err(DeviceError::Unsupported("photo_library")),
        3 => Err(DeviceError::Internal("photo_library save")),
        4 => Err(DeviceError::Internal("photo_library access")),
        _ => Err(DeviceError::Internal("photo_library outcome")),
    }
}

/// OS 側コールバックとの橋渡し（macOS / iOS / Android、pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::photo_save::SaveOutcome;
    use fandhe_device::photo_library::SavedTo;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<SaveOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(request_id: u64, result: Result<SavedTo, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。[`SLOT`] はプロセス static の
    /// ため再生成をまたいで残る。保留中の要求は復元せず破棄し、以後届く旧 `request_id` の
    /// [`deliver`] を無視させる（`calendar::bridge::reset` と同じ位置づけ）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

/// OS へ保存を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn begin_save(request_id: u64, image: &PngImage) -> Result<(), DeviceError> {
    super::apple_photo_library::begin_save(request_id, image)
}

#[cfg(target_os = "android")]
fn begin_save(request_id: u64, image: &PngImage) -> Result<(), DeviceError> {
    super::android_photo_library::begin_save(request_id, image)
}

/// macOS / iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
fn drive_callback(state: &PhotoSaveState, sender: &CompletionSender<PhotoSaveState>) {
    use fandhe_core::screens::photo_save::SavePhase;

    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_saving));
        let request_id = bridge::begin();
        // 検査を通らない画像は OS へ渡さずその場で画面へ返す。
        let accepted = PngImage::new(sample_png()).and_then(|png| begin_save(request_id, &png));
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != SavePhase::Saving {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut PhotoSaveState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != SavePhase::Idle {
                finish_save(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `photo_save` 画面用に配線される effect ドライバ（macOS / iOS / Android）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub fn photo_save_effect(state: &PhotoSaveState, sender: &CompletionSender<PhotoSaveState>) {
    drive_callback(state, sender);
}

/// 同期経路（Windows・Linux）: 検査してから [`PhotoLibrary`] を呼ぶ。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
fn drive(
    state: &PhotoSaveState,
    sender: &CompletionSender<PhotoSaveState>,
    lib: &dyn fandhe_device::photo_library::PhotoLibrary,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_saving));
    let result = PngImage::new(sample_png()).and_then(|png| lib.save_png(&png));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut PhotoSaveState| {
        finish_save(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ（Windows・Linux）。
///
/// ponytail: Windows は小さな PNG 1 枚の書き込みなので UI スレッドで同期に書く。Pictures が
/// ネットワーク先へリダイレクトされて遅いと分かったら、作業スレッド + bridge へ移す。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
pub fn photo_save_effect(state: &PhotoSaveState, sender: &CompletionSender<PhotoSaveState>) {
    #[cfg(target_os = "windows")]
    drive(
        state,
        sender,
        &super::windows_photo_library::WindowsPhotoLibrary,
    );
    #[cfg(not(target_os = "windows"))]
    drive(
        state,
        sender,
        &fandhe_device::photo_library::UnsupportedPhotoLibrary,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_outcome_covers_all_paths() {
        assert_eq!(
            to_outcome(Ok(SavedTo::PhotoLibrary)),
            Ok(SaveOutcome::Saved(SavedPlace::PhotoLibrary))
        );
        assert_eq!(
            to_outcome(Ok(SavedTo::MediaStore)),
            Ok(SaveOutcome::Saved(SavedPlace::Gallery))
        );
        assert_eq!(
            to_outcome(Ok(SavedTo::PicturesFolder)),
            Ok(SaveOutcome::Saved(SavedPlace::PicturesFolder))
        );
        assert_eq!(
            to_outcome(Err(DeviceError::PermissionDenied("photo_library"))),
            Ok(SaveOutcome::Denied)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("photo_library"))),
            Ok(SaveOutcome::Unsupported)
        );
        assert!(to_outcome(Err(DeviceError::InvalidInput("photo_library png"))).is_err());
        assert!(to_outcome(Err(DeviceError::Internal("photo_library save"))).is_err());
    }

    #[test]
    fn outcome_from_apple_code_table() {
        assert!(matches!(
            outcome_from_apple_code(0),
            Ok(SavedTo::PhotoLibrary)
        ));
        assert!(matches!(
            outcome_from_apple_code(1),
            Err(DeviceError::PermissionDenied("photo_library"))
        ));
        assert!(matches!(
            outcome_from_apple_code(2),
            Err(DeviceError::Unsupported("photo_library"))
        ));
        for code in [3, 4, 5, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_apple_code(code),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn sample_png_passes_validation() {
        assert!(PngImage::new(sample_png()).is_ok());
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use fandhe_core::screens::photo_save::{save, SavePhase};
        use fandhe_device::photo_library::UnsupportedPhotoLibrary;

        #[test]
        fn unsupported_is_shown_and_never_saved() {
            let mut s = PhotoSaveState::default();
            save(&mut s);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, &UnsupportedPhotoLibrary);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            assert_eq!(s.phase, SavePhase::Idle);
            assert!(!s.status.contains("保存しました"));
            assert!(!s.status.is_empty());
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod bridge_tests {
    use super::bridge;
    use super::*;

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id + 1000, Ok(SavedTo::PhotoLibrary)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(
            id,
            Err(DeviceError::PermissionDenied("photo_library"))
        ));
        assert!(!bridge::deliver(id, Ok(SavedTo::PhotoLibrary)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(SaveOutcome::Denied)));
        bridge::SLOT.clear(id);
    }
}
