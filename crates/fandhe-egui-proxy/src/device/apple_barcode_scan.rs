//! macOS / iOS 向けバーコード・QR 読み取り実装（Issue #557 macOS、iOS は #558。iOS は
//! `ios_barcode_scan` が使う）。`device/barcode_scan.rs` の macOS 経路から呼ばれ、
//! `apple/fandhe_apple_barcode_scan.m`（Vision `VNDetectBarcodesRequest`。`build.rs` が feature
//! `device` 有効時にコンパイル）を `unsafe extern "C"` 1 関数で呼ぶ。中身は `.m` からコールバックで
//! 1 件ずつ Rust の `Vec<String>` へ積み、所有権とアロケータは境界を越えない。`unsafe` はこの呼び出しと
//! コールバックの 2 箇所に閉じる（coding-rust.md の FFI 例外規定準拠）。`build.rs` の
//! コンパイル条件・`device.rs` の mod cfg と同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::barcode_scan`: 得た中身は必ず `normalize_payloads` を通し、
//! 復号・OS 呼び出しの失敗は `Internal`（0 件の成功へ畳まない）。呼び出しはブロックするため、
//! 呼び出し元はワーカーで呼ぶ。

use std::ffi::c_void;

use fandhe_device::barcode_scan::{normalize_payloads, BarcodeReader};
use fandhe_device::photo_picker::PickedPhoto;
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_barcode_scan.m` が定義する C ABI（`build.rs` が macOS / iOS
// ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_barcode_scan_read(
        bytes: *const u8,
        len: usize,
        ctx: *mut c_void,
        on_payload: extern "C" fn(*mut c_void, *const u8, usize),
    ) -> i32;
}

/// `.m` から同期に呼ばれ、中身を `ctx`（`&mut Vec<String>`）へ積む。panic しない処理だけにする
/// （unwind を FFI 越しに伝えない）。
#[allow(unsafe_code)]
extern "C" fn push_payload(ctx: *mut c_void, payload: *const u8, len: usize) {
    // SAFETY: `ctx` は `read` が渡した `&mut Vec<String>` で、`.m` は呼び出しの間だけ
    // 同期に使い保持しない。`payload` は `len` バイトの UTF-8 で、このコールバックの間だけ有効
    // （NUL を含みうるため NUL 終端には頼らない）。`len == 0` のとき `payload` は NULL でありうるので
    // スライスを作らない。
    let out = unsafe { &mut *(ctx as *mut Vec<String>) };
    let bytes: &[u8] = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(payload, len) }
    };
    out.push(String::from_utf8_lossy(bytes).into_owned());
}

/// Vision の `VNDetectBarcodesRequest`（全形式）で読む実装。
pub(crate) struct AppleBarcodeReader;

impl BarcodeReader for AppleBarcodeReader {
    #[allow(unsafe_code)]
    fn read(&self, photo: &PickedPhoto) -> Result<Vec<String>, DeviceError> {
        let mut raw: Vec<String> = Vec::new();
        // SAFETY: `photo.bytes` は呼び出しの間生存する読み取り専用の領域で、`.m` はコピーせず
        // 呼び出しの間だけ参照する。`raw` への可変参照はこの呼び出しの間だけ `ctx` として渡し、
        // `.m` 側は保持しない。
        let code = unsafe {
            fandhe_apple_barcode_scan_read(
                photo.bytes.as_ptr(),
                photo.bytes.len(),
                &mut raw as *mut Vec<String> as *mut c_void,
                push_payload,
            )
        };
        match code {
            0 => Ok(normalize_payloads(raw)),
            _ => Err(DeviceError::Internal("barcode_scan")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn push_payload_keeps_embedded_nul() {
        let mut out: Vec<String> = Vec::new();
        let ctx = &mut out as *mut Vec<String> as *mut c_void;
        let a = b"a\0b";
        push_payload(ctx, a.as_ptr(), a.len());
        push_payload(ctx, std::ptr::null(), 0);
        assert_eq!(out, vec!["a\0b".to_string(), String::new()]);
    }
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
            AppleBarcodeReader.read(&photo).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn corrupt_image_is_failure_not_empty_success() {
        // PNG の先頭マジックだけ正しく、以降は壊れている。
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0xFF; 32]);
        let photo = validate_photo(bytes).unwrap();
        let err = AppleBarcodeReader.read(&photo).unwrap_err();
        assert!(matches!(err, DeviceError::Internal(_)), "{err:?}");
    }
}
