//! macOS 向けプレビュー実装（Issue #917）。`device/file_preview.rs` の macOS 経路から呼ばれ、
//! `macos/fandhe_macos_file_preview.m`（自前の窓に載せる `QLPreviewView`。`build.rs` が feature
//! `device` 有効時のみコンパイルする）を `unsafe extern "C"` 1 関数（[`present`]）で呼ぶ。
//! `unsafe` はこの呼び出しと受け口に閉じ `// SAFETY:` で契約を明記する
//! （`macos_eye_dropper.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! 窓が閉じると `.m` から [`fandhe_rust_file_preview_outcome`] が呼ばれ、
//! `device/file_preview::bridge` へ結果を push する（pull 型）。コード → 結果の写像は
//! `fandhe_device::file_preview::preview_from_macos_code` に一本化する。固定ファイルは
//! `temp_dir()/fandhe-file-preview/` へ書き、そのパスだけを `.m` へ渡す。

use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Mutex;

use fandhe_device::file_preview::{
    preview_from_macos_code, write_fixed, FileKind, PreviewResult, DIR_NAME, WRITE,
};
use fandhe_device::DeviceError;

/// 表示中の窓が参照する一時ファイル（要求 id → パス）。窓が閉じたとき（または提示失敗時）に消す。
/// 閉じた通知が届かない場合は残る（`fandhe_device::file_preview` の「守らない範囲」）。
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

// SAFETY: `macos/fandhe_macos_file_preview.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_macos_file_preview_present(request_id: u64, path: *const c_char) -> i32;
}

/// プレビュー窓の表示を依頼する。受理できれば `Ok(None)`（結果は窓が閉じたときに
/// [`fandhe_rust_file_preview_outcome`] 経由で届く）。受理できなければ `Internal`。
#[allow(unsafe_code)]
pub(crate) fn present(
    request_id: u64,
    kind: FileKind,
    bytes: &[u8],
) -> Result<Option<PreviewResult>, DeviceError> {
    let path = write_fixed(&std::env::temp_dir().join(DIR_NAME), kind, bytes)?;
    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
        let _ = std::fs::remove_file(&path);
        return Err(DeviceError::Internal(WRITE));
    };
    // `.m` が窓の通知を返す前に登録する（通知が先に届いても取りこぼさない）。
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
    // main queue へ dispatch して即座に 0 を返す契約（ポインタを保持しない）。
    let code = unsafe { fandhe_macos_file_preview_present(request_id, cpath.as_ptr()) };
    if code == 0 {
        Ok(None)
    } else {
        remove_pending_file(request_id);
        Err(DeviceError::Internal("file_preview present"))
    }
}

/// `.m` 側の窓の通知から呼ばれる受け口。ポインタを受けない。macOS は event loop 側が待機中に
/// `request_repaint_after` を出すため repaint は呼ばない。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_file_preview_outcome(request_id: u64, code: i32) {
    remove_pending_file(request_id);
    super::file_preview::bridge::deliver(request_id, preview_from_macos_code(code));
}
