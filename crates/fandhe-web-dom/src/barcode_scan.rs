//! 型サンプル「バーコード・QR 読み取り」の Web 経路（Issue #559、親 #116）。
//!
//! `fandhe_core::screens::barcode_scan`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、写真選択
//! （[`crate::file_picker::drive`] と共用）で得た画像 1 枚を検査 → `createImageBitmap` →
//! `BarcodeDetector.detect()` で読み、中身の文字列一覧を `finish_scan` へ書き戻す。
//! Web は `fandhe-device` の `BarcodeReader` トレイトを通らない（`file_picker.rs` と同じ分担）。
//! 検査の SSOT は `fandhe_device::{photo_picker, barcode_scan}` を使い、複製しない。
//!
//! # 結果の写像
//!
//! - 1 件以上: 成功。0 件も成功（「コードは見つかりませんでした」）
//! - 写真選択のキャンセル: 失敗にしない（`Ok(None)`）
//! - `BarcodeDetector` が無い（写真選択を開く前に判定）／`getSupportedFormats()` が空・reject
//!   （選択後に判定。`await` でユーザー操作起点が切れるため選択後に回す）: `BrowserUnsupported`
//! - 上限超過（バイト数・辺・総画素数。復号前に検査）: `InvalidInput`
//! - その他（形式不明・復号不能・検出の reject）: 固定文言の失敗。例外の `message` は
//!   画面にも `console` にも出さない（security.md A09）
//!
//! 読んだ中身は信頼できない入力。画面には `set_text_content` で出すだけで、URL として開かず、
//! `console` にも出さず、trim もしない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（`finish_scan` の世代照合）、失敗を成功へ畳まないこと、
//! `ImageBitmap` の `close()` と `<input>` の解放（`file_picker::release_mount`）。
//! 守らない: ブラウザごとの検出結果・順序の違い、処理時間（期限なし）、実行中の検出の取り消し、
//! 複数タブ、`fandhe-egui-proxy` の wasm canvas 経路。
//! Safari・Firefox・Windows/Linux の Chrome は `BarcodeDetector` が無いか形式が空のため非対応表示。

use fandhe_core::screens::barcode_scan::ScanFailure;
use fandhe_device::barcode_scan::normalize_payloads;
use fandhe_device::photo_picker::{dimensions_within_limits, image_dimensions, PickedPhoto};
use fandhe_device::DeviceError;

/// 読み取り失敗の固定文言（`fandhe-egui-proxy` と同文言）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_FAILED: &str = "コードを読み取れませんでした";

/// `validate_photo` の失敗を画面の失敗種別へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn photo_failure(e: DeviceError) -> ScanFailure {
    match e {
        DeviceError::InvalidInput(_) => ScanFailure::InvalidInput,
        _ => ScanFailure::Failed(READ_FAILED.to_string()),
    }
}

/// 復号前にヘッダーの辺・画素数を検査する（小さな巨大 PNG 対策）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn precheck(photo: &PickedPhoto) -> Result<(), ScanFailure> {
    match image_dimensions(photo) {
        Some((w, h)) if dimensions_within_limits(w, h) => Ok(()),
        Some(_) => Err(ScanFailure::InvalidInput),
        None => Err(ScanFailure::Failed(READ_FAILED.to_string())),
    }
}

/// `getSupportedFormats()` の件数が 0 なら非対応。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn formats_supported(count: u32) -> bool {
    count > 0
}

/// 文字列にできない値（`None`）を除き、契約の形（空文字除去・trim しない）へ揃える。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn payloads_from(raw: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    normalize_payloads(raw.into_iter().flatten())
}

