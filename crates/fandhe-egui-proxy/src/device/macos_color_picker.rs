//! macOS 向けシステムの色選択ダイアログ実装（Issue #941）。`device/color_picker.rs` の macOS 経路から
//! 呼ばれ、`macos/fandhe_macos_color_picker.m`（`NSColorPanel`。`build.rs` が feature `device` 有効時のみ
//! コンパイルする）を `unsafe extern "C"` 1 関数（[`present`]）で呼ぶ。
//! `unsafe` はこの呼び出しと受け口に閉じ `// SAFETY:` で契約を明記する
//! （`macos_eye_dropper.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の閉じた通知から [`fandhe_rust_macos_color_picker_outcome`] が呼ばれ、
//! `device/color_picker::bridge` へ結果を push する（pull 型）。コード → 結果の写像は
//! `fandhe_device::color_picker::result_from_apple_code` に一本化する。
//!
//! 選んだ色をログへ出力しない。

use fandhe_device::color_picker::{result_from_apple_code, INITIAL_RGB};
use fandhe_device::DeviceError;

// SAFETY: `macos/fandhe_macos_color_picker.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_macos_color_picker_present(request_id: u64, initial_rgb: i32) -> i32;
}

/// 色パネルの表示を依頼する。`request_id` は `bridge::begin` が払い出した ID。
/// `Err` は依頼自体の失敗（呼び出し元が `bridge::deliver` へ畳む）。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64) -> Result<(), DeviceError> {
    // SAFETY: 引数は値渡しの整数のみでポインタを渡さない。`.m` 側は main queue へ
    // dispatch してから即座に 0 を返す契約。
    let code = unsafe { fandhe_macos_color_picker_present(request_id, INITIAL_RGB as i32) };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("color_picker present"))
    }
}

/// `.m` 側の通知から呼ばれる受け口。ポインタを受けず、色は整数 `rgb` で渡る。
/// macOS は event loop 側が待機中に `request_repaint_after` を出すため repaint は呼ばない。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_macos_color_picker_outcome(request_id: u64, code: i32, rgb: i32) {
    super::color_picker::bridge::deliver(request_id, result_from_apple_code(code, rgb));
}
