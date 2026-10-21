//! 端末内翻訳の抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/translation.json`、
//! 親 #115、Issue #535）。
//!
//! 扱う範囲は「固定の原文を、元言語と訳先言語を明示して端末内で 1 回訳し、結果を表示する。
//! 使えない端末・組み合わせでは理由つきの失敗を出す」ことだけ。元言語の自動判定・言語パックの
//! ダウンロード開始・利用者が入力する原文・取り消しは扱わない。対象は macOS / iOS（Translation
//! フレームワーク）・Android（`android.view.translation`）・Web（Translator API）。Windows・Linux は
//! `Unsupported` 固定。一発呼び出し型の同期 API で、呼び出し側（`fandhe-egui-proxy` の effect
//! ドライバ）は UI を止めないようワーカースレッドから呼ぶ。Web は同じ契約を `fandhe-web-dom` の
//! 非同期 effect で実装する。
//!
//! # 契約
//!
//! - **成功**: 空でない訳文だけ（[`check_result`]）。言語パックが無い・組み合わせが非対応の状態では
//!   訳文を作らず失敗にする（`device-bridge-contracts.md`「6. 嘘の成功」）。出力が出力バッファを
//!   超える場合も切り詰めず失敗（`Internal`）にする
//! - **失敗の区別**（写像表）:
//!
//! | 状況 | Apple | Android | Web | 返す値 |
//! | --- | --- | --- | --- | --- |
//! | OS が古い・API が無い | `#available` / `canImport` 不成立 | API 31 未満 | `Translator` が無い | `Unsupported` |
//! | 言語の組み合わせが非対応 | `unsupported*Language*`・`.unsupported` | 非対応の組み合わせ | `unavailable` | `Internal(`[`LANGUAGE_UNSUPPORTED`]`)` |
//! | 言語パックが未導入・取得中 | `.supported`・`notInstalled` | 未導入 | `downloadable`・`downloading` | `Internal(`[`NOT_INSTALLED`]`)` |
//! | 期限切れ | Rust の期限 | Rust の期限 | Rust の期限 | `Internal(`[`TIMEOUT`]`)` |
//! | 空・空白だけの訳文 | — | — | — | `Internal(`[`EMPTY`]`)` |
//! | `create()` の拒否 | — | — | `NotAllowedError` | `PermissionDenied` |
//! | 入力不正 | — | — | — | `InvalidInput`（[`validate_request`]） |
//! | それ以外 | 他の `TranslationError` | 他の例外 | 他の例外 | `Internal("translation")` |
//!
//! - **三値**（契約表 10）: 利用者の取り消し操作が無いため成功 / 失敗の 2 値
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`TRANSLATE_DEADLINE`]、初回の
//!   モデル読み込みを含む）。起点は要求を観測した時の `Instant`（Web は `performance.now()`）。
//!   期限後に届いた成功は捨てる。Web の `pagehide` は「中断」として失敗にする
//! - **並行性**（契約表 2）: 世代は画面が要求ごとに進め、完了は「処理中かつ同じ世代」の
//!   ときだけ適用する。処理中の再押下は無視する
//! - **ライフサイクル**（契約表 1）: Android は Activity 再生成でドライバの記録を捨てる
//! - **資源の解放**（契約表 9）: Apple のセッションは呼び出しごとに作り、期限切れで `cancel()`
//!   する。Android・Web は全経路で `destroy()` する
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、資源の解放。守らない範囲:
//! OS ごとの所要時間の違い、訳文の非決定性、期限後も OS 側の翻訳が走り切ること（期限直後に
//! 繰り返し押すと翻訳が一時的に重なり得る）、複数タブ・複数プロセス。原文・訳文・OS のエラー文は
//! ログへ出さない。

use std::time::Duration;

use crate::DeviceError;

/// 要求を受け付けてから結果を確定するまでの期限（Rust のドライバだけが持つ。初回のモデル読み込みを含む）。
pub const TRANSLATE_DEADLINE: Duration = Duration::from_secs(30);
/// 原文の最大バイト数。
pub const MAX_TEXT_BYTES: usize = 1024;
/// Apple の出力バッファのバイト数。
pub const TRANSLATION_BUF_BYTES: usize = 4 * 1024;
/// 言語タグ（BCP 47）の最大バイト数。
pub const MAX_LANG_TAG_BYTES: usize = 35;

