//! Android 向けセーフエリア実装のスタブ（Issue #444、親 #105）。常に `Unsupported` を返す。
//! `device/safe_area_insets.rs` の Android 経路から呼ばれ、契約は
//! `fandhe_device::safe_area_insets`。
//!
//! 兄弟 issue（b-android）がこのファイルの中身だけを埋める。埋める側の契約: Java が
//! 受信した `systemBars` と `displayCutout` の合計を px で保持し、受信前は `Ok(None)` を返す。
//! px から dp への換算は `ctx.native_pixels_per_point()` で行う。JNI の署名は増やさず
//! 既存の経路に載せる。dispatch（`device/safe_area_insets.rs`）は変えない。

use std::sync::Arc;

use fandhe_device::safe_area_insets::{SafeAreaSource, UnsupportedSafeArea};

/// Android の実装を返す（b-android が実体に差し替える。換算用に `egui::Context` を受ける）。
pub(crate) fn implementation(_ctx: &egui::Context) -> Arc<dyn SafeAreaSource> {
    Arc::new(UnsupportedSafeArea)
}
