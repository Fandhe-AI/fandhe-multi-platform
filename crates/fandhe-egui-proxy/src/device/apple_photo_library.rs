//! macOS / iOS 向け写真ライブラリ保存実装（Issue #1254 macOS・#1255 iOS）。
//! `device/photo_save.rs` の iOS / macOS 経路から呼ばれ、Photos 実装
//! （`apple/fandhe_apple_photo_library.m`、`build.rs` が feature `device` 有効時のみ
//! コンパイル・リンクする）を `unsafe extern "C"` 1 関数（[`begin_save`]）で呼ぶ。
//! `unsafe` はこの呼び出し 1 箇所に閉じ `// SAFETY:` で契約を明記する
//! （`apple_calendar.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の完了から [`fandhe_rust_photo_library_outcome`] が呼ばれ、`device/photo_save::bridge`
//! へ結果を置く（pull 型）。結果コード → 結果の写像は `device/photo_save.rs::
//! outcome_from_apple_code` に一本化する。画像のバイト列はログへ出力しない（security.md A09）。
//!
//! iOS（#1255）は Photos の API が同じため、同じ `.m` を分岐なしで共用する。iOS ではコールバックが
//! egui の `update` 外から来るので、結果を置いた直後に `ios::request_repaint` で次フレームを起こす。

use fandhe_device::photo_library::PngImage;
use fandhe_device::DeviceError;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use std::os::raw::c_int;

// SAFETY: `apple/fandhe_apple_photo_library.m` が定義する C ABI。契約は `.m` 冒頭コメントと
// 下記 `begin_save` のコメントに集約する。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_photo_library_save(request_id: u64, bytes: *const u8, len: usize) -> c_int;
}

/// 検査済みの PNG を Photos へ渡す依頼を出す。`Ok` は受理済み（結果は後から
/// `bridge::deliver` で届く）、`Err` は依頼自体が受理されなかった。`request_id` は
/// `bridge::begin` が払い出したプロセス内一意な ID。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
pub(crate) fn begin_save(request_id: u64, image: &PngImage) -> Result<(), DeviceError> {
    let bytes = image.bytes();
    // SAFETY: `bytes` は呼び出しが返るまで生存する有効なスライスで、`len` はその長さ。
    // `.m` 側は返る前に `NSData` へ値コピーするため、所有権は移らず返却後は参照されない。
    let code = unsafe { fandhe_apple_photo_library_save(request_id, bytes.as_ptr(), bytes.len()) };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("photo_library present"))
    }
}

/// `.m` 側の完了から呼ばれる `extern "C"` 関数。コードの解釈は
/// `device/photo_save.rs::outcome_from_apple_code`。任意スレッドから呼ばれてよい
/// （`bridge` は `Mutex`）。画面への反映は毎フレームの `drive_callback` が行う。
///
/// iOS では OS コールバックが egui の `update` 外から呼ぶため、結果を置いた後に
/// `ios::request_repaint` で次フレームを起こす（契約表 4）。macOS は event loop が別途
/// repaint を要求するため呼ばない。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_photo_library_outcome(request_id: u64, code: i32) {
    super::photo_save::bridge::deliver(
        request_id,
        super::photo_save::outcome_from_apple_code(code),
    );
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
