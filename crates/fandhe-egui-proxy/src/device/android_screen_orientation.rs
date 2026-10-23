//! Android 向け画面向き実装のスタブ（Issue #655、親 #128）。`UnsupportedScreenOrientation` を返す
//! だけで、成功を装わない。dispatch（`device/screen_orientation.rs`）・共通コア画面・trait 契約
//! （`fandhe_device::screen_orientation`）は (a) で最終形にしてあり、後続の #657 は本ファイルの
//! `implementation()` の中身と OS の付帯物だけを埋める（dispatch・画面・fallback 行・
//! `device.rs` の `pub mod` 列は触らない）。
//!
//! # 後続 #657 が埋める範囲
//!
//! - `ScreenOrientationHelper.java`（`MainActivity` は生成・入口・委譲だけ）
//! - `setRequestedOrientation`（`SENSOR_PORTRAIT`・`SENSOR_LANDSCAPE`・`UNSPECIFIED`）。manifest は変えない
//! - `Display.getRotation` と `Configuration.orientation` の読み取り（API 30 での分岐）
//! - UI スレッドへの post を期限付きで待ち、適用開始と放棄を CAS で排他にすること
//! - JNI 署名を Java・no-op スタブ・実体の 3 か所でそろえること
//! - 整数コードは `fandhe_device::screen_orientation::parse_reading` で検証すること

use std::sync::Arc;

use fandhe_device::screen_orientation::{ScreenOrientation, UnsupportedScreenOrientation};

/// `device/screen_orientation.rs` の Android 経路から呼ばれる。#657 までは非対応を返す。
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
        assert!(imp.request_lock(Some(Orientation::Portrait)).is_err());
        assert!(imp.request_lock(None).is_err());
        assert!(!imp.can_lock());
    }
}
