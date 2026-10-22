//! iOS 向けスペルチェック実装のスタブ（Issue #578 で置き、実体は #579 が埋める。親 #119）。
//! `device/spell_check.rs::platform` の iOS 経路から呼ばれる。実体が入るまでは `Unsupported` を返し、
//! ダミーの成功実装へは倒さない。
//!
//! #579 が埋める範囲: UIKit `UITextChecker`（iOS 3.2+、権限・Info.plist・entitlement 不要）。
//! `availableLanguages` で辞書を確かめてから呼ぶ（辞書の無い言語を「誤り 0 件」にしない。
//! `fandhe_device::spell_check::resolve_language`）。クラスが main スレッド専用のため main queue で呼び、
//! 期限切れ後に Rust のバッファへ書かない。結果は `device/spell_check.rs` の
//! `parse_apple_issue_buffer`・`languages_from_nul_buffer`（macOS と共用）で解釈する。
//!
//! dispatch・画面・`registry.rs` の行・`device.rs` の `pub mod` 列は #578 で最終形になっているため
//! 触らない（このファイルと、必要なら `ios/` の `.m` と `build.rs` の iOS 分岐だけを足す）。

use std::sync::Arc;

use fandhe_device::spell_check::{SpellChecker, UnsupportedSpellChecker};

/// iOS の `SpellChecker` 実装を返す（実体は #579）。
pub(crate) fn implementation() -> Arc<dyn SpellChecker> {
    Arc::new(UnsupportedSpellChecker)
}
