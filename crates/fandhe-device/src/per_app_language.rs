//! Phase 11 の拡充ループ（親 #137、Issue #1073）: アプリ別の言語設定（アプリが使う言語のタグと
//! 設定の出どころ）を 1 回読む契約。`fandhe-egui-proxy` の `device/per_app_language.rs` が
//! 「言語を読む」ボタン押下時に [`PerAppLanguage::read`] を 1 回だけ呼び、各 OS 実装
//! （macOS: #1073、iOS: #1074、Android: #1075）が本トレイトを実装する。Web・Windows・Linux は
//! [`UnsupportedPerAppLanguage`] 固定。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`AppLanguage`]。`tag` は [`clean_tag`] を通った値
//!   - [`LanguageSource::PerApp`]: Android で `getApplicationLocales()` が空でない（タグはその先頭）
//!   - [`LanguageSource::System`]: Android で `getApplicationLocales()` が空（タグは
//!     `getSystemLocales()` の先頭）
//!   - [`LanguageSource::Unknown`]: iOS / macOS。`Bundle.main.preferredLocalizations` の先頭で、
//!     アプリ別か OS 全体かを区別する公開 API が無い
//! - 非対応: `Err(DeviceError::Unsupported("per_app_language"))`（Android API 32 以下、他 OS）
//! - 失敗: `Err(DeviceError::Internal("per_app_language read"))`（OS 呼び出しの失敗、結果が
//!   nil・空、タグが [`clean_tag`] の検査に外れた場合）
//!
//! キャンセル・権限ダイアログの経路は無い（契約表 10 は該当なし）。
//!
//! # null・空の写像の向き（契約表 6。レビューの指摘に合わせて反転させない）
//!
//! - Android の空の `getApplicationLocales()` は「アプリ別の設定なし」で、成功（`System`）。
//!   公式文書が空の LocaleList を「アプリ別の設定なし」と定めているため
//! - Android の `getSystemLocales()` の空と、Apple の nil・空配列は「取得できなかった」で失敗。
//!   「言語なし」の成功へ畳まない
//!
//! # 呼び出し契約
//!
//! UI スレッドから同期で、ボタン押下ごとに 1 回だけ呼ばれる。前回値は覚えず、押下ごとに OS へ聞く。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 嘘の値を出さない（空・不正・失敗を区別し、非対応を成功にしない）、1 プロセス内の取得、
//! タグの検査（[`clean_tag`]、全 OS 共用で SSOT は本ファイル）。
//! 守らない: 読み取り後の変更の反映（再読込まで）、アプリ自身の文言の切り替え、アプリ内からの
//! 言語変更、Apple でのアプリ別 / OS 全体の区別、宣言（`CFBundleLocalizations` / `localeConfig`）で
//! OS が描く UI の言語が変わること、OS 間のタグ表記の揃い（`ja` と `ja-JP` 等）、複数プロセス、
//! OS 呼び出しが戻らない場合の期限。
//!
//! 並行性: 読み取りは UI スレッドの effect 1 本だけの同期呼び出しで、ワーカー・コールバック・
//! 購読を持たない。集約すべき並行経路は無い。

use crate::DeviceError;

/// タグの長さ上限（BCP 47 の実用上の上限）。
const MAX_TAG_CHARS: usize = 35;

/// 設定の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSource {
    /// アプリ別の設定。
    PerApp,
    /// OS 全体の設定。
    System,
    /// 区別できない（Apple）。
    Unknown,
}

/// アプリが使う言語。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppLanguage {
    /// 言語タグ（[`clean_tag`] 済み。例: `ja`、`zh-Hant`）。
    pub tag: String,
    /// 設定の出どころ。
    pub source: LanguageSource,
}

/// アプリ別の言語設定の読み取り。
pub trait PerAppLanguage: Send + Sync {
    /// アプリが使う言語を 1 回読む。契約はモジュール doc を参照。
    fn read(&self) -> Result<AppLanguage, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported`（ダミーの成功へ倒さない）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedPerAppLanguage;

impl PerAppLanguage for UnsupportedPerAppLanguage {
    fn read(&self) -> Result<AppLanguage, DeviceError> {
        Err(DeviceError::Unsupported("per_app_language"))
    }
}

/// OS が返したタグを検査して正規化する。通すのは ASCII 英数字・`-`・`_` だけ（`_` は `-` へ直す）、
/// 長さ 1〜35。外れたら `None`（呼び出し側は `Internal` にする）。前後の空白は trim せず不正とする。
pub fn clean_tag(raw: &str) -> Option<String> {
    let ok = !raw.is_empty()
        && raw.len() <= MAX_TAG_CHARS
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then(|| raw.replace('_', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedPerAppLanguage.read(),
            Err(DeviceError::Unsupported("per_app_language"))
        ));
    }

    #[test]
    fn clean_tag_accepts_and_normalizes() {
        assert_eq!(clean_tag("ja").as_deref(), Some("ja"));
        assert_eq!(clean_tag("zh_Hant").as_deref(), Some("zh-Hant"));
        assert_eq!(clean_tag(&"a".repeat(35)), Some("a".repeat(35)));
    }

    #[test]
    fn clean_tag_rejects_invalid() {
        for raw in ["", " ", " ja", "ja ", "ja\n", "日本", "ja jp", "a\u{0}b"] {
            assert_eq!(clean_tag(raw), None, "{raw:?}");
        }
        assert_eq!(clean_tag(&"a".repeat(36)), None);
    }
}
