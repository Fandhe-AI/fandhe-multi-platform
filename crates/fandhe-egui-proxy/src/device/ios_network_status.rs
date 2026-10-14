//! iOS 向けネットワーク接続状態実装（Issue #1282 で枠、#1283 で実装）。
//! `device/network_status.rs::implementation` の iOS 経路から呼ばれ、Apple 共用の
//! `apple_network_status::AppleNetworkStatus`（`NWPathMonitor`）を返す。
//! `ios_screen_wake_lock.rs` と同様、Linux ホストの `cargo test` では `AppleNetworkStatus` が
//! 存在しないため、関数と import は iOS 限定にしている。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::network_status::NetworkStatus;

/// iOS の実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn NetworkStatus> {
    Arc::new(super::apple_network_status::AppleNetworkStatus)
}
