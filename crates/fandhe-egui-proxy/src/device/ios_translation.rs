//! iOS 向け端末内翻訳のスタブ（Issue #535 a。実体は #536 b-ios が埋める）。
//! `device/translation.rs` の iOS 経路から呼ばれる。現状は `Unsupported` を返すだけで、ダミーの
//! 成功へは倒さない。
//!
//! #536 が埋める範囲: `implementation()` を `apple_translation::AppleTextTranslator` へ差し替え、
//! `device.rs` の `apple_translation` の `allow(dead_code)` を外す。`crate::ios::request_repaint` の
//! 要否は `ios_on_device_llm` と同じ判断（結果は `CompletionSender::send` で届き次フレームが起きる）。
//! dispatch・画面・fallback 行は触らない。契約は `fandhe_device::translation` を正とする。

use std::sync::Arc;

use fandhe_device::translation::{TextTranslator, UnsupportedTextTranslator};

/// iOS の `TextTranslator` 実装を返す（#536 まで `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn TextTranslator> {
    Arc::new(UnsupportedTextTranslator)
}
