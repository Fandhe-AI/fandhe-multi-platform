//! スペルチェックの抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/spell-check.json`、
//! 親 #119、Issue #578）。
//!
//! 扱う範囲は「入力した短い文（[`MAX_TEXT_CHARS`] 文字まで）を、BCP 47 で指定した言語の OS 辞書で
//! 1 回検査し、誤り語ごとに位置・語・修正候補（最大 [`MAX_SUGGESTIONS`] 件）を出現順に返す」こと。
//! 誤りは [`MAX_ISSUES`] 件までで、超えたら [`SpellReport::truncated`] を立てる。範囲外は文法
//! チェック・自動修正の適用・学習語／無視語（端末のユーザー辞書を書き換える）・入力中のリアルタイム
//! 検査・言語の自動判定・辞書のダウンロード・日本語・Windows の重複語検出。対象は macOS（`NSSpellChecker`）・
//! iOS（`UITextChecker`）・Android（`TextServicesManager`）・Windows（`ISpellChecker`）。Web は
//! 呼び出せる API が無く対象外。呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は UI を止めない
//! ようワーカースレッドから [`SpellChecker::check`] を呼ぶ。
//!
//! # 契約
//!
//! - **成功**: 辞書のある言語で検査した結果だけ。誤り 0 件も成功（辞書がある場合に限る）。
//!   **辞書の無い言語を「誤り 0 件」にしない**: OS の検査を呼ぶ前に必ず利用可能な言語を確かめ、
//!   [`resolve_language`] が `None` なら `Internal(`[`LANGUAGE_UNAVAILABLE`]`)` にする。これは全 OS 共通の義務
//! - **失敗の区別**（写像表）:
//!
//! | 状況 | 返す値 |
//! | --- | --- |
//! | 非対応 OS・スペルチェッカーが無効（macOS で `sharedSpellChecker` が nil 等） | `Unsupported` |
//! | 言語の辞書が無い | `Internal(`[`LANGUAGE_UNAVAILABLE`]`)` |
//! | 期限切れ | `Internal(`[`TIMEOUT`]`)` |
//! | 入力不正 | `InvalidInput`（[`validate_request`]） |
//! | それ以外（範囲の異常・バッファ不足・OS の失敗） | `Internal`（固定の識別子） |
//!
//! - **三値**（契約表 10）: 利用者の取り消し操作が無いため成功 / 失敗の 2 値。`Cancelled`・
//!   `PermissionDenied` が実装から届いても成功にせず、画面では固定文言の失敗にする
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`SPELL_CHECK_DEADLINE`]）。起点は
//!   要求を観測した時の `Instant`。期限後に届いた成功は捨てる
//! - **並行性**（契約表 2）: 世代は画面が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ
//!   適用する。処理中の再押下は無視する
//! - **資源の解放**（契約表 9）: OS 側に確保した資源（macOS の document tag 等）は成功・失敗・期限切れの
//!   全経路で解放する。端末のユーザー辞書・共有の言語設定は書き換えない
//! - **位置**: [`SpellIssue::start`]・[`SpellIssue::len`] は Unicode のスカラー値（`char`）単位。OS は
//!   UTF-16 単位で返すため [`RawIssue`] で受け、[`build_report`] が変換する。語は入力文から切り出す
//!   （OS から受け取らない）
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内の要求と完了の順序、嘘の成功の禁止、資源の解放（macOS の document tag 等）、
//! 入力文・誤り語・候補のログ非出力。守らない範囲: OS 間での検出結果・候補の一致、利用者が OS に学習
//! させた語による結果の違い、Windows の重複語検出、期限後も OS 側の検査が走り切ること、Android API 30
//! 以下での「スペルチェッカー無効」と「言語未導入」の区別、複数タブ・複数プロセス。

use std::fmt;
use std::time::Duration;

use crate::DeviceError;
use crate::translation::valid_lang_tag;

/// 要求を受け付けてから結果を確定するまでの期限（Rust のドライバだけが持つ）。
pub const SPELL_CHECK_DEADLINE: Duration = Duration::from_secs(10);
/// 入力文の最大文字数（バイト数ではない）。
pub const MAX_TEXT_CHARS: usize = 1000;
/// 誤り語 1 件あたりの修正候補の最大件数。
pub const MAX_SUGGESTIONS: usize = 5;
/// 返す誤りの最大件数。超えたら [`SpellReport::truncated`] を立てる。
pub const MAX_ISSUES: usize = 50;
/// 言語の既定値（BCP 47）。
pub const DEFAULT_LANGUAGE: &str = "en-US";