/// 言語の組み合わせが非対応のときの `Internal` の識別子（SSOT）。
pub const LANGUAGE_UNSUPPORTED: &str = "translation language_unsupported";
/// 言語パックが端末にまだ無い（未導入・取得中）ときの `Internal` の識別子（SSOT）。
pub const NOT_INSTALLED: &str = "translation not_installed";
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "translation timeout";
/// 訳文が空のときの `Internal` の識別子（SSOT）。
pub const EMPTY: &str = "translation empty";

/// 端末内翻訳の契約（モジュール doc の「契約」節）。
pub trait TextTranslator: Send + Sync {
    /// `text` を `source` から `target` へ 1 回訳して返す。`remaining` は期限までの残り時間。
    fn translate(
        &self,
        text: &str,
        source: &str,
        target: &str,
        remaining: Duration,
    ) -> Result<String, DeviceError>;
}

/// 対象外 OS・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedTextTranslator;

impl TextTranslator for UnsupportedTextTranslator {
    fn translate(&self, _: &str, _: &str, _: &str, _: Duration) -> Result<String, DeviceError> {
        Err(DeviceError::Unsupported("translation"))
    }
}

/// BCP 47 の構文だけを見る（主タグは ASCII 英字 2〜3 文字、続くサブタグは 1〜8 文字の英数字）。
/// 言語が実在・対応かどうかは OS に判定させる。
fn valid_lang_tag(tag: &str) -> bool {
    if tag.len() > MAX_LANG_TAG_BYTES {
        return false;
    }
    let mut parts = tag.split('-');
    let primary_ok = parts
        .next()
        .is_some_and(|p| (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()));
    primary_ok
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// 要求を OS へ渡す前に検査する。原文が空・空白だけ・NUL を含む・`MAX_TEXT_BYTES` 超過、
/// または言語タグが BCP 47 の構文に反するときは `InvalidInput`。
pub fn validate_request(text: &str, source: &str, target: &str) -> Result<(), DeviceError> {
    if text.trim().is_empty() || text.contains('\0') || text.len() > MAX_TEXT_BYTES {
        return Err(DeviceError::InvalidInput("translation text"));
    }
    if !valid_lang_tag(source) || !valid_lang_tag(target) {
        return Err(DeviceError::InvalidInput("translation lang"));
    }
    Ok(())
}

/// 訳文を検査する。trim 後に空なら `Internal(EMPTY)`。それ以外は入力どおり返す（trim しない）。
pub fn check_result(text: String) -> Result<String, DeviceError> {
    if text.trim().is_empty() {
        Err(DeviceError::Internal(EMPTY))
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_text_boundaries() {
        assert!(validate_request("今日", "ja", "en").is_ok());
        assert!(validate_request(&"a".repeat(MAX_TEXT_BYTES), "ja", "en").is_ok());
        for bad in [
            "".to_string(),
            " \n\t".to_string(),
            "a\0b".to_string(),
            "a".repeat(MAX_TEXT_BYTES + 1),
        ] {
            assert!(
                matches!(
                    validate_request(&bad, "ja", "en"),
                    Err(DeviceError::InvalidInput("translation text"))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn validate_lang_tags() {
        for ok in ["ja", "en", "zh-Hant", "zz", "pt-BR"] {
            assert!(validate_request("x", ok, "en").is_ok(), "{ok}");
        }
        let long = "a".repeat(36);
        for bad in [
            "",
            "j",
            "jaaa",
            "ja_JP",
            "ja-",
            "日本",
            "ja-toolongsub1",
            &long,
        ] {
            assert!(
                matches!(
                    validate_request("x", "ja", bad),
                    Err(DeviceError::InvalidInput("translation lang"))
                ),
                "{bad:?}"
            );
            assert!(validate_request("x", bad, "en").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn check_result_rejects_empty_and_keeps_text() {
        for bad in ["", "  \n"] {
            assert!(matches!(
                check_result(bad.to_string()),
                Err(DeviceError::Internal(EMPTY))
            ));
        }
        assert_eq!(check_result(" Hi ".to_string()).unwrap(), " Hi ");
    }

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let e = UnsupportedTextTranslator
            .translate("x", "ja", "en", TRANSLATE_DEADLINE)
            .unwrap_err();
        assert!(matches!(e, DeviceError::Unsupported("translation")));
    }
}
