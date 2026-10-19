//! macOS / iOS 向け PDF 表示実装（Issue #469 macOS、iOS は #471 で共用）。
//! `device/pdf_viewer.rs` の macOS 経路（と `ios_pdf_viewer`）から呼ばれ、CoreGraphics 実装
//! （`apple/fandhe_apple_pdf_viewer.m`、`build.rs` が feature `device` 有効時のみコンパイル・
//! リンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこのモジュールの extern 宣言と
//! 呼び出し 1 か所に閉じ `// SAFETY:` で契約を明記する（coding-rust.md の FFI 例外規定準拠）。
//!
//! 結果コード → 結果の写像は `fandhe_device::pdf_viewer::decode_code` に一本化する。出力バッファは
//! Rust が確保して容量を渡し、戻り値の寸法は [`check_page`] で再検査する（境界を越えて
//! malloc / free を渡し合わない）。PDF・画素・OS のエラーはログへ出さない（security.md A09）。
//!
//! 同期呼び出しで、`.m` は UI も main スレッドも要らない。呼び出し側（ワーカー）から直接呼ぶ。

use fandhe_device::pdf_viewer::{
    check_page, decode_code, validate_input, PdfFirstPage, PdfPageRenderer, MAX_SIDE,
};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_pdf_viewer.m` が定義する C ABI。契約は `.m` 冒頭コメントと
// 下記 `render_first_page` の SAFETY コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_pdf_render_first_page(
        pdf: *const u8,
        len: usize,
        max_side: u32,
        out: *mut u8,
        out_cap: usize,
        w: *mut u32,
        h: *mut u32,
        pages: *mut u32,
    ) -> i32;
}

/// CoreGraphics による [`PdfPageRenderer`]。
pub struct ApplePdfPageRenderer;

impl PdfPageRenderer for ApplePdfPageRenderer {
    #[allow(unsafe_code)]
    fn render_first_page(&self, pdf: &[u8], max_side: u32) -> Result<PdfFirstPage, DeviceError> {
        validate_input(pdf)?;
        if max_side == 0 || max_side > MAX_SIDE {
            return Err(DeviceError::InvalidInput("pdf_viewer input"));
        }
        let side = max_side as usize;
        let cap = side
            .checked_mul(side)
            .and_then(|n| n.checked_mul(4))
            .ok_or(DeviceError::InvalidInput("pdf_viewer input"))?;
        let mut buf = vec![0u8; cap];
        let (mut w, mut h, mut pages) = (0u32, 0u32, 0u32);
        // SAFETY: `pdf` は有効なスライスで、ポインタと長さを一緒に渡す。`.m` は呼び出しの間だけ
        // 読み、保持しない。`buf` は `cap` バイトの確保済み領域で、`.m` は `w*h*4 <= out_cap` を
        // 確かめてから書く。`w` / `h` / `pages` は呼び出しの間だけ有効なローカルへの書き込み先。
        let code = unsafe {
            fandhe_apple_pdf_render_first_page(
                pdf.as_ptr(),
                pdf.len(),
                max_side,
                buf.as_mut_ptr(),
                buf.len(),
                &mut w,
                &mut h,
                &mut pages,
            )
        };
        decode_code(code)?;
        let used = (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= buf.len())
            .ok_or(DeviceError::Internal(fandhe_device::pdf_viewer::SIZE))?;
        buf.truncate(used);
        check_page(pages, w, h, buf)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use fandhe_device::pdf_viewer::{empty_page_pdf, sample_pdf, truncated_pdf, BROKEN, EMPTY};

    // 実 CGPDF を呼ぶホストテスト。cargo test の既定どおり main 以外のスレッドで走る
    // （`.m` は main を要求しない）。

    #[test]
    fn renders_first_page_of_sample() {
        let p = ApplePdfPageRenderer
            .render_first_page(&sample_pdf(), MAX_SIDE)
            .expect("サンプルは描ける");
        assert_eq!(p.page_count, 3);
        assert_eq!((p.image.width, p.image.height), (724, 1024));
        // 中央付近は 1 ページ目の矩形（赤系）で、白ではない。
        let (cx, cy) = (p.image.width as usize / 2, p.image.height as usize / 2);
        let i = (cy * p.image.width as usize + cx) * 4;
        assert_ne!(&p.image.rgba[i..i + 3], &[255, 255, 255]);
        assert_eq!(p.image.rgba[i + 3], 255, "不透明");
        // 左上の余白は白。
        assert_eq!(&p.image.rgba[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn broken_and_empty_inputs_never_succeed() {
        let r = ApplePdfPageRenderer.render_first_page(&truncated_pdf(), MAX_SIDE);
        assert!(matches!(r, Err(DeviceError::Internal(BROKEN))), "{r:?}");
        let r = ApplePdfPageRenderer.render_first_page(&empty_page_pdf(), MAX_SIDE);
        assert!(
            matches!(r, Err(DeviceError::Internal(EMPTY | BROKEN))),
            "{r:?}"
        );
        let r = ApplePdfPageRenderer.render_first_page(&[], MAX_SIDE);
        assert!(matches!(r, Err(DeviceError::InvalidInput(_))));
        let r = ApplePdfPageRenderer.render_first_page(&sample_pdf(), MAX_SIDE + 1);
        assert!(matches!(r, Err(DeviceError::InvalidInput(_))));
    }
}
