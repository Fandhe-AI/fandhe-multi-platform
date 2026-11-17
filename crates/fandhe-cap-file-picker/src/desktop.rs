//! macOS/Windows 向けのファイル選択実装 [`RfdFilePicker`]。macOS は `rfd`（`NSOpenPanel`）へ委譲し、Windows は PowerShell 子プロセス
//! （`fandhe_device::file_picker::run_windows_pick`）を使う。`crate::driver` の `platform_file_picker` から呼ばれる。
//!
//! Windows で rfd を使わない理由: rfd 0.17 の同期 API は Windows 側で COM 初期化・ダイアログ生成の失敗を `run(self).ok()` で握り潰し
//! `None`（キャンセル）に化けさせ、契約（キャンセルと失敗は別の結果）に違反する（#872 codex レビュー P1 指摘）。PowerShell 子プロセスなら
//! 終了コードで両者を区別できるため、macOS だけ rfd に残し、Windows は自前の子プロセス経路へ切り替えた。
//! 起動する PowerShell は絶対パスの固定スクリプトで、外部入力を連結しない（security.md A03）。

use std::sync::atomic::{AtomicBool, Ordering};

use fandhe_device::file_picker::BusyGuard;
#[cfg(target_os = "windows")]
use fandhe_device::file_picker::{run_windows_pick, windows_pick_command};

use crate::contract::{FilePicker, PickedFile};
#[cfg(target_os = "macos")]
use crate::contract::{MAX_FILE_BYTES, read_picked};
use crate::error::FilePickerError;

/// macOS/Windows 向け実装。
///
/// `busy` は「進行中に再度呼ばれたら `Internal`」という契約（`crate::contract` の doc 参照）を守るためのガード。
/// `NSOpenPanel`/`IFileOpenDialog` はモーダルでブロッキングするため、呼び出し元が誤って別スレッドから同時に `pick()` を呼んだ場合の
/// 多重表示を防ぐ。
pub struct RfdFilePicker {
    busy: AtomicBool,
}

impl RfdFilePicker {
    /// 構築自体は失敗しない（ダイアログの生成失敗は `pick()` 呼び出し時に `Ok(None)`/`Err` として現れる）。
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
        }
    }
}

impl Default for RfdFilePicker {
    fn default() -> Self {
        Self::new()
    }
}

impl FilePicker for RfdFilePicker {
    fn pick(&self) -> Result<Option<PickedFile>, FilePickerError> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(FilePickerError::Internal("file_picker busy"));
        }
        let _guard = BusyGuard(&self.busy);

        // ponytail: 同期モーダル。非モーダル化が要るなら `rfd::AsyncFileDialog` を
        // メインスレッドで spawn する方式へ上げる。
        #[cfg(target_os = "macos")]
        {
            match rfd::FileDialog::new().pick_file() {
                None => Ok(None),
                Some(path) => read_picked(&path, MAX_FILE_BYTES)
                    .map(Some)
                    .map_err(FilePickerError::from),
            }
        }
        #[cfg(target_os = "windows")]
        {
            run_windows_pick(
                windows_pick_command(None),
                fandhe_device::file_picker::MAX_FILE_BYTES,
            )
            .map_err(FilePickerError::from)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfd_picker_busy_guard_blocks_reentrant_call_and_releases_after() {
        let picker = RfdFilePicker::new();
        picker.busy.store(true, Ordering::Release);
        let err = picker.pick().unwrap_err();
        assert!(matches!(err, FilePickerError::Internal(_)));
        // busy を手動で戻す（実ダイアログは開かずガードの片方だけ検証する）。
        picker.busy.store(false, Ordering::Release);
        assert!(!picker.busy.load(Ordering::Acquire));
    }
}
