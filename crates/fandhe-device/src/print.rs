//! 印刷（OS の印刷ダイアログを開く）機能の抽象（Phase 11 の拡充ループ、親 #94、Issue #325）。
//!
//! 範囲は次の 2 つ。どちらも 1 ページとして OS の印刷ダイアログに渡す。
//!
//! - 固定の短い文章（見出し 1 行 + 本文数行）
//! - 今の画面の描画を 1 枚の画像として、縦横比を保って印刷可能領域に収めたもの
//!   （対応表 `print-screen`。ダイアログの出し方・結果の三値・橋渡しは文章と共用する）
//!
//! 任意の画像ファイル・複数ページ・PDF・プリンタ列挙・進捗・紙に出たかの追跡は扱わない。
//! 検証の SSOT は [`PrintRequest::new`]（これ以外では要求を作れない）。画面
//! （`fandhe_core::screens::print_page`、端末依存ゼロ）へは `fandhe-egui-proxy` の effect
//! ドライバ（`device/print.rs`）が写像する。
//!
//! # 結果の種類（[`PrintOutcome`]）
//!
//! - `Submitted`: OS の印刷系へジョブを渡した（PDF 保存を含む）。**紙に出たことは意味しない**
//! - `Cancelled`: 利用者がダイアログで取り消した。失敗ではない
//! - `Unconfirmed`: ダイアログを開くよう依頼したが、結果を OS が返さない
//!
//! 失敗は `Err(DeviceError)`。
//!
//! | OS | `Submitted` | `Cancelled` | `Unconfirmed` |
//! |---|---|---|---|
//! | macOS | パネル OK かつ `runOperation` が YES | パネルが Cancel | 到達しない |
//! | Windows | `PrintDlgExW` が `PD_RESULT_PRINT` でジョブ投入 | `PD_RESULT_CANCEL` / `PD_RESULT_APPLY` | 到達しない |
//! | iOS | `completionHandler` が completed かつ error なし | completed NO かつ error なし | 到達しない |
//! | Android | `PrintJob` が QUEUED / STARTED / BLOCKED / COMPLETED | CANCELED | 2 秒後も CREATED のまま |
//! | Web | 到達しない | 到達しない | `window.print()` 呼び出し後は常に |
//!
//! 画面の画像の印刷も、各 OS の到達値は文章と同じ（Web は画面 DOM の複製を印刷用ブロックへ入れて
//! `window.print()` するので、常に `Unconfirmed`）。
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | 入力不正 | `InvalidInput("print title")` / `InvalidInput("print body")` |
//! | 画面を取り込めない（期限内に読み戻しが届かない・寸法不正・Web の複製の失敗）。ダイアログを出す前に確定する | `Internal("print capture")` |
//! | 非対応（Linux ほか） | `Unsupported("print")` |
//! | ダイアログを出せない（main thread 外・Activity 無しなど） | `Internal("print present")` |
//! | ジョブ投入・描画の失敗 | `Internal("print failed")` |
//! | OS の結果コードが未知 | `Internal("print outcome")` |
//!
//! 印刷に権限は要らない（権限拒否の経路は無い）。
//!
//! # 三値の報告先（契約表 10）
//!
//! 利用者の取り消しを失敗に、OS の失敗を取り消しに畳まない。ダイアログ提示前の失敗は
//! `print present`、提示後のジョブ投入・描画の失敗は `print failed` に分ける。macOS は
//! 取り消しを先に確定させる 2 段構成（パネル → パネルなしの実行）にして、実行の NO を
//! `Cancelled` に畳まず失敗として返す。
//!
//! # 期限・ライフサイクル
//!
//! 利用者操作のダイアログなので固定期限は掛けない。再確認が要る Android のみ、単調時計で
//! 最大 2 秒（`CREATED` のまま）。macOS・Windows・Web は同期モーダルで保留状態を持たない。
//! iOS は背景遷移で取り消さない。Android は Activity 再生成で保留を復元せず破棄する
//! （`bridge::reset`）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内での要求と完了の順序（世代の照合、保留中は新規要求を受けない）、
//! 入力不正を OS へ渡さないこと、嘘の成功の禁止（失敗・非対応・未知コードを `Submitted` に
//! 畳まない）、プリンタ DC・PDF 文書・ネイティブハンドルの解放。
//!
//! 守らない: 紙に出たか・ジョブ投入後のプリンタ側の失敗、Android の `onFinish` 後の状態変化、
//! Web の sandbox やブラウザ設定で黙って無視される場合、OS ごとの書体・余白の差、
//! 複数タブ・複数プロセスからの同時操作。見出し・本文・内部エラー種別はログへ出さない
//! （security.md A09）。

use crate::DeviceError;

const TITLE_MAX_CHARS: usize = 64;
const BODY_MAX_LINES: usize = 20;
const LINE_MAX_CHARS: usize = 80;
/// 画面の画像の一辺の上限（画素）。最悪でも 8192 x 8192 x 4 = 256MB で、読み戻しは
/// ウィンドウの大きさなので実際の値は小さい。超える寸法は取り込み失敗として扱う。
pub const IMAGE_MAX_SIDE: u32 = 8192;

