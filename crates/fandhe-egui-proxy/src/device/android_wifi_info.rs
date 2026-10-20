//! Android 向け Wi-Fi 情報実装のスタブ（Issue #514 が骨格を置き、b-android 子がこのファイルだけを埋める）。
//! `device/wifi_info.rs::implementation` の Android 経路から呼ばれる。契約の SSOT は
//! `fandhe_device::wifi_info`。埋めるまでは `Unsupported` を返し、成功を偽らない。

use std::sync::Arc;

use fandhe_device::wifi_info::{UnsupportedWifiInfo, WifiInfo};

/// Android の `WifiInfo` 実装を返す（実装まではスタブ）。
pub(crate) fn implementation() -> Arc<dyn WifiInfo> {
    Arc::new(UnsupportedWifiInfo)
}
