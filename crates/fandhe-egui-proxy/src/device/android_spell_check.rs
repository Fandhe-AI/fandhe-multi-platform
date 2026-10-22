//! Android 向けスペルチェック実装のスタブ（Issue #578 で置き、実体は #580 が埋める。親 #119）。
//! `device/spell_check.rs::platform` の Android 経路から呼ばれる。実体が入るまでは `Unsupported` を返し、
//! ダミーの成功実装へは倒さない。
//!
//! #580 が埋める範囲: `TextServicesManager.newSpellCheckerSession` +
//! `SpellCheckerSession.getSentenceSuggestions`（API 16、権限・`<queries>` 不要）。`getSuggestions` は
//! 非推奨のため使わない。`newSpellCheckerSession` が null（有効なスペルチェッカーが無い・指定言語の
//! subtype が無い）なら `Unsupported` か `LANGUAGE_UNAVAILABLE` に写す。Looper のあるスレッドで呼び、
//! エラーのコールバックが無いため結果が来ない場合は期限で打ち切る。Activity 再生成の扱いは
//! `device/spell_check.rs::reset_pending_tracking`（`android.rs` から呼び済み）。
//!
//! dispatch・画面・`registry.rs` の行・`device.rs` の `pub mod` 列は #578 で最終形になっているため
//! 触らない（このファイルと Java 側のヘルパーだけを足す）。

use std::sync::Arc;

use fandhe_device::spell_check::{SpellChecker, UnsupportedSpellChecker};

/// Android の `SpellChecker` 実装を返す（実体は #580）。
pub(crate) fn implementation() -> Arc<dyn SpellChecker> {
    Arc::new(UnsupportedSpellChecker)
}