/// 印刷の結果。`Submitted` でも紙に出たとは限らない（モジュール doc）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintOutcome {
    /// OS の印刷系へジョブを渡した（PDF 保存を含む）。
    Submitted,
    /// 利用者がダイアログで取り消した。
    Cancelled,
    /// ダイアログを開くよう依頼したが、結果を OS が返さない。
    Unconfirmed,
}

/// 検査済みの印刷要求（見出し 1 行 + 本文 1〜20 行）。[`PrintRequest::new`] でのみ作れる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintRequest {
    title: String,
    body: String,
}

impl PrintRequest {
    /// 見出しは 1〜64 文字で制御文字なし、本文は `\n` 区切りで 1〜20 行・各行 1〜80 文字で
    /// `\n` 以外の制御文字なし。違反は黙って切り詰めず `InvalidInput` で拒否する
    /// （末尾改行は空行になるので拒否）。
    pub fn new(title: &str, body: &str) -> Result<Self, DeviceError> {
        let title_ok = (1..=TITLE_MAX_CHARS).contains(&title.chars().count())
            && !title.chars().any(char::is_control);
        if !title_ok {
            return Err(DeviceError::InvalidInput("print title"));
        }
        let lines: Vec<&str> = body.split('\n').collect();
        let body_ok = lines.len() <= BODY_MAX_LINES
            && lines.iter().all(|l| {
                (1..=LINE_MAX_CHARS).contains(&l.chars().count())
                    && !l.chars().any(char::is_control)
            });
        if !body_ok {
            return Err(DeviceError::InvalidInput("print body"));
        }
        Ok(Self {
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    /// 見出し。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 本文（`\n` 区切り）。
    pub fn body(&self) -> &str {
        &self.body
    }

    /// 本文の行。
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.body.split('\n')
    }
}

/// 検査済みの画面の画像（RGBA、行は詰めて上から下）。[`PrintImage::new`] でのみ作れる。
///
/// 画面の内容の印刷（対応表 `print-screen`）で、egui のフレームの読み戻しを OS の印刷へ渡す入れ物。
/// 寸法と長さの不変条件（各辺 1〜[`IMAGE_MAX_SIDE`]、`rgba.len() == width * height * 4`）が
/// OS 呼び出し（FFI のポインタと長さ）の安全の根拠になる。画素はログへ出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct PrintImage {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for PrintImage {
    // 画素を書き出さない（security.md A09）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PrintImage({}x{})", self.width, self.height)
    }
}

impl PrintImage {
    /// 幅・高さは各 1〜[`IMAGE_MAX_SIDE`]、`rgba.len()` は `width * height * 4` と一致すること。
    /// 違反は `Internal("print capture")`（画面を取り込めない扱い。ダイアログを出す前に確定する）。
    pub fn new(rgba: Vec<u8>, width: u32, height: u32) -> Result<Self, DeviceError> {
        let side_ok = |v: u32| (1..=IMAGE_MAX_SIDE).contains(&v);
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4));
        if !side_ok(width) || !side_ok(height) || expected != Some(rgba.len()) {
            return Err(DeviceError::Internal("print capture"));
        }
        Ok(Self {
            rgba,
            width,
            height,
        })
    }

    /// RGBA の画素列。
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// 幅（画素）。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ（画素）。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// BGRA 並びの画素列（Windows の 32bpp top-down DIB 用）。
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut v = self.rgba.clone();
        // 長さは 4 の倍数（不変条件）なので端数は出ない。
        for px in v.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        v
    }

    /// ARGB の `i32` 列（Android `Bitmap.setPixels` 用）。
    pub fn to_argb(&self) -> Vec<i32> {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| i32::from_be_bytes([p[3], p[0], p[1], p[2]]))
            .collect()
    }
}

/// 縦横比を保ち、拡大・縮小とも中央寄せで `area_w x area_h` の領域に収めた矩形
/// `(x, y, w, h)` を返す。領域・元の寸法が 0 以下なら `None`。収めた幅・高さは 1 以上かつ領域以内。
pub fn fit_rect(src_w: i32, src_h: i32, area_w: i32, area_h: i32) -> Option<(i32, i32, i32, i32)> {
    if src_w <= 0 || src_h <= 0 || area_w <= 0 || area_h <= 0 {
        return None;
    }
    let (sw, sh, aw, ah) = (src_w as i64, src_h as i64, area_w as i64, area_h as i64);
    // 幅基準で収まるか（aw / sw <= ah / sh）を交差乗算で判定する。
    let (w, h) = if aw * sh <= ah * sw {
        (aw, (aw * sh / sw).max(1))
    } else {
        ((ah * sw / sh).max(1), ah)
    };
    Some((
        ((aw - w) / 2) as i32,
        ((ah - h) / 2) as i32,
        w as i32,
        h as i32,
    ))
}

