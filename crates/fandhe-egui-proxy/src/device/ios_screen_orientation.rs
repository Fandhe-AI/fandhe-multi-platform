//! iOS 向け画面向き実装のスタブ（Issue #655、親 #128）。`UnsupportedScreenOrientation` を返す
//! だけで、成功を装わない。dispatch（`device/screen_orientation.rs`）・共通コア画面・trait 契約
//! （`fandhe_device::screen_orientation`）は (a) で最終形にしてあり、後続の #656 は本ファイルの
//! `implementation()` の中身と OS の付帯物だけを埋める（dispatch・画面・fallback 行・
//! `device.rs` の `pub mod` 列は触らない）。
//!
//! # 後続 #656 が埋める範囲
//!
//! - `ios/fandhe_ios_screen_orientation.m` と `build.rs` への追加
//! - `WinitUIViewController` の `supportedInterfaceOrientations` の差し替えと
//!   `setNeedsUpdateOfSupportedInterfaceOrientations`・`requestGeometryUpdate`
//! - 読み取りは `effectiveGeometry.interfaceOrientation`。iPad は `can_lock = false`（固定は `Unsupported`）
//! - main スレッドで扱い、OS コールバック後に `crate::ios::request_repaint` を呼ぶこと
//! - 整数コードは `fandhe_device::screen_orientation::parse_reading` で検証すること

use std::sync::Arc;

use fandhe_device::screen_orientation::{ScreenOrientation, UnsupportedScreenOrientation};

/// `device/screen_orientation.rs` の iOS 経路から呼ばれる。#656 までは非対応を返す。
pub(crate) fn implementation() -> Arc<dyn ScreenOrientation> {
    Arc::new(UnsupportedScreenOrientation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::screen_orientation::Orientation;

    #[test]
    fn stub_never_succeeds() {
        let imp = implementation();
        assert!(imp.read().is_err());
        assert!(imp.request_lock(Some(Orientation::Landscape)).is_err());
        assert!(imp.request_lock(None).is_err());
        assert!(!imp.can_lock());
    }
}
