//! Windows 向けスペルチェック実装のスタブ（Issue #578 で置き、実体は #581 が埋める。親 #119）。
//! `device/spell_check.rs::platform` の Windows 経路から呼ばれる。実体が入るまでは `Unsupported` を返し、
//! ダミーの成功実装へは倒さない。
//!
//! #581 が埋める範囲: `ISpellCheckerFactory` / `ISpellChecker`（spellcheck.h、Windows 8+、package
//! identity 不要）。out-of-proc で RPC 待ちが起きるためワーカーで呼ぶ。返る文字列は `CoTaskMemFree` で
//! 解放する。`windows` crate の feature `Win32_Globalization` の追加も #581 の担当（#578 では依存を
//! 変えない）。重複語（`CORRECTIVE_ACTION_DELETE`）は範囲外。
//!
//! dispatch・画面・`registry.rs` の行・`device.rs` の `pub mod` 列は #578 で最終形になっているため
//! 触らない（このファイルと `Cargo.toml` の feature だけを足す）。

use std::sync::Arc;

use fandhe_device::spell_check::{SpellChecker, UnsupportedSpellChecker};

/// Windows の `SpellChecker` 実装を返す（実体は #581）。
pub(crate) fn implementation() -> Arc<dyn SpellChecker> {
    Arc::new(UnsupportedSpellChecker)
}
