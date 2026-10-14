//! macOS / iOS 向け画面録画（macOS: Issue #1335、iOS: #1336）。
//! `device/screen_capture.rs` の dispatch から呼ばれ、ScreenCaptureKit 実装
//! （`apple/fandhe_apple_screen_capture.m`、`build.rs` が macOS で compile）を
//! `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` は呼び出し 1 か所ずつに閉じ `// SAFETY:` を付ける
//! （coding-rust.md の FFI 例外規定）。
//!
//! `.m` の完了コールバックから、本モジュールが公開する `extern "C"` 2 関数
//! （[`fandhe_rust_screen_capture_started`] / [`fandhe_rust_screen_capture_finished`]）が呼ばれ、
//! `screen_capture::bridge` へ結果を push する。映像のバイト列は FFI を越えない（長さのみ）。
//! 映像内容・ファイルパスはログへ出さない（security.md A09）。
//!
//! # iOS（#1336）
//!
//! `.m` の `#if TARGET_OS_IPHONE` 側が ReplayKit（`RPScreenRecorder`）で自アプリの画面を録画する。
//! OS コールバックが結果を共有状態へ入れた直後に `crate::ios::request_repaint` で次フレームを
//! 起こす（契約表 4。`started` も対象: 同意待ち中は入力が無く、経過秒数が止まって見えるため）。

#[cfg(any(target_os = "macos", target_os = "ios"))]
use std::ffi::CStr;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use std::os::raw::c_char;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use std::time::Duration;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use fandhe_device::screen_capture::{build_capture_result, ScreenCapturer};
#[cfg(any(target_os = "macos", target_os = "ios"))]
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_screen_capture.m` が定義する C ABI（`build.rs` が macOS で
// compile・リンクする）。契約は `.m` 冒頭コメントに集約する。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_screen_capture_start(request_id: u64, max_seconds: f64) -> i32;
    fn fandhe_apple_screen_capture_stop(request_id: u64) -> i32;
}

/// macOS は ScreenCaptureKit、iOS は ReplayKit 実装。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) struct AppleScreenCapturer;

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl ScreenCapturer for AppleScreenCapturer {
    #[allow(unsafe_code)]
    fn start(&self, request_id: u64, max_duration: Duration) -> Result<(), DeviceError> {
        // SAFETY: 引数はプリミティブのみ。戻り値 0 は依頼の受理のみを意味し、結果は非同期に
        // `fandhe_rust_screen_capture_finished` 経由で届く。
        let code =
            unsafe { fandhe_apple_screen_capture_start(request_id, max_duration.as_secs_f64()) };
        if code == 0 {
            Ok(())
        } else {
            Err(super::screen_capture::error_from_code(code))
        }
    }

    #[allow(unsafe_code)]
    fn stop(&self, request_id: u64) -> Result<(), DeviceError> {
        // SAFETY: 引数はプリミティブのみ。`.m` 側は常に 0（受理）を返し、request_id 不一致の判定は
        // main queue 上で行って黙って捨てる。非 0 は ABI 契約外の防御分岐。
        let code = unsafe { fandhe_apple_screen_capture_stop(request_id) };
        if code == 0 {
            Ok(())
        } else {
            Err(DeviceError::Internal("screen_capture stale"))
        }
    }
}

/// OS 別エンジンを返す（`screen_capture::capturer` の dispatch 先）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn capturer() -> &'static dyn ScreenCapturer {
    static CAPTURER: AppleScreenCapturer = AppleScreenCapturer;
    &CAPTURER
}

/// `.m` が録画を実際に開始した瞬間に呼ぶ。同意ダイアログ中は呼ばれない
/// （経過時間の計測をこの時点から始める）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_screen_capture_started(request_id: u64) {
    super::screen_capture::bridge::mark_started(request_id);
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}

/// `.m` の完了コールバック。`code`: 0 = 成功（`size_bytes` / `mime` / `duration_ms` が有効）、
/// 他は `screen_capture::error_from_code` の表。0 以外では `mime` は NULL。
/// `mime` はこの呼び出しが戻るまでのみ有効なため、同期的にコピーし切る。
/// 不変条件（0 バイト・0 ミリ秒の拒否）は `build_capture_result` に一本化する。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_screen_capture_finished(
    request_id: u64,
    code: i32,
    size_bytes: u64,
    mime: *const c_char,
    duration_ms: u64,
) {
    let result = if code == 0 {
        if mime.is_null() {
            Err(DeviceError::Internal(
                "screen_capture: null result on success",
            ))
        } else {
            // SAFETY: `code == 0` の契約により `mime` は NUL 終端文字列を指し、この呼び出しが
            // 戻るまで有効。ここで即座にコピーする。
            let mime = unsafe { CStr::from_ptr(mime) }
                .to_string_lossy()
                .into_owned();
            build_capture_result(size_bytes, mime, duration_ms)
        }
    } else {
        Err(super::screen_capture::error_from_code(code))
    };
    super::screen_capture::bridge::deliver(request_id, result);
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
