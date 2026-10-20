//! Windows 向け写真保存（Issue #1254、親 #1121）。`device/photo_save.rs` の Windows 経路から
//! 呼ばれる。Windows には写真ライブラリを管理する OS API が無く、Pictures は通常のフォルダ。
//! 本モジュールは Pictures のパス解決（`SHGetKnownFolderPath(FOLDERID_Pictures)`）だけを持ち、
//! 書き込みは `fandhe_device::photo_library::FolderPhotoLibrary`（safe code、失敗時の
//! 部分ファイル削除・上書き禁止込み）に任せる。権限・package identity は要らない。
//!
//! OneDrive 等へリダイレクトされたパスもそのまま使う（同期の完了は守らない範囲、契約は
//! `fandhe_device::photo_library` の doc）。`CoInitializeEx` は不要（Known Folder API と
//! `CoTaskMemFree` は COM の初期化を要さない）。`unsafe` は [`known_folder`] に閉じる。

use std::path::PathBuf;

use fandhe_device::photo_library::{FolderPhotoLibrary, PhotoLibrary, PngImage, SavedTo};
use fandhe_device::DeviceError;
use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

/// Known Folder のパスを OS から得る。失敗は `None`（HRESULT の詳細は載せない）。
/// 呼び出し元は Pictures（本モジュール）と Music（`windows_music_library`）。
#[allow(unsafe_code)]
pub(super) fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: `id` は有効な KNOWNFOLDERID への参照で、token は既定（現在の
    // ユーザー）。成功時の戻り値は呼び出し側が `CoTaskMemFree` で 1 回だけ解放する所有権付き
    // の NUL 終端 UTF-16。
    let pwstr = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: `pwstr` は上で得た有効な NUL 終端文字列。変換の成否にかかわらず直後に解放する
    // ため、変換失敗の経路でもリークしない。
    let converted = unsafe { pwstr.to_string() };
    // SAFETY: `pwstr.0` は `SHGetKnownFolderPath` が確保したブロックで、ここで 1 回だけ解放する。
    unsafe { CoTaskMemFree(Some(pwstr.0 as *const _)) };
    converted.ok().map(PathBuf::from)
}

/// Pictures フォルダへ保存する [`PhotoLibrary`]。呼ばれるたびにパスを解決する
/// （利用者がフォルダの場所を変えても追従する）。
pub(crate) struct WindowsPhotoLibrary;

impl PhotoLibrary for WindowsPhotoLibrary {
    fn save_png(&self, image: &PngImage) -> Result<SavedTo, DeviceError> {
        FolderPhotoLibrary::new(
            known_folder(&FOLDERID_Pictures)
                .ok_or(DeviceError::Internal("photo_library folder"))?,
        )
        .save_png(image)
    }
}
