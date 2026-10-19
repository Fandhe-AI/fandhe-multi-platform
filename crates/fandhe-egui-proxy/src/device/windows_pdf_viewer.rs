//! Windows 向け PDF 表示（Issue #470。親 #107）。WinRT `Windows.Data.Pdf` で 1 ページ目を描く。
//!
//! 呼び出し元: `device/pdf_viewer.rs` の Windows 経路が `implementation` を呼び、ワーカー
//! スレッドから `render_first_page` を同期に呼ぶ。期限・世代・`check_page` の再検査は呼び出し側が
//! 持ち、本ファイルは「ブロックしてよい同期関数」だけを持つ。契約の SSOT は
//! `fandhe_device::pdf_viewer`。
//!
//! 失敗の写像（全呼び出し箇所で一貫。`map_load_err` / `map_render_err`）:
//! - アクティベーション失敗（`REGDB_E_CLASSNOTREG` / `E_NOINTERFACE` / package identity 無し）
//!   → `Unsupported("pdf_viewer")`。ダミーの成功へは倒さない
//! - 読み込み段の失敗: パスワード要求 → `ENCRYPTED`、それ以外 → `BROKEN`
//! - 描画・復号段の失敗 → 固定ラベル `Internal`
//!
//! HRESULT・OS のエラー文・PDF・画素はログにも画面にも出さない（security.md A09）。
//! `RoInitialize` は呼ばず（MTA フォールバック）、`unsafe` は使わない。`.get()` に期限は設けない
//! （期限はドライバだけが持ち、OS 側は走り切らせる）。
//!
//! 既知の天井: Microsoft の「APIs that require package identity」に `PdfDocument` 等が載っている。
//! unpackaged の exe で動くかは未確認で実機確認待ち（#468）。型検査は `x86_64-pc-windows-gnu` のみ。

use std::sync::Arc;

use fandhe_device::pdf_viewer::{
    check_page, render_size, validate_input, PdfFirstPage, PdfPageRenderer, BROKEN, EMPTY,
    ENCRYPTED, MAX_SIDE, SIZE,
};
use fandhe_device::DeviceError;
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
    ExifOrientationMode,
};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
use windows::UI::Color;

const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
/// `HRESULT_FROM_WIN32(APPMODEL_ERROR_NO_PACKAGE)`。
const NO_PACKAGE: i32 = 0x8007_3D54_u32 as i32;
/// `HRESULT_FROM_WIN32(ERROR_WRONG_PASSWORD)`。
const WRONG_PASSWORD: i32 = 0x8007_052B_u32 as i32;

/// Windows の `PdfPageRenderer`（`Windows.Data.Pdf`）。
pub(crate) struct WindowsPdfPageRenderer;

/// Windows の `PdfPageRenderer` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn PdfPageRenderer> {
    Arc::new(WindowsPdfPageRenderer)
}

fn is_unsupported(code: i32) -> bool {
    matches!(code, REGDB_E_CLASSNOTREG | E_NOINTERFACE | NO_PACKAGE)
}

/// 読み込み段（`LoadFromStreamAsync`）の HRESULT を契約上のエラーへ写す。
fn map_load_err(code: i32) -> DeviceError {
    match code {
        c if is_unsupported(c) => DeviceError::Unsupported("pdf_viewer"),
        WRONG_PASSWORD => DeviceError::Internal(ENCRYPTED),
        _ => DeviceError::Internal(BROKEN),
    }
}

/// 描画・復号段の HRESULT を契約上のエラーへ写す。
fn map_render_err(code: i32) -> DeviceError {
    if is_unsupported(code) {
        DeviceError::Unsupported("pdf_viewer")
    } else {
        DeviceError::Internal("pdf_viewer render")
    }
}

fn stream_err(_: windows::core::Error) -> DeviceError {
    DeviceError::Internal("pdf_viewer stream")
}

