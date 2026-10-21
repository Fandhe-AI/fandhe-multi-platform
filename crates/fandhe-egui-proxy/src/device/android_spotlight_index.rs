//! Android 向け OS の検索への登録（Issue #562 b-android で埋めるスタブ）。`device/spotlight_index.rs` の
//! Android 経路から呼ばれる。いまは `UnsupportedSpotlightIndex` を返し、成功を装わない。
//!
//! #562 が埋める範囲: AppSearch（API 31+、API 31 未満は `Unsupported`）の Java ヘルパーと JNI、
//! `implementation()` の差し替え。Android は受信経路を持たない。dispatch・画面・文言は触らない。

use std::sync::Arc;

use fandhe_device::spotlight_index::{SpotlightIndex, UnsupportedSpotlightIndex};

/// Android の `SpotlightIndex` 実装を返す（#562 まで非対応）。
pub(crate) fn implementation() -> Arc<dyn SpotlightIndex> {
    Arc::new(UnsupportedSpotlightIndex)
}
