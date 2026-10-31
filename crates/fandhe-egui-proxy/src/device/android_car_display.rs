//! Android 向け車載ディスプレイ実装の骨格（Issue #1116、親 #152）。
//! `device/car_display.rs` の Android 経路から呼ばれる。実体は #1117 が置き換える。
//!
//! 現状は [`UnsupportedCarDisplay`] を返すだけ（成功を装うダミーにはしない）。
//! #1117 が埋める範囲は次のとおりで、dispatch・共通コア画面・`device.rs` の `pub mod` は触らない。
//!
//! - `with_activity`（`android_home_widget` と同形・`// SAFETY:` 付き）
//! - JNI の受け口 `carDisplaySetText(String)` / `carDisplayIsShowing()`
//! - 純関数 `state_from(Option<bool>)`（`None` を `Internal` にする）
//! - Activity の不在は `Internal`（`Unsupported` にするとドライバがポーリングをやめるため）
//! - Java（`CarAppService`・`Session`・`Screen`）・manifest・res・対応表 JSON の android セル

use std::sync::Arc;

use fandhe_device::car_display::{CarDisplay, UnsupportedCarDisplay};

/// Android の `CarDisplay` 実装（骨格）。
pub(crate) fn implementation() -> Arc<dyn CarDisplay> {
    Arc::new(UnsupportedCarDisplay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_never_succeeds() {
        let d = implementation();
        assert!(matches!(
            d.set_text("x"),
            Err(DeviceError::Unsupported("car_display"))
        ));
        assert!(matches!(
            d.state(),
            Err(DeviceError::Unsupported("car_display"))
        ));
    }
}