fn render(pdf: &[u8], max_side: u32) -> Result<PdfFirstPage, DeviceError> {
    let rerr = |e: windows::core::Error| map_render_err(e.code().0);

    let stream = InMemoryRandomAccessStream::new().map_err(stream_err)?;
    let writer = DataWriter::CreateDataWriter(&stream).map_err(stream_err)?;
    writer.WriteBytes(pdf).map_err(stream_err)?;
    writer
        .StoreAsync()
        .map_err(stream_err)?
        .get()
        .map_err(stream_err)?;
    // drop 時に stream を閉じさせない。
    writer.DetachStream().map_err(stream_err)?;
    stream.Seek(0).map_err(stream_err)?;

    let load = |e: windows::core::Error| map_load_err(e.code().0);
    let doc = PdfDocument::LoadFromStreamAsync(&stream)
        .map_err(load)?
        .get()
        .map_err(load)?;
    let page_count = doc.PageCount().map_err(rerr)?;
    if page_count == 0 {
        return Err(DeviceError::Internal(EMPTY));
    }
    let page = doc.GetPage(0).map_err(rerr)?;
    let size = page.Size().map_err(rerr)?;
    let (mut w, mut h) = render_size(f64::from(size.Width), f64::from(size.Height))
        .ok_or(DeviceError::Internal(SIZE))?;
    if max_side < MAX_SIDE {
        // ドライバは常に MAX_SIDE を渡すので通常は通らない。長辺を比例で縮める。
        let f = |v: u32| (u64::from(v) * u64::from(max_side) / u64::from(MAX_SIDE)).max(1) as u32;
        (w, h) = (f(w), f(h));
    }

    let opts = PdfPageRenderOptions::new().map_err(rerr)?;
    opts.SetDestinationWidth(w).map_err(rerr)?;
    opts.SetDestinationHeight(h).map_err(rerr)?;
    opts.SetBackgroundColor(Color {
        A: 255,
        R: 255,
        G: 255,
        B: 255,
    })
    .map_err(rerr)?;
    opts.SetIsIgnoringHighContrast(true).map_err(rerr)?;
    let out = InMemoryRandomAccessStream::new().map_err(stream_err)?;
    page.RenderWithOptionsToStreamAsync(&out, &opts)
        .map_err(rerr)?
        .get()
        .map_err(rerr)?;
    out.Seek(0).map_err(stream_err)?;

    let decoder = BitmapDecoder::CreateAsync(&out)
        .map_err(rerr)?
        .get()
        .map_err(rerr)?;
    let (mut dw, mut dh) = (
        decoder.PixelWidth().map_err(rerr)?,
        decoder.PixelHeight().map_err(rerr)?,
    );
    let transform = BitmapTransform::new().map_err(rerr)?;
    let longest = dw.max(dh);
    // DPI により実寸が要求より大きくなる場合は、縦横比を保って縮める。
    if longest > max_side {
        let s = |v: u32| (u64::from(v) * u64::from(max_side) / u64::from(longest)).max(1) as u32;
        (dw, dh) = (s(dw), s(dh));
        transform.SetScaledWidth(dw).map_err(rerr)?;
        transform.SetScaledHeight(dh).map_err(rerr)?;
    }
    let pixels = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Rgba8,
            BitmapAlphaMode::Straight,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .map_err(rerr)?
        .get()
        .map_err(rerr)?;
    let rgba = pixels.DetachPixelData().map_err(rerr)?.to_vec();
    check_page(page_count, dw, dh, rgba)
}

impl PdfPageRenderer for WindowsPdfPageRenderer {
    fn render_first_page(&self, pdf: &[u8], max_side: u32) -> Result<PdfFirstPage, DeviceError> {
        validate_input(pdf)?;
        if max_side == 0 || max_side > MAX_SIDE {
            return Err(DeviceError::InvalidInput("pdf_viewer input"));
        }
        render(pdf, max_side)
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use fandhe_device::pdf_viewer::{empty_page_pdf, sample_pdf, truncated_pdf};

    #[test]
    fn error_mapping_table() {
        for c in [REGDB_E_CLASSNOTREG, E_NOINTERFACE, NO_PACKAGE] {
            assert!(matches!(map_load_err(c), DeviceError::Unsupported(_)));
            assert!(matches!(map_render_err(c), DeviceError::Unsupported(_)));
        }
        assert!(matches!(
            map_load_err(WRONG_PASSWORD),
            DeviceError::Internal(ENCRYPTED)
        ));
        assert!(matches!(
            map_load_err(0x8003_001E_u32 as i32),
            DeviceError::Internal(BROKEN)
        ));
        assert!(matches!(map_load_err(-1), DeviceError::Internal(BROKEN)));
    }

    #[test]
    fn renders_sample_or_unsupported() {
        match WindowsPdfPageRenderer.render_first_page(&sample_pdf(), MAX_SIDE) {
            Ok(p) => {
                assert_eq!(p.page_count, 3);
                assert!(p.image.width.max(p.image.height) <= MAX_SIDE);
            }
            // package identity が無い環境
            Err(DeviceError::Unsupported(_)) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn bad_inputs_never_succeed() {
        let r = |b: &[u8], m| WindowsPdfPageRenderer.render_first_page(b, m);
        assert!(r(&truncated_pdf(), MAX_SIDE).is_err());
        assert!(r(&empty_page_pdf(), MAX_SIDE).is_err());
        assert!(r(&[], MAX_SIDE).is_err());
        assert!(r(&sample_pdf(), MAX_SIDE + 1).is_err());
    }
}
