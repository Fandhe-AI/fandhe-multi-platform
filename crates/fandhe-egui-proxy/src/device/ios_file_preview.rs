//! iOS 向けファイル・動画のプレビューとサムネイル（Issue #919、親 #182）。
//!
//! 呼び出し元: `device/file_preview.rs` の iOS 経路が [`thumbnailer`]・[`present_preview`] を呼ぶ。
//! - サムネイルは macOS と共用の `apple_file_preview::AppleFileThumbnailer`（QuickLookThumbnailing。
//!   iOS 13 未満は `Unsupported`）。結果はワーカーが `request_repaint_after` で起こすため、
//!   ここで足す再描画は無い。
//! - プレビューは `ios/fandhe_ios_file_preview.m`（`QLPreviewController`）を `unsafe extern "C"` 1 関数で呼ぶ。
//!   閉じた通知（`previewControllerDidDismiss:`）で [`fandhe_rust_ios_file_preview_outcome`] が呼ばれ、
//!   `file_preview::bridge` へ結果を push した直後に `crate::ios::request_repaint` で次フレームを起こす
//!   （入力が無いと egui が描画せず `Working` のまま止まるため。契約表 4）。
//!
//! 固定ファイルは `temp_dir()/fandhe-file-preview/` へ書き、そのパスだけを `.m` へ渡す
//! （任意のパスを受け取らない）。コード → 結果の写像は
//! `fandhe_device::file_preview::preview_from_macos_code`（macOS 共用）に一本化する。
//! 守らない範囲: 閉じた通知が届かない場合の復旧、複数 scene、プロセス死後の結果
//! （`fandhe_device::file_preview` の trait doc のとおり）。

use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fandhe_device::file_preview::{
    preview_from_macos_code, write_fixed, FileKind, FileThumbnailer, PreviewResult, DIR_NAME, WRITE,
};
use fandhe_device::DeviceError;

/// 表示中のプレビューが参照する一時ファイル（要求 id → パス）。閉じたとき（または提示失敗時）に消す。
static PENDING_FILES: Mutex<Option<HashMap<u64, PathBuf>>> = Mutex::new(None);

fn remove_pending_file(request_id: u64) {
    let path = PENDING_FILES
        .lock()
        .ok()
        .and_then(|mut g| g.as_mut().and_then(|m| m.remove(&request_id)));
    if let Some(p) = path {
        let _ = std::fs::remove_file(p);
    }
}

// SAFETY: `ios/fandhe_ios_file_preview.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_file_preview_present(request_id: u64, path: *const c_char) -> i32;
}

/// iOS のサムネイル実装（macOS と共用の QuickLookThumbnailing）。
pub(crate) fn thumbnailer() -> Arc<dyn FileThumbnailer> {
    Arc::new(super::apple_file_preview::AppleFileThumbnailer)
}

/// プレビューの提示を依頼する。受理できれば `Ok(None)`（結果は閉じたときに
/// [`fandhe_rust_ios_file_preview_outcome`] 経由で届く）。受理できなければ `Internal`。
#[allow(unsafe_code)]
pub(crate) fn present_preview(
    request_id: u64,
    kind: FileKind,
    bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, DeviceError> {
    let path = write_fixed(&std::env::temp_dir().join(DIR_NAME), kind, &bytes)?;
    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
        let _ = std::fs::remove_file(&path);
        return Err(DeviceError::Internal(WRITE));
    };
    // `.m` の通知が返る前に登録する（通知が先に届いても取りこぼさない）。
    match PENDING_FILES.lock() {
        Ok(mut g) => {
            g.get_or_insert_with(HashMap::new).insert(request_id, path);
        }
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return Err(DeviceError::Internal(WRITE));
        }
    }
    // SAFETY: `cpath` は NUL 終端の有効な C 文字列で、`.m` は呼び出しの間にコピーしてから
    // main queue へ dispatch して即座に戻る契約（ポインタを保持しない）。
    let code = unsafe { fandhe_ios_file_preview_present(request_id, cpath.as_ptr()) };
    if code == 0 {
        Ok(None)
    } else {
        remove_pending_file(request_id);
        Err(DeviceError::Internal("file_preview present"))
    }
}

/// `.m` 側の閉じた通知・提示失敗から 1 回だけ呼ばれる受け口。ポインタを受けない。
/// 結果を置いた直後に次フレームを起こす。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ios_file_preview_outcome(request_id: u64, code: i32) {
    remove_pending_file(request_id);
    super::file_preview::bridge::deliver(request_id, preview_from_macos_code(code));
    crate::ios::request_repaint();
}
