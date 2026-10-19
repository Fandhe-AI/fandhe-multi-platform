//! PDF 表示の抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/pdf-viewer.json`、
//! 親 #107、Issue #469）。
//!
//! 扱う範囲は「アプリに埋め込んだサンプル PDF（[`sample_pdf`]、3 ページ）の 1 ページ目を
//! 長辺 [`MAX_SIDE`] px 以内の RGBA に描き、総ページ数と一緒に返す」ことだけ。利用者が持ち込む
//! ファイル・ページ送り・注釈・検索・選択は扱わない。対象は macOS / iOS（CoreGraphics
//! `CGPDFDocument`）、Windows（`Windows.Data.Pdf`）、Android（`PdfRenderer`）。Web は対象外。
//! 一発呼び出し型の同期 API で、呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は
//! UI を止めないようワーカースレッドから呼ぶ。
//!
//! # 契約
//!
//! - **成功**: [`check_page`] を通ったものだけ。ページ数が 1 以上・寸法が `1..=`[`MAX_SIDE`]・
//!   `rgba.len() == width * height * 4`、をすべて満たす（契約表 6「嘘の成功」）
//! - **失敗の区別**: 非対応 → `Err(Unsupported("pdf_viewer"))`、壊れている → `Internal(`[`BROKEN`]`)`、
//!   暗号化（開けない）→ `Internal(`[`ENCRYPTED`]`)`、0 ページ → `Internal(`[`EMPTY`]`)`、
//!   寸法不正 → `Internal(`[`SIZE`]`)`、期限切れ → `Internal(`[`TIMEOUT`]`)`、
//!   入力が空・上限超え → `InvalidInput`、それ以外 → `Internal`
//! - **結果の三値**（契約表 10）: 成功 / 失敗の 2 値。**取り消しの操作と OS の権限は存在しない**
//!   （メモリ上のバイト列だけを読む）ため、キャンセル・権限拒否の失敗系は無い。短い処理の
//!   正常終了を失敗として見せない
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`PDF_DEADLINE`]）。起点は要求を
//!   観測した時の単調時計 `Instant`。OS 側の描画は中断できないので走り切らせ、期限後に届いた
//!   結果はドライバが捨てる
//! - **並行性**（契約表 2）: 世代は画面側が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ
//!   適用する。処理中の再押下は無視する
//! - **ライフサイクル**（契約表 1）: 短い処理なので、背景遷移では中断しない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、OS 資源の解放。
//! 守らない範囲: 任意の PDF・`/Rotate`・ボックスの違い、暗号化 PDF の実 API での確認、期限の後も
//! OS 側の描画が走り切ること、Windows の HRESULT の写像の正確さと unpackaged で動くか、
//! 色の正確さと所要時間。PDF の中身・画素・OS のエラー文はログへ出さない。

use std::time::Duration;

use crate::{DeviceError, Image};

/// 出力画像の長辺の上限（ピクセル）。`fandhe_core::RGBA_MAX_SIDE` 以下に保つ。
pub const MAX_SIDE: u32 = 1024;
/// 入力 PDF の上限（バイト）。FFI へ渡す前に検査する。
pub const MAX_PDF_BYTES: usize = 1 << 20;
/// 要求を観測してから結果を確定するまでの期限（Rust のドライバだけが持つ）。
pub const PDF_DEADLINE: Duration = Duration::from_secs(10);

/// PDF として読めないときの `Internal` の識別子（SSOT）。
pub const BROKEN: &str = "pdf_viewer broken";
/// 暗号化されていて開けないときの `Internal` の識別子（SSOT）。
pub const ENCRYPTED: &str = "pdf_viewer encrypted";
/// ページが 0 のときの `Internal` の識別子（SSOT）。
pub const EMPTY: &str = "pdf_viewer empty";
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "pdf_viewer timeout";
/// 出力の寸法・長さが不正なときの `Internal` の識別子（SSOT）。
pub const SIZE: &str = "pdf_viewer size";
/// 未知の結果コードのときの `Internal` の識別子（SSOT）。
pub const OUTCOME: &str = "pdf_viewer outcome";

/// 1 ページ目の画像と総ページ数。
#[derive(Clone)]
pub struct PdfFirstPage {
    /// 総ページ数（1 以上）。
    pub page_count: u32,
    /// 1 ページ目の RGBA 画像。
    pub image: Image,
}

impl std::fmt::Debug for PdfFirstPage {
    /// 画素を出さない（寸法とページ数だけ）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfFirstPage")
            .field("page_count", &self.page_count)
            .field("width", &self.image.width)
            .field("height", &self.image.height)
            .finish()
    }
}

/// PDF の 1 ページ目の描画契約（モジュール doc の「契約」節）。
pub trait PdfPageRenderer: Send + Sync {
    /// `pdf` の 1 ページ目を長辺 `max_side` 以内の RGBA に描いて返す。同期でブロックしてよい。
    fn render_first_page(&self, pdf: &[u8], max_side: u32) -> Result<PdfFirstPage, DeviceError>;
}

