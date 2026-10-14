//! macOS / iOS 向け文字認識実装（Issue #1250 macOS、iOS は #1252。iOS は `ios_image_recognition::deliver` が使う）。
//! `device/image_recognition.rs` の macOS 経路から呼ばれ、`apple/fandhe_apple_image_recognition.m`
//! （Vision `VNRecognizeTextRequest`。`build.rs` が feature `device` 有効時にコンパイル）を
//! `unsafe extern "C"` 1 関数で呼ぶ。行は `.m` からコールバックで 1 件ずつ Rust の
//! `Vec<String>` へ積み、所有権とアロケータは境界を越えない。`unsafe` はこの呼び出しと
//! コールバックの 2 箇所に閉じる（coding-rust.md の FFI 例外規定準拠）。`build.rs` の
//! コンパイル条件・`device.rs` の mod cfg と同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::image_recognition`: 得た行は必ず `normalize_lines` を通し、
//! 日本語が使えない OS は `Unsupported(LANGUAGE_UNAVAILABLE)`、復号・OS 呼び出しの失敗は
//! `Internal`（0 行の成功へ畳まない）。呼び出しはブロックするため、呼び出し元はワーカーで呼ぶ。

use std::ffi::{c_char, c_void, CStr};

use fandhe_device::image_recognition::{normalize_lines, TextRecognizer, LANGUAGE_UNAVAILABLE};
use fandhe_device::photo_picker::PickedPhoto;
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_image_recognition.m` が定義する C ABI（`build.rs` が macOS / iOS
// ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_image_recognition_recognize(
        bytes: *const u8,
        len: usize,
        ctx: *mut c_void,
        on_line: extern "C" fn(*mut c_void, *const c_char),
    ) -> i32;
}

/// `.m` から同期に呼ばれ、行を `ctx`（`&mut Vec<String>`）へ積む。panic しない処理だけにする
/// （unwind を FFI 越しに伝えない）。
#[allow(unsafe_code)]
extern "C" fn push_line(ctx: *mut c_void, line: *const c_char) {
    // SAFETY: `ctx` は `recognize` が渡した `&mut Vec<String>` で、`.m` は呼び出しの間だけ
    // 同期に使い保持しない。`line` は NUL 終端の UTF-8 で、このコールバックの間だけ有効。
    let (lines, line) = unsafe { (&mut *(ctx as *mut Vec<String>), CStr::from_ptr(line)) };
    lines.push(line.to_string_lossy().into_owned());
}

/// Vision の `VNRecognizeTextRequest`（Accurate・日本語 + 英語）で認識する実装。
pub(crate) struct AppleTextRecognizer;

impl TextRecognizer for AppleTextRecognizer {
    #[allow(unsafe_code)]
    fn recognize(&self, photo: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
        let mut raw: Vec<String> = Vec::new();
        // SAFETY: `photo.bytes` は呼び出しの間生存する読み取り専用の領域で、`.m` はコピーせず
        // 呼び出しの間だけ参照する。`raw` への可変参照はこの呼び出しの間だけ `ctx` として渡し、
        // `.m` 側は保持しない。
        let code = unsafe {
            fandhe_apple_image_recognition_recognize(
                photo.bytes.as_ptr(),
                photo.bytes.len(),
                &mut raw as *mut Vec<String> as *mut c_void,
                push_line,
            )
        };
        match code {
            0 => Ok(normalize_lines(raw)),
            2 => Err(DeviceError::Unsupported(LANGUAGE_UNAVAILABLE)),
            _ => Err(DeviceError::Internal("image_recognition")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use fandhe_device::photo_picker::validate_photo;

    fn png_bytes(img: image::RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("PNG を書ける");
        out
    }

    #[test]
    fn blank_image_is_empty_success() {
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
        let photo = validate_photo(png_bytes(img)).unwrap();
        assert_eq!(
            AppleTextRecognizer.recognize(&photo).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn corrupt_image_is_failure_not_empty_success() {
        // PNG の先頭マジックだけ正しく、以降は壊れている。
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0xFF; 32]);
        let photo = validate_photo(bytes).unwrap();
        let err = AppleTextRecognizer.recognize(&photo).unwrap_err();
        assert!(matches!(err, DeviceError::Internal(_)), "{err:?}");
    }
}