/// 印刷ダイアログを開く。OS ごとの実装は `fandhe-egui-proxy` / `fandhe-web-dom` が持つ。
pub trait Print: Send + Sync {
    /// 印刷ダイアログを開く。結果の意味と OS ごとの到達値はモジュール doc。
    fn print(&self, req: &PrintRequest) -> Result<PrintOutcome, DeviceError>;

    /// 画面の画像を 1 ページに収めて印刷ダイアログへ渡す（対応表 `print-screen`）。結果の意味は
    /// [`Print::print`] と同じ。既定は非対応で、対応する OS だけが上書きする。
    fn print_image(&self, _img: &PrintImage) -> Result<PrintOutcome, DeviceError> {
        Err(DeviceError::Unsupported("print"))
    }
}

/// 非対応 OS 用。常に `Unsupported` を返し、成功を作らない。
pub struct UnsupportedPrint;

impl Print for UnsupportedPrint {
    fn print(&self, _req: &PrintRequest) -> Result<PrintOutcome, DeviceError> {
        Err(DeviceError::Unsupported("print"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 印刷に権限は要らないため、権限拒否の失敗系は該当なし。

    #[test]
    fn boundaries_pass() {
        let title = "あ".repeat(64);
        let body = vec!["a".repeat(80); 20].join("\n");
        let r = PrintRequest::new(&title, &body).unwrap();
        assert_eq!(r.lines().count(), 20);
        assert_eq!(r.title(), title);
        assert_eq!(r.body(), body);
        assert!(PrintRequest::new("t", "x").is_ok());
    }

    #[test]
    fn invalid_title_is_rejected() {
        for t in ["", &"a".repeat(65), "a\nb", "a\tb", "a\0b", "a\rb"] {
            assert!(matches!(
                PrintRequest::new(t, "x"),
                Err(DeviceError::InvalidInput("print title"))
            ));
        }
    }

    #[test]
    fn invalid_body_is_rejected() {
        let too_many = vec!["a"; 21].join("\n");
        let too_long = "a".repeat(81);
        for b in [
            "",
            too_many.as_str(),
            too_long.as_str(),
            "a\n\nb",
            "a\n",
            "\na",
            "a\r\nb",
            "a\tb",
            "a\0b",
        ] {
            assert!(
                matches!(
                    PrintRequest::new("t", b),
                    Err(DeviceError::InvalidInput("print body"))
                ),
                "{b:?}"
            );
        }
    }

    #[test]
    fn unsupported_is_never_success() {
        let req = PrintRequest::new("t", "x").unwrap();
        assert!(matches!(
            UnsupportedPrint.print(&req),
            Err(DeviceError::Unsupported("print"))
        ));
    }

    #[test]
    fn image_new_checks_bounds() {
        assert!(PrintImage::new(vec![0; 4], 1, 1).is_ok());
        for (len, w, h) in [(0, 0, 1), (0, 1, 0), (4, 1, 2), (8, 1, 1), (0, 8193, 1)] {
            assert!(
                matches!(
                    PrintImage::new(vec![0; len], w, h),
                    Err(DeviceError::Internal("print capture"))
                ),
                "{len} {w} {h}"
            );
        }
        // 上限ちょうどは通る。
        assert!(PrintImage::new(vec![0; 8192 * 8192 * 4], 8192, 8192).is_ok());
        // 乗算の桁あふれ・巨大寸法は拒否する。
        assert!(PrintImage::new(vec![], u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn image_conversions() {
        let img = PrintImage::new(vec![1, 2, 3, 4], 1, 1).unwrap();
        assert_eq!(img.to_bgra(), vec![3, 2, 1, 4]);
        assert_eq!(img.to_argb(), vec![i32::from_be_bytes([4, 1, 2, 3])]);
        assert_eq!(format!("{img:?}"), "PrintImage(1x1)");
    }

    #[test]
    fn fit_rect_keeps_aspect_and_centers() {
        assert_eq!(fit_rect(200, 100, 100, 200), Some((0, 75, 100, 50)));
        assert_eq!(fit_rect(100, 200, 200, 100), Some((75, 0, 50, 100)));
        assert_eq!(fit_rect(10, 10, 100, 100), Some((0, 0, 100, 100)));
        assert_eq!(fit_rect(4, 3, 400, 300), Some((0, 0, 400, 300)));
        assert_eq!(fit_rect(10000, 1, 100, 100).map(|r| r.3), Some(1));
        for (a, b, c, d) in [(0, 1, 1, 1), (1, 0, 1, 1), (1, 1, 0, 1), (1, 1, 1, -1)] {
            assert_eq!(fit_rect(a, b, c, d), None);
        }
    }

    #[test]
    fn unsupported_image_is_never_success() {
        let img = PrintImage::new(vec![0; 4], 1, 1).unwrap();
        assert!(matches!(
            UnsupportedPrint.print_image(&img),
            Err(DeviceError::Unsupported("print"))
        ));
    }
}
