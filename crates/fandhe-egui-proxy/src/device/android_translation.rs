//! Android 向け端末内翻訳のスタブ（Issue #535 a。実体は #537 b-android が埋める）。
//! `device/translation.rs` の Android 経路から呼ばれる。現状は `Unsupported` を返すだけで、ダミーの
//! 成功へは倒さない。
//!
//! #537 が埋める範囲: `TranslationHelper.java`（`android.view.translation`、API 31 以上）と JNI。
//! `MainActivity` は委譲 1 メソッド。API 31 未満は `Unsupported`。dispatch・画面・fallback 行は
//! 触らない。契約は `fandhe_device::translation` を正とする。

use std::sync::Arc;

use fandhe_device::translation::{TextTranslator, UnsupportedTextTranslator};

/// Android の `TextTranslator` 実装を返す（#537 まで `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn TextTranslator> {
    Arc::new(UnsupportedTextTranslator)
}