/// 対象外 OS（Linux 等）・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedPdfPageRenderer;

impl PdfPageRenderer for UnsupportedPdfPageRenderer {
    fn render_first_page(&self, _: &[u8], _: u32) -> Result<PdfFirstPage, DeviceError> {
        Err(DeviceError::Unsupported("pdf_viewer"))
    }
}

/// 入力を OS を呼ぶ前に検査する。空、または [`MAX_PDF_BYTES`] 超は `InvalidInput`。
pub fn validate_input(pdf: &[u8]) -> Result<(), DeviceError> {
    if pdf.is_empty() || pdf.len() > MAX_PDF_BYTES {
        Err(DeviceError::InvalidInput("pdf_viewer input"))
    } else {
        Ok(())
    }
}

/// Apple（`.m`）と Android が返す結果コードを写す。0 のみ成功。未知の値は失敗（嘘の成功の禁止）。
/// コード表の SSOT: 0 成功 / 1 非対応 / 2 壊れている / 3 暗号化 / 4 0 ページ / 5 寸法・引数の不正。
pub fn decode_code(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::Unsupported("pdf_viewer")),
        2 => Err(DeviceError::Internal(BROKEN)),
        3 => Err(DeviceError::Internal(ENCRYPTED)),
        4 => Err(DeviceError::Internal(EMPTY)),
        5 => Err(DeviceError::Internal(SIZE)),
        _ => Err(DeviceError::Internal(OUTCOME)),
    }
}

/// 成功を判定する唯一の関数。OS 実装とドライバの両方が通す。
pub fn check_page(
    page_count: u32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) -> Result<PdfFirstPage, DeviceError> {
    if page_count == 0 {
        return Err(DeviceError::Internal(EMPTY));
    }
    let in_range = |v: u32| (1..=MAX_SIDE).contains(&v);
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if !in_range(width) || !in_range(height) || expected != Some(rgba.len()) {
        return Err(DeviceError::Internal(SIZE));
    }
    Ok(PdfFirstPage {
        page_count,
        image: Image {
            width,
            height,
            rgba,
        },
    })
}

/// ページ寸法（pt）から、長辺を [`MAX_SIDE`] に合わせた出力寸法を求める。縦横比を保ち、
/// 短辺は四捨五入して最低 1。有限でない値・0 以下は `None`。`.m` の式と同じ。
pub fn render_size(w_pt: f64, h_pt: f64) -> Option<(u32, u32)> {
    if !(w_pt.is_finite() && h_pt.is_finite() && w_pt > 0.0 && h_pt > 0.0) {
        return None;
    }
    let scale = f64::from(MAX_SIDE) / w_pt.max(h_pt);
    let dim = |v: f64| ((v * scale).round() as u32).clamp(1, MAX_SIDE);
    Some((dim(w_pt), dim(h_pt)))
}

/// A4（595 x 842 pt）の最小の PDF を組み立てる。`pages` が 0 なら `/Count 0`。
/// xref のオフセットと `/Length` は組み立てながら数える。フォントは埋め込まない。
fn build_pdf(pages: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    fn obj(out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: String) {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    // 1 = Catalog, 2 = Pages, 3 = Font, 以降は (Page, Content) の組。
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 4 + i * 2)).collect();
    obj(
        &mut out,
        &mut offsets,
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
    );
    obj(
        &mut out,
        &mut offsets,
        format!("<< /Type /Pages /Kids [ {kids}] /Count {pages} >>"),
    );
    obj(
        &mut out,
        &mut offsets,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    );
    const COLORS: [&str; 3] = ["0.9 0.3 0.3", "0.3 0.7 0.4", "0.3 0.4 0.9"];
    for i in 0..pages {
        let content = format!(
            "{} rg 100 300 395 240 re f\nBT /F1 36 Tf 100 700 Td 0 g (Page {} of {}) Tj ET",
            COLORS[i % 3],
            i + 1,
            pages
        );
        obj(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents {} 0 R \
                 /Resources << /Font << /F1 3 0 R >> >> >>",
                5 + i * 2
            ),
        );
        obj(
            &mut out,
            &mut offsets,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        );
    }
    let xref_at = out.len();
    let count = offsets.len() + 1;
    out.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

/// アプリに埋め込むサンプル PDF（3 ページ・A4）。コードで生成し、ファイルは置かない。
pub fn sample_pdf() -> Vec<u8> {
    build_pdf(3)
}

/// `/Count 0` の PDF（0 ページの失敗系の確認用）。
#[doc(hidden)]
pub fn empty_page_pdf() -> Vec<u8> {
    build_pdf(0)
}

