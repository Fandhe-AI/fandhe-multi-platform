//! 印刷（OS の印刷ダイアログを開く）機能の抽象（Phase 11 の拡充ループ、親 #94、Issue #325）。
//!
//! 範囲は「固定の短い文章（見出し 1 行 + 本文数行）を 1 ページとして OS の印刷ダイアログに渡す」
//! ことだけ。複数ページ・画像・PDF・プリンタ列挙・進捗・紙に出たかの追跡は扱わない。
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
//! | 状況 | 返す値 |
//! |---|---|
//! | 入力不正 | `InvalidInput("print title")` / `InvalidInput("print body")` |
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

/// 印刷ダイアログを開く。OS ごとの実装は `fandhe-egui-proxy` / `fandhe-web-dom` が持つ。
pub trait Print: Send + Sync {
    /// 印刷ダイアログを開く。結果の意味と OS ごとの到達値はモジュール doc。
    fn print(&self, req: &PrintRequest) -> Result<PrintOutcome, DeviceError>;
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
}