/// 言語の辞書が端末に無いときの `Internal` の識別子（SSOT）。
pub const LANGUAGE_UNAVAILABLE: &str = "spell_check language_unavailable";
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "spell_check timeout";
/// 範囲の異常（変換できない・重なる）のときの `Internal` の識別子。
const RANGE_INVALID: &str = "spell_check range";

/// 誤り語 1 件。位置と長さは `char` 単位。
#[derive(Clone, PartialEq, Eq)]
pub struct SpellIssue {
    /// 入力文の中での開始位置（`char` 単位）。
    pub start: usize,
    /// 誤り語の長さ（`char` 単位）。
    pub len: usize,
    /// 誤り語（入力文から切り出したもの）。
    pub word: String,
    /// 修正候補（最大 [`MAX_SUGGESTIONS`] 件。空もあり得る）。
    pub suggestions: Vec<String>,
}

// 入力文の一部をログへ漏らさないよう、語と候補は件数だけを出す（security.md A09）。
impl fmt::Debug for SpellIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpellIssue")
            .field("start", &self.start)
            .field("len", &self.len)
            .field("word_chars", &self.word.chars().count())
            .field("suggestions", &self.suggestions.len())
            .finish()
    }
}

/// 検査の結果。
#[derive(Clone, PartialEq, Eq)]
pub struct SpellReport {
    /// 実際に検査に使った言語（OS の識別子）。
    pub language: String,
    /// 誤り語（開始位置の昇順。最大 [`MAX_ISSUES`] 件）。
    pub issues: Vec<SpellIssue>,
    /// 誤りが [`MAX_ISSUES`] 件を超えて打ち切ったか。
    pub truncated: bool,
}

impl fmt::Debug for SpellReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpellReport")
            .field("language", &self.language)
            .field("issues", &self.issues)
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// OS が返す誤りの生データ。4 OS とも位置を UTF-16 単位で返すため、共通の入力にする。
#[derive(Clone, PartialEq, Eq)]
pub struct RawIssue {
    /// 誤り語の開始位置（UTF-16 単位）。
    pub utf16_start: usize,
    /// 誤り語の長さ（UTF-16 単位）。
    pub utf16_len: usize,
    /// OS の返した修正候補（検査前。[`build_report`] が整形する）。
    pub suggestions: Vec<String>,
}

impl fmt::Debug for RawIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawIssue")
            .field("utf16_start", &self.utf16_start)
            .field("utf16_len", &self.utf16_len)
            .field("suggestions", &self.suggestions.len())
            .finish()
    }
}

/// スペルチェックの契約（モジュール doc の「契約」節）。
pub trait SpellChecker: Send + Sync {
    /// `text` を `language`（BCP 47）の辞書で 1 回検査する。`remaining` は期限までの残り時間。
    /// 呼び出し側は事前に [`validate_request`] を通す。
    fn check(
        &self,
        text: &str,
        language: &str,
        remaining: Duration,
    ) -> Result<SpellReport, DeviceError>;
}

/// 対象外 OS・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedSpellChecker;

impl SpellChecker for UnsupportedSpellChecker {
    fn check(&self, _: &str, _: &str, _: Duration) -> Result<SpellReport, DeviceError> {
        Err(DeviceError::Unsupported("spell_check"))
    }
}

/// 要求を OS へ渡す前に検査する。文が空・空白だけ・NUL を含む・[`MAX_TEXT_CHARS`] 文字超過、
/// または言語タグが BCP 47 の構文に反するときは `InvalidInput`。
pub fn validate_request(text: &str, language: &str) -> Result<(), DeviceError> {
    if text.trim().is_empty() || text.contains('\0') || text.chars().count() > MAX_TEXT_CHARS {
        return Err(DeviceError::InvalidInput("spell_check text"));
    }
    if !valid_lang_tag(language) {
        return Err(DeviceError::InvalidInput("spell_check lang"));
    }
    Ok(())
}

fn normalize_tag(tag: &str) -> String {
    tag.replace('_', "-").to_ascii_lowercase()
}

/// 要求の言語を OS の利用可能な言語の一覧から解決する。`_` と `-` は同一視し、大文字小文字は
/// 無視する。完全一致を先に探し、無ければ同じ主言語のうち OS の列挙順で先頭のものを選ぶ。
/// 戻り値は OS の識別子そのまま。見つからなければ `None`（呼び出し側は
/// `Internal(`[`LANGUAGE_UNAVAILABLE`]`)` にする。「誤り 0 件」にしない）。
pub fn resolve_language<S: AsRef<str>>(requested: &str, available: &[S]) -> Option<String> {
    let want = normalize_tag(requested);
    if let Some(exact) = available.iter().find(|a| normalize_tag(a.as_ref()) == want) {
        return Some(exact.as_ref().to_string());
    }
    let primary = want.split('-').next().unwrap_or("");
    if primary.is_empty() {
        return None;
    }
    available
        .iter()
        .find(|a| {
            normalize_tag(a.as_ref())
                .split('-')
                .next()
                .is_some_and(|p| p == primary)
        })
        .map(|a| a.as_ref().to_string())
}