/// 先頭の `%PDF-` ヘッダを含む数十バイトを落とした PDF（壊れた入力の失敗系の確認用）。
#[doc(hidden)]
pub fn truncated_pdf() -> Vec<u8> {
    build_pdf(3).split_off(40)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        hay[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    }

    fn num(b: &[u8]) -> usize {
        std::str::from_utf8(b).unwrap().trim().parse().unwrap()
    }

    fn internal(r: Result<impl std::fmt::Debug, DeviceError>, id: &str) {
        match r {
            Err(DeviceError::Internal(got)) => assert_eq!(got, id),
            other => panic!("Internal({id}) のはず: {other:?}"),
        }
    }

    #[test]
    fn sample_pdf_xref_points_at_objects() {
        let pdf = sample_pdf();
        assert!(pdf.starts_with(b"%PDF-"));
        let sx = find(&pdf, b"startxref\n", 0).unwrap() + 10;
        let end = find(&pdf, b"\n", sx).unwrap();
        let xref_at = num(&pdf[sx..end]);
        assert!(pdf[xref_at..].starts_with(b"xref\n"));
        // 0 番の free エントリの後に 20 バイトずつ並ぶ
        let table = find(&pdf, b"65535 f \n", xref_at).unwrap() + 9;
        for n in 1..=(3 + 2 * 3) {
            let e = table + (n - 1) * 20;
            let off = num(&pdf[e..e + 10]);
            let head = format!("{n} 0 obj\n");
            assert!(pdf[off..].starts_with(head.as_bytes()), "obj {n}");
        }
        assert!(find(&pdf, b"/Count 3", 0).is_some());
    }

    #[test]
    fn stream_length_matches_body() {
        let s = String::from_utf8(sample_pdf()).unwrap();
        for part in s.split("/Length ").skip(1) {
            let len: usize = part.split(' ').next().unwrap().parse().unwrap();
            let body = part.split_once("\nstream\n").unwrap().1;
            let body = body.split("\nendstream").next().unwrap();
            assert_eq!(body.len(), len);
        }
    }

    #[test]
    fn broken_variants_differ_from_sample() {
        assert!(!truncated_pdf().starts_with(b"%PDF-"));
        assert!(find(&empty_page_pdf(), b"/Count 0", 0).is_some());
    }

    #[test]
    fn decode_code_table() {
        assert!(decode_code(0).is_ok());
        assert!(matches!(
            decode_code(1),
            Err(DeviceError::Unsupported("pdf_viewer"))
        ));
        for (c, id) in [
            (2, BROKEN),
            (3, ENCRYPTED),
            (4, EMPTY),
            (5, SIZE),
            (6, OUTCOME),
            (-1, OUTCOME),
        ] {
            internal(decode_code(c).map(|_| 0), id);
        }
    }

    #[test]
    fn check_page_boundaries() {
        let px = |w: u32, h: u32| vec![0u8; (w * h * 4) as usize];
        assert!(check_page(3, 2, 2, px(2, 2)).is_ok());
        internal(check_page(0, 2, 2, px(2, 2)), EMPTY);
        for (w, h) in [(0, 2), (2, 0), (MAX_SIDE + 1, 1), (1, MAX_SIDE + 1)] {
            internal(check_page(1, w, h, vec![]), SIZE);
        }
        assert!(check_page(1, MAX_SIDE, 1, px(MAX_SIDE, 1)).is_ok());
        internal(check_page(1, 2, 2, vec![0; 15]), SIZE);
        internal(check_page(1, 2, 2, vec![0; 17]), SIZE);
        internal(check_page(1, u32::MAX, u32::MAX, vec![0; 4]), SIZE);
    }

    #[test]
    fn render_size_keeps_aspect() {
        assert_eq!(render_size(595.0, 842.0), Some((724, 1024)));
        assert_eq!(render_size(842.0, 595.0), Some((1024, 724)));
        assert_eq!(render_size(1.0, 100_000.0), Some((1, 1024)));
        for (w, h) in [
            (f64::NAN, 1.0),
            (0.0, 1.0),
            (-1.0, 1.0),
            (f64::INFINITY, 1.0),
        ] {
            assert_eq!(render_size(w, h), None);
        }
    }

    #[test]
    fn validate_input_bounds() {
        assert!(validate_input(&[]).is_err());
        assert!(validate_input(&[0]).is_ok());
        assert!(validate_input(&vec![0; MAX_PDF_BYTES]).is_ok());
        assert!(validate_input(&vec![0; MAX_PDF_BYTES + 1]).is_err());
    }

    #[test]
    fn unsupported_never_succeeds() {
        let r = UnsupportedPdfPageRenderer.render_first_page(&sample_pdf(), MAX_SIDE);
        assert!(matches!(r, Err(DeviceError::Unsupported("pdf_viewer"))));
    }

    #[test]
    fn debug_hides_pixels() {
        let p = check_page(1, 1, 1, vec![9, 8, 7, 6]).unwrap();
        assert!(!format!("{p:?}").contains('9'));
    }
}
