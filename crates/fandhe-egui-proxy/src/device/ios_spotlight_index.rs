//! iOS 向け OS の検索への登録（Issue #561 b-ios で埋めるスタブ）。`device/spotlight_index.rs` の
//! iOS 経路から呼ばれる。いまは `UnsupportedSpotlightIndex` を返し、成功を装わない。
//!
//! #561 が埋める範囲: `apple_spotlight_index::implementation()` への差し替え、
//! `ios/fandhe_ios_scene.m` の `continueUserActivity`（`fandhe_rust_url_scheme_received` へ
//! `fandhe://spotlight/<id>` を渡す）、`apple_spotlight_index.rs` 先頭の `allow(dead_code)` の解除。
//! dispatch・画面・文言は触らない。

use std::sync::Arc;

use fandhe_device::spotlight_index::{SpotlightIndex, UnsupportedSpotlightIndex};

/// iOS の `SpotlightIndex` 実装を返す（#561 まで非対応）。
pub(crate) fn implementation() -> Arc<dyn SpotlightIndex> {
    Arc::new(UnsupportedSpotlightIndex)
}
