//! 単語分割の抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/nlp.json`）。
//!
//! 扱う範囲は「入力文字列を単語に分割し、件数と一覧を返す」ことだけ。品詞タグ付け・
//! 言語判定・翻訳・要約は扱わない。対象は iOS / macOS（NaturalLanguage `NLTokenizer`）、
//! Android（`android.icu.text.BreakIterator`）、Windows（`WordsSegmenter`）、
//! Web（`Intl.Segmenter`）。一発呼び出し型。各 OS 実装が得た生のトークン列は
//! [`normalize_words`] で契約の形へ揃えてから返す。
//!
//! # 契約
//!
//! - **OS 間で分割結果の一致を保証しない**。辞書・規則が OS ごとに違い、同じ入力でも
//!   件数・境界が異なりうる（特に空白区切りの無い日本語等）。呼び出し側・テストは
//!   「件数 = 一覧の長さ」「各語が空でない」「各語が入力の部分文字列に由来する」といった
//!   性質だけに依存し、特定の分割結果に依存しない
//! - **含める語**: 前後空白を除いたうえで、文字・数字（`char::is_alphanumeric`）を 1 つ以上
//!   含むトークンだけ。空白だけ・句読点や記号だけのトークンは除く（[`normalize_words`]。
//!   各 OS 実装は必ずこれを通す）。順序は入力中の出現順、重複は残す
//! - **件数は一覧の長さ**: 件数用の別の値を持たず `Vec::len()` を件数とする
//! - **空文字入力は空リストの成功**（対応 OS の実装は OS を呼ばずに `Ok(vec![])`）。空白だけの
//!   入力も正規化後に空となり同じ扱い。非対応 OS（[`UnsupportedWordSegmenter`]）は入力に
//!   関わらず `Err(Unsupported)` を返す（空入力でも「使える」と見せない）
//! - **入力長の上限**: [`MAX_INPUT_CHARS`] 文字（Unicode スカラー値）。超過・NUL 文字を
//!   含む入力は `Err(InvalidInput)`（[`validate_input`]。実装は OS を呼ぶ前に必ず通す）。
//!   切り詰めて成功にしない
//! - **失敗の区別**: 非対応 OS・API 無し（Web で `Intl.Segmenter` が無い等）→
//!   `Err(Unsupported)`、OS 呼び出しの失敗 → `Err(Internal)`。失敗を空リストの成功へ
//!   畳まない（`device-bridge-contracts.md`「6. 嘘の成功」）
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは入力の検証・正規化の一貫性と、失敗を成功と見せないこと。分割結果の OS 差、
//! `NLTokenizer` / `BreakIterator` 等のバージョンによる境界の違い、巨大入力に近い長さでの
//! 処理時間は扱わない（上限で頭打ちにするのみ）。同期で完結し確保する資源は無いため、
//! 並行・画面離脱に関する要求追跡は持たない。

use crate::DeviceError;

/// 入力文字数（Unicode スカラー値）の上限。超過は `InvalidInput`。
pub const MAX_INPUT_CHARS: usize = 10_000;

/// 単語分割の契約（モジュール doc の「契約」節）。
pub trait WordSegmenter: Send + Sync {
    /// `text` を単語に分割し、[`normalize_words`] 済みの一覧を返す。件数は戻り値の長さ。
    ///
    /// 実装は [`validate_input`] を通し、空入力は OS を呼ばず `Ok(vec![])` を返す。
    fn segment(&self, text: &str) -> Result<Vec<String>, DeviceError>;
}

/// 対象外 OS（Linux 等）・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedWordSegmenter;

impl WordSegmenter for UnsupportedWordSegmenter {
    fn segment(&self, _text: &str) -> Result<Vec<String>, DeviceError> {
        Err(DeviceError::Unsupported("nlp"))
    }
}

/// 入力が契約に収まるか検証する（長さ上限・NUL 混入）。各 OS 実装が OS を呼ぶ前に使う。
pub fn validate_input(text: &str) -> Result<(), DeviceError> {
    if text.contains('\0') {
        return Err(DeviceError::InvalidInput("nlp text"));
    }
    // 上限 + 1 文字目の有無だけ見る（全体を数えない）。
    if text.chars().nth(MAX_INPUT_CHARS).is_some() {
        return Err(DeviceError::InvalidInput("nlp text length"));
    }
    Ok(())
}

/// OS が返した生のトークン列を契約の形へ揃える: 前後空白を除き、文字・数字を 1 つも
/// 含まないもの（空・空白・句読点・記号のみ）を除く。順序と重複は保つ。
pub fn normalize_words<I, S>(raw: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    raw.into_iter()
        .map(|t| t.as_ref().trim().to_owned())
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_empty_whitespace_and_punctuation_only() {
        let out = normalize_words(["Hello", " ", "", ",", "  world ", "!?", "日本語", "。"]);
        assert_eq!(out, ["Hello", "world", "日本語"]);
    }

    #[test]
    fn normalize_keeps_order_and_duplicates() {
        assert_eq!(normalize_words(["a", "b", "a"]), ["a", "b", "a"]);
    }

    #[test]
    fn normalize_empty_input_is_empty() {
        assert!(normalize_words(Vec::<String>::new()).is_empty());
    }

    #[test]
    fn validate_accepts_empty_and_limit_length() {
        assert!(validate_input("").is_ok());
        assert!(validate_input(&"あ".repeat(MAX_INPUT_CHARS)).is_ok());
    }

    #[test]
    fn validate_rejects_over_limit_and_nul() {
        let long = "a".repeat(MAX_INPUT_CHARS + 1);
        assert!(matches!(
            validate_input(&long),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_input("a\0b"),
            Err(DeviceError::InvalidInput(_))
        ));
    }

    #[test]
    fn unsupported_segmenter_is_not_a_fake_success() {
        let err = UnsupportedWordSegmenter.segment("hello").unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)));
    }
}
