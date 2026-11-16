//! Windows 向け写真保存（Issue #1254、親 #1121）。`driver.rs` の Windows 経路から
//! 呼ばれる。Windows には写真ライブラリを管理する OS API が無く、Pictures は通常のフォルダ。
//! 本モジュールは Pictures のパス解決（`SHGetKnownFolderPath(FOLDERID_Pictures)`）だけを持ち、
//! 書き込みは `crate::contract::FolderPhotoLibrary`（safe code、失敗時の
//! 部分ファイル削除・上書き禁止込み）に任せる。権限・package identity は要らない。
//!
//! OneDrive 等へリダイレクトされたパスもそのまま使う（同期の完了は守らない範囲、契約は
//! `crate::contract` の doc）。`CoInitializeEx` は不要（Known Folder API と
//! `CoTaskMemFree` は COM の初期化を要さない）。Known Folder の解決（`unsafe`）は `fandhe_platform_windows::known_folder` が持つ。

use crate::contract::{FolderPhotoLibrary, PhotoLibrary, PngImage, SavedTo};
use crate::error::PhotoLibraryError;
use windows::Win32::UI::Shell::FOLDERID_Pictures;

/// Pictures フォルダへ保存する [`PhotoLibrary`]。呼ばれるたびにパスを解決する
/// （利用者がフォルダの場所を変えても追従する）。
pub(crate) struct WindowsPhotoLibrary;

impl PhotoLibrary for WindowsPhotoLibrary {
    fn save_png(&self, image: &PngImage) -> Result<SavedTo, PhotoLibraryError> {
        FolderPhotoLibrary::new(
            fandhe_platform_windows::known_folder(&FOLDERID_Pictures)
                .ok_or(PhotoLibraryError::Internal("photo_library folder"))?,
        )
        .save_png(image)
    }
}
