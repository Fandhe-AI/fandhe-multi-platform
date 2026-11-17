//! macOS / Windows 向けの写真選択実装 [`RfdPhotoPicker`]。ファイル選択（`fandhe-cap-file-picker` の `RfdFilePicker`）と同じ OS 分けを
//! 踏襲する: macOS は `rfd`（`NSOpenPanel`）、Windows は PowerShell 子プロセス（`fandhe_device::file_picker::run_windows_pick`。
//! 終了コードで 3 値を区別。rfd は失敗をキャンセルへ化けさせるため使わない）。`crate::driver` の `platform_photo_picker` から呼ばれる。
//! UI スレッドから同期で呼ぶ契約（`NSOpenPanel` は main スレッド必須）。
//!
//! 起動する PowerShell は絶対パスの固定スクリプトで、外部入力を連結しない（security.md A03）。

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, Ordering};

use fandhe_device::DeviceError;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use fandhe_device::file_picker::BusyGuard;
use fandhe_device::file_picker::PickedFile;
#[cfg(target_os = "macos")]
use fandhe_device::file_picker::read_picked;
#[cfg(target_os = "windows")]
use fandhe_device::file_picker::{run_windows_pick, windows_pick_command};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::contract::{MAX_PHOTO_BYTES, PhotoPicker};
use crate::contract::{PickedPhoto, validate_photo};
use crate::error::PhotoPickerError;

/// Windows の `OpenFileDialog.Filter` に渡す固定フィルタ。デスクトップの復号が PNG / JPEG のみ（依存を増やさないため。GIF / WebP は
/// 対象外）なので、ダイアログ側でも絞る。
#[cfg(target_os = "windows")]
const WINDOWS_PHOTO_FILTER: &str = "Images (*.png;*.jpg;*.jpeg)|*.png;*.jpg;*.jpeg";

/// デスクトップのダイアログが返す 3 値（選択 / キャンセル / 失敗）を [`PickedPhoto`] の契約へ写す純関数。キャンセルは `Cancelled`、
/// 選択は 1 段目 [`validate_photo`] を通す。OS を問わずホストでテストできるよう `cfg` を付けない。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn photo_from_pick(
    picked: Result<Option<PickedFile>, DeviceError>,
) -> Result<PickedPhoto, PhotoPickerError> {
    match picked? {
        None => Err(PhotoPickerError::Cancelled("photo_picker")),
        Some(f) => Ok(validate_photo(f.bytes)?),
    }
}

/// macOS / Windows 向け実装。`busy` は「進行中に再度呼ばれたら `Internal`」という契約（`crate::contract` の doc）を守るためのガード。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct RfdPhotoPicker {
    busy: AtomicBool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl RfdPhotoPicker {
    /// 構築は失敗しない。
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Default for RfdPhotoPicker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl PhotoPicker for RfdPhotoPicker {
    fn pick(&self) -> Result<PickedPhoto, PhotoPickerError> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(PhotoPickerError::Internal("photo_picker busy"));
        }
        let _guard = BusyGuard(&self.busy);

        #[cfg(target_os = "macos")]
        let picked = match rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_file()
        {
            None => Ok(None),
            Some(path) => read_picked(&path, MAX_PHOTO_BYTES).map(Some),
        };
        #[cfg(target_os = "windows")]
        let picked = run_windows_pick(
            windows_pick_command(Some(WINDOWS_PHOTO_FILTER)),
            MAX_PHOTO_BYTES,
        );
        photo_from_pick(picked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ImageFormat, MAX_PHOTO_BYTES};

    fn file(bytes: &[u8]) -> PickedFile {
        PickedFile {
            name: "a".to_string(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn cancel_is_cancelled_not_success() {
        assert!(matches!(
            photo_from_pick(Ok(None)),
            Err(PhotoPickerError::Cancelled(_))
        ));
    }

    #[test]
    fn pick_failure_passes_through() {
        assert!(matches!(
            photo_from_pick(Err(DeviceError::Internal("x"))),
            Err(PhotoPickerError::Internal(_))
        ));
    }

    #[test]
    fn picked_file_goes_through_stage_one() {
        assert!(matches!(
            photo_from_pick(Ok(Some(file(b"")))),
            Err(PhotoPickerError::Internal(_))
        ));
        assert!(matches!(
            photo_from_pick(Ok(Some(file(b"not an image")))),
            Err(PhotoPickerError::Internal(_))
        ));
        let mut big = vec![0xFF, 0xD8, 0xFF];
        big.resize(MAX_PHOTO_BYTES + 1, 0);
        assert!(matches!(
            photo_from_pick(Ok(Some(file(&big)))),
            Err(PhotoPickerError::InvalidInput(_))
        ));
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0];
        assert_eq!(
            photo_from_pick(Ok(Some(file(&png)))).unwrap().format,
            ImageFormat::Png
        );
    }

    #[cfg(unix)]
    fn sh(script: &str) -> fandhe_device::file_picker::PickCommand {
        fandhe_device::file_picker::PickCommand {
            program: std::path::PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    #[test]
    #[cfg(unix)]
    fn windows_style_cancel_failure_and_non_image_are_not_success() {
        use fandhe_device::file_picker::run_windows_pick;
        let run = |script: &str| photo_from_pick(run_windows_pick(sh(script), MAX_PHOTO_BYTES));
        // キャンセル専用コード 2。
        assert!(matches!(run("exit 2"), Err(PhotoPickerError::Cancelled(_))));
        // それ以外の非ゼロは失敗。
        assert!(matches!(run("exit 1"), Err(PhotoPickerError::Internal(_))));
        // 画像でないファイルは 1 段目で落ちる。
        let path = std::env::temp_dir().join(format!("fandhe-photo-test-{}", std::process::id()));
        std::fs::write(&path, b"plain text").unwrap();
        let r = run(&format!("printf '%s' '{}'", path.display()));
        let _ = std::fs::remove_file(&path);
        assert!(matches!(r, Err(PhotoPickerError::Internal(_))), "{r:?}");
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn rfd_photo_picker_busy_is_internal() {
        let picker = RfdPhotoPicker::new();
        picker.busy.store(true, Ordering::Release);
        assert!(matches!(picker.pick(), Err(PhotoPickerError::Internal(_))));
    }
}
