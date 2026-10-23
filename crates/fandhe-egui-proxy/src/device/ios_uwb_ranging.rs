//! iOS 向け UWB 測距実装のスタブ（Issue #653、親 #129）。`device/uwb_ranging.rs` の iOS 経路の
//! dispatch 先で、後続の子 #654 が実体を入れるまで全メソッドで `UnsupportedOnOs` を返す
//! （ダミーの成功値へは倒さない）。
//!
//! 後続が埋める範囲: `NISession`（距離。方向は `deviceCapabilities` が使える iOS 16+）と
//! `MultipeerConnectivity`（token の交換）。MPC・`NISession` の delegate、背景遷移、`start` / `stop` は
//! 1 本の serial queue に集め、受け取った token は `is_valid_token_len` で長さを見てから
//! `NIDiscoveryToken` 1 クラスだけを許す secure coding で解凍する。コールバックで共有状態を更新した直後に
//! `crate::ios::request_repaint` を呼ぶ。
//! 後続は本ファイル内の `#[cfg(target_os = "ios")]` の項目と OS 固有の付帯物（`.m`・build.rs・Info.plist）だけを触る。
//! dispatch・画面・fallback 行・`device.rs` の `pub mod` 列は触らない。

use std::sync::Arc;

use fandhe_device::uwb_ranging::{UnsupportedUwbRanging, UwbRanging};

/// UWB 測距の実装を返す（実体は #654）。
pub(crate) fn implementation() -> Arc<dyn UwbRanging> {
    Arc::new(UnsupportedUwbRanging)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::uwb_ranging::UwbFailure;

    #[test]
    fn stub_never_succeeds() {
        let imp = implementation();
        assert_eq!(imp.capabilities(), Err(UwbFailure::UnsupportedOnOs));
        assert_eq!(imp.start(), Err(UwbFailure::UnsupportedOnOs));
        assert_eq!(imp.read(), Err(UwbFailure::UnsupportedOnOs));
    }
}
