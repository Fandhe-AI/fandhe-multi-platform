//! iOS 向けセーフエリア実装のスタブ（Issue #444、親 #105）。常に `Unsupported` を返す。
//! `device/safe_area_insets.rs` の iOS 経路から呼ばれ、契約は
//! `fandhe_device::safe_area_insets`。
//!
//! 兄弟 issue（b-ios）がこのファイルの中身だけを埋める。埋める側の契約: `ios/` に
//! `int32_t fandhe_ios_safe_area_insets_read(double out[4])` を 1 関数置き（main スレッドで
//! key window の `safeAreaInsets` を `{top, right, bottom, left}` の順に書いて 0、取れなければ
//! -1、この OS 版で取れなければ -2）、`fandhe_device::safe_area_insets::insets_from_raw` で写す。
//! dispatch（`device/safe_area_insets.rs`）は変えない。

use std::sync::Arc;

use fandhe_device::safe_area_insets::{SafeAreaSource, UnsupportedSafeArea};

/// iOS の実装を返す（b-ios が実体に差し替える）。
pub(crate) fn implementation() -> Arc<dyn SafeAreaSource> {
    Arc::new(UnsupportedSafeArea)
}