/// `drive` の完了結果を画面の `finish_scan` が受ける形へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_screen_result(
    r: Result<Option<Result<Vec<String>, ScanFailure>>, String>,
) -> Result<Option<Vec<String>>, ScanFailure> {
    match r {
        Ok(Some(inner)) => inner.map(Some),
        Ok(None) => Ok(None),
        // 文言はファイル選択用のため、固定文言へ揃える。
        Err(_) => Err(ScanFailure::Failed(READ_FAILED.to_string())),
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::barcode_scan_effect;

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::barcode_scan::{
        finish_scan, mark_scanning, take_request, BarcodeScanState, ScanFailure,
    };
    use fandhe_device::photo_picker::{validate_photo, MAX_PHOTO_BYTES};
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Blob, BlobPropertyBag, File, ImageBitmap};

    use super::{
        formats_supported, payloads_from, photo_failure, precheck, to_screen_result, READ_FAILED,
    };
    use crate::file_picker::{drive, read, ConvertFuture, WebPickHooks};
    use crate::mount::Applier;

    type ScanOutcome = Result<Vec<String>, ScanFailure>;

    fn failed() -> ScanFailure {
        ScanFailure::Failed(READ_FAILED.to_string())
    }

    /// `window.BarcodeDetector`（実在する関数のときだけ `Some`）。
    fn detector_ctor() -> Option<Function> {
        Reflect::get(&js_sys::global(), &JsValue::from_str("BarcodeDetector"))
            .ok()
            .and_then(|v| v.dyn_into::<Function>().ok())
    }

    /// `BarcodeDetector.getSupportedFormats()` の件数。reject・配列でない場合は 0。
    async fn supported_format_count(ctor: &Function) -> u32 {
        let Ok(f) = Reflect::get(ctor, &JsValue::from_str("getSupportedFormats"))
            .and_then(|v| v.dyn_into::<Function>())
        else {
            return 0;
        };
        let Ok(p) = f.call0(ctor) else { return 0 };
        let Ok(p) = p.dyn_into::<js_sys::Promise>() else {
            return 0;
        };
        match JsFuture::from(p).await {
            Ok(v) => v.dyn_into::<Array>().map(|a| a.length()).unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// 構築済みの `ImageBitmap` を検出器へ渡して `rawValue` 一覧を得る。
    async fn detect(ctor: &Function, bitmap: &ImageBitmap) -> ScanOutcome {
        let detector = Reflect::construct(ctor, &Array::new()).map_err(|_| failed())?;
        let f: Function = Reflect::get(&detector, &JsValue::from_str("detect"))
            .and_then(|v| v.dyn_into())
            .map_err(|_| failed())?;
        let promise: js_sys::Promise = f
            .call1(&detector, bitmap)
            .and_then(|v| v.dyn_into())
            .map_err(|_| failed())?;
        let found: Array = JsFuture::from(promise)
            .await
            .map_err(|_| failed())?
            .dyn_into()
            .map_err(|_| failed())?;
        let raw = found.iter().map(|item| {
            Reflect::get(&item, &JsValue::from_str("rawValue"))
                .ok()
                .and_then(|v| v.as_string())
        });
        Ok(payloads_from(raw))
    }

    /// 写真選択の `convert`。バイト列は関数終了で drop し、画面状態には中身の文字列だけ残す。
    async fn scan(file: File) -> Result<ScanOutcome, String> {
        if file.size() as usize > MAX_PHOTO_BYTES {
            return Ok(Err(ScanFailure::InvalidInput));
        }
        let picked = read(file).await?;
        let bytes = js_sys::Uint8Array::new(&picked.buffer).to_vec();
        let photo = match validate_photo(bytes) {
            Ok(p) => p,
            Err(e) => return Ok(Err(photo_failure(e))),
        };
        if let Err(f) = precheck(&photo) {
            return Ok(Err(f));
        }

        let Some(ctor) = detector_ctor() else {
            return Ok(Err(ScanFailure::BrowserUnsupported));
        };
        if !formats_supported(supported_format_count(&ctor).await) {
            return Ok(Err(ScanFailure::BrowserUnsupported));
        }

        // `file.type` は信用せず、判定した形式の MIME で Blob を作る（A03）。
        let opts = BlobPropertyBag::new();
        opts.set_type(photo.format.mime());
        let parts = Array::of1(&picked.buffer);
        let Ok(blob) = Blob::new_with_buffer_source_sequence_and_options(&parts, &opts) else {
            return Ok(Err(failed()));
        };
        let Some(window) = web_sys::window() else {
            return Ok(Err(failed()));
        };
        let Ok(promise) = window.create_image_bitmap_with_blob(&blob) else {
            return Ok(Err(failed()));
        };
        let bitmap: ImageBitmap = match JsFuture::from(promise).await {
            Ok(v) => match v.dyn_into() {
                Ok(b) => b,
                Err(_) => return Ok(Err(failed())),
            },
            Err(_) => return Ok(Err(failed())),
        };

        let result = detect(&ctor, &bitmap).await;
        bitmap.close();
        Ok(result)
    }

    fn scan_boxed(_mount_id: u64, file: File) -> ConvertFuture<ScanOutcome> {
        Box::pin(scan(file))
    }

    fn finish(
        state: &mut BarcodeScanState,
        generation: u64,
        r: Result<Option<ScanOutcome>, String>,
    ) {
        finish_scan(state, generation, to_screen_result(r));
    }

    const BARCODE_HOOKS: WebPickHooks<BarcodeScanState, ScanOutcome> = WebPickHooks {
        accept: Some("image/*"),
        take_request,
        mark_picking: mark_scanning,
        convert: scan_boxed,
        finish,
    };

    /// `mount_with_effect` へ渡す effect。`BarcodeDetector` が無ければ写真選択を開かず
    /// 非対応で完了する（選ばせてから「非対応」と言わない）。
    pub fn barcode_scan_effect(state: &BarcodeScanState, applier: &Applier<BarcodeScanState>) {
        let Some(generation) = take_request(state) else {
            return;
        };
        if detector_ctor().is_none() {
            applier.apply(mark_scanning);
            applier.apply(move |s| {
                finish_scan(s, generation, Err(ScanFailure::BrowserUnsupported));
            });
            return;
        }
        drive(state, applier, BARCODE_HOOKS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::barcode_scan::{
        finish_scan, mark_scanning, request_scan, BarcodeScanState, BROWSER_UNSUPPORTED_STATUS,
    };
    use fandhe_device::photo_picker::validate_photo;

    fn png_with_size(w: u32, h: u32) -> PickedPhoto {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        validate_photo(b).expect("PNG と判定できる")
    }

    #[test]
    fn photo_failure_maps_kinds() {
        assert_eq!(
            photo_failure(DeviceError::InvalidInput("x")),
            ScanFailure::InvalidInput
        );
        assert_eq!(
            photo_failure(DeviceError::Internal("secret")),
            ScanFailure::Failed(READ_FAILED.to_string())
        );
    }

    #[test]
    fn precheck_checks_header() {
        assert!(precheck(&png_with_size(10, 10)).is_ok());
        let big = png_with_size(1, fandhe_device::photo_picker::MAX_PHOTO_SIDE + 1);
        assert_eq!(precheck(&big), Err(ScanFailure::InvalidInput));
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        b.extend_from_slice(&[0; 4]);
        let no_ihdr = validate_photo(b).expect("形式は判定できる");
        assert!(matches!(precheck(&no_ihdr), Err(ScanFailure::Failed(_))));
    }

    #[test]
    fn formats_and_payloads() {
        assert!(!formats_supported(0));
        assert!(formats_supported(1));
        let v = payloads_from([
            Some(" a ".to_string()),
            None,
            Some(String::new()),
            Some(" a ".to_string()),
        ]);
        assert_eq!(v, vec![" a ".to_string(), " a ".to_string()]);
    }

    #[test]
    fn screen_result_mapping() {
        assert_eq!(to_screen_result(Ok(None)), Ok(None));
        assert_eq!(
            to_screen_result(Ok(Some(Ok(vec!["x".into()])))),
            Ok(Some(vec!["x".to_string()]))
        );
        assert_eq!(
            to_screen_result(Ok(Some(Err(ScanFailure::InvalidInput)))),
            Err(ScanFailure::InvalidInput)
        );
        assert_eq!(
            to_screen_result(Err("ファイル名".into())),
            Err(ScanFailure::Failed(READ_FAILED.to_string()))
        );
    }

    #[test]
    fn browser_unsupported_reaches_screen() {
        let mut s = BarcodeScanState::default();
        request_scan(&mut s);
        mark_scanning(&mut s);
        let g = s.generation;
        finish_scan(
            &mut s,
            g,
            to_screen_result(Ok(Some(Err(ScanFailure::BrowserUnsupported)))),
        );
        assert_eq!(s.error, BROWSER_UNSUPPORTED_STATUS);
    }
}