/// UTF-16 単位の範囲を `char` 単位の `(開始, 長さ)` へ直す。長さ 0・範囲外・サロゲートペアの途中で
/// 始まるか終わる範囲は `None`。
pub fn utf16_range_to_chars(
    text: &str,
    utf16_start: usize,
    utf16_len: usize,
) -> Option<(usize, usize)> {
    if utf16_len == 0 {
        return None;
    }
    let end = utf16_start.checked_add(utf16_len)?;
    let mut offset = 0usize;
    let mut start_char = None;
    let mut total = 0usize;
    for (i, ch) in text.chars().enumerate() {
        if offset == utf16_start {
            start_char = Some(i);
        }
        if offset == end {
            return start_char.map(|s| (s, i - s));
        }
        offset += ch.len_utf16();
        total = i + 1;
    }
    (offset == end).then(|| start_char.map(|s| (s, total - s)))?
}

fn clean_suggestions(raw: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in raw {
        if s.is_empty() || s.contains('\0') || out.contains(&s) {
            continue;
        }
        out.push(s);
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    out
}

/// OS の生データから [`SpellReport`] を組み立てる。開始位置の昇順に並べ、範囲を `char` 単位に直せない
/// 場合と範囲が重なる場合は `Internal`（嘘の成功にしない）。語は入力文から切り出す。候補は空・NUL を含む
/// ものを除き、重複を消して先頭 [`MAX_SUGGESTIONS`] 件にする。[`MAX_ISSUES`] 件を超えたら切り詰めて
/// `truncated` を立てる（OS 側で打ち切った場合は `os_truncated` で反映する）。
pub fn build_report(
    text: &str,
    language: &str,
    raw: Vec<RawIssue>,
    os_truncated: bool,
) -> Result<SpellReport, DeviceError> {
    let chars: Vec<char> = text.chars().collect();
    let mut issues = Vec::with_capacity(raw.len().min(MAX_ISSUES + 1));
    for r in raw {
        let (start, len) = utf16_range_to_chars(text, r.utf16_start, r.utf16_len)
            .ok_or(DeviceError::Internal(RANGE_INVALID))?;
        issues.push(SpellIssue {
            start,
            len,
            word: chars[start..start + len].iter().collect(),
            suggestions: clean_suggestions(r.suggestions),
        });
    }
    issues.sort_by_key(|i| i.start);
    if issues
        .windows(2)
        .any(|w| w[1].start < w[0].start + w[0].len)
    {
        return Err(DeviceError::Internal(RANGE_INVALID));
    }
    let mut truncated = os_truncated;
    if issues.len() > MAX_ISSUES {
        issues.truncate(MAX_ISSUES);
        truncated = true;
    }
    Ok(SpellReport {
        language: language.to_string(),
        issues,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(start: usize, len: usize, sugg: &[&str]) -> RawIssue {
        RawIssue {
            utf16_start: start,
            utf16_len: len,
            suggestions: sugg.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn validate_text_boundaries() {
        assert!(validate_request("hello", "en-US").is_ok());
        assert!(validate_request(&"a".repeat(MAX_TEXT_CHARS), "en").is_ok());
        // マルチバイトは文字数で数える（1000 文字は 3000 バイトでも可）
        assert!(validate_request(&"あ".repeat(MAX_TEXT_CHARS), "en").is_ok());
        for bad in [
            String::new(),
            " \n\t".to_string(),
            "a\0b".to_string(),
            "a".repeat(MAX_TEXT_CHARS + 1),
            "あ".repeat(MAX_TEXT_CHARS + 1),
        ] {
            assert!(
                matches!(
                    validate_request(&bad, "en"),
                    Err(DeviceError::InvalidInput("spell_check text"))
                ),
                "{}",
                bad.chars().count()
            );
        }
    }

    #[test]
    fn validate_lang_tags() {
        for bad in ["", "e", "en_US", "en-", "日本", "en-toolongsub1"] {
            assert!(
                matches!(
                    validate_request("x", bad),
                    Err(DeviceError::InvalidInput("spell_check lang"))
                ),
                "{bad:?}"
            );
        }
        assert!(validate_request("x", "zz").is_ok());
    }

    #[test]
    fn resolve_exact_and_normalized() {
        let av = ["en_GB", "en_US", "fr"];
        assert_eq!(resolve_language("en-US", &av).as_deref(), Some("en_US"));
        assert_eq!(resolve_language("EN-us", &av).as_deref(), Some("en_US"));
        assert_eq!(resolve_language("fr", &av).as_deref(), Some("fr"));
    }

    #[test]
    fn resolve_primary_takes_first_in_os_order() {
        let av = ["en_GB", "en_US"];
        assert_eq!(resolve_language("en-AU", &av).as_deref(), Some("en_GB"));
        assert_eq!(resolve_language("en", &av).as_deref(), Some("en_GB"));
    }

    #[test]
    fn resolve_none_when_missing() {
        assert_eq!(resolve_language("zz", &["en", "fr"]), None);
        assert_eq!(resolve_language("ja", &["en_US"]), None);
        assert_eq!(resolve_language("en", &Vec::<String>::new()), None);
    }

    #[test]
    fn utf16_ranges() {
        // ASCII
        assert_eq!(utf16_range_to_chars("abc def", 4, 3), Some((4, 3)));
        assert_eq!(utf16_range_to_chars("abc", 0, 3), Some((0, 3)));
        // BMP の多バイト文字は UTF-16 で 1 単位
        assert_eq!(utf16_range_to_chars("あい abc", 3, 3), Some((3, 3)));
        // 絵文字（サロゲートペア）は UTF-16 で 2 単位
        assert_eq!(utf16_range_to_chars("😀 abc", 3, 3), Some((2, 3)));
        assert_eq!(utf16_range_to_chars("😀", 0, 2), Some((0, 1)));
    }

    #[test]
    fn utf16_invalid_ranges_are_none() {
        assert_eq!(utf16_range_to_chars("abc", 1, 0), None);
        assert_eq!(utf16_range_to_chars("abc", 2, 2), None);
        assert_eq!(utf16_range_to_chars("abc", 5, 1), None);
        assert_eq!(utf16_range_to_chars("abc", usize::MAX, 2), None);
        // サロゲートペアの途中で始まる / 終わる
        assert_eq!(utf16_range_to_chars("😀a", 1, 2), None);
        assert_eq!(utf16_range_to_chars("😀a", 0, 1), None);
    }

    #[test]
    fn build_report_orders_and_extracts_words() {
        let text = "This sentense has a speling mistake.";
        let r = build_report(
            text,
            "en",
            vec![raw(20, 7, &["spelling"]), raw(5, 8, &["sentence"])],
            false,
        )
        .unwrap();
        assert_eq!(r.issues[0].word, "sentense");
        assert_eq!(r.issues[0].start, 5);
        assert_eq!(r.issues[1].word, "speling");
        assert!(!r.truncated);
    }

    #[test]
    fn build_report_cleans_suggestions() {
        let r = build_report(
            "abc",
            "en",
            vec![raw(0, 3, &["", "a\0b", "x", "x", "1", "2", "3", "4", "5"])],
            false,
        )
        .unwrap();
        assert_eq!(r.issues[0].suggestions, ["x", "1", "2", "3", "4"]);
    }

    #[test]
    fn build_report_truncates_issues() {
        let text = "a ".repeat(MAX_ISSUES + 5);
        let many: Vec<_> = (0..MAX_ISSUES + 3).map(|i| raw(i * 2, 1, &[])).collect();
        let r = build_report(&text, "en", many, false).unwrap();
        assert_eq!(r.issues.len(), MAX_ISSUES);
        assert!(r.truncated);
        let one = build_report("a", "en", vec![raw(0, 1, &[])], true).unwrap();
        assert!(one.truncated);
        let none = build_report("a", "en", vec![], false).unwrap();
        assert!(none.issues.is_empty() && !none.truncated);
    }

    #[test]
    fn build_report_rejects_bad_or_overlapping_ranges() {
        for bad in [
            vec![raw(0, 0, &[])],
            vec![raw(9, 1, &[])],
            vec![raw(0, 3, &[]), raw(2, 1, &[])],
        ] {
            assert!(matches!(
                build_report("abcd", "en", bad, false),
                Err(DeviceError::Internal("spell_check range"))
            ));
        }
    }

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let e = UnsupportedSpellChecker
            .check("x", "en", SPELL_CHECK_DEADLINE)
            .unwrap_err();
        assert!(matches!(e, DeviceError::Unsupported("spell_check")));
    }

    #[test]
    fn debug_hides_words_and_suggestions() {
        let r = build_report("secretword", "en", vec![raw(0, 10, &["hidden"])], false).unwrap();
        let d = format!("{r:?} {:?}", raw(0, 1, &["hidden"]));
        assert!(!d.contains("secretword") && !d.contains("hidden"), "{d}");
    }
}
