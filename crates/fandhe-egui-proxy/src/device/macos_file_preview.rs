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

use std::ffi::{c_char, CString};
use std::os::unix::ffi::OsStrExt;

use fandhe_device::file_preview::{
    preview_from_macos_code, write_fixed, FileKind, PreviewResult, DIR_NAME, WRITE,
};
use fandhe_device::DeviceError;

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
    let cpath =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| DeviceError::Internal(WRITE))?;
    // SAFETY: `cpath` は NUL 終端の有効な C 文字列で、`.m` は呼び出しの間にコピーしてから
    // main queue へ dispatch して即座に 0 を返す契約（ポインタを保持しない）。
    let code = unsafe { fandhe_macos_file_preview_present(request_id, cpath.as_ptr()) };
    if code == 0 {
        Ok(None)
    } else {
        Err(DeviceError::Internal("file_preview present"))
    }
}

/// `.m` 側の窓の通知から呼ばれる受け口。ポインタを受けない。macOS は event loop 側が待機中に
/// `request_repaint_after` を出すため repaint は呼ばない。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_file_preview_outcome(request_id: u64, code: i32) {
    super::file_preview::bridge::deliver(request_id, preview_from_macos_code(code));
}
