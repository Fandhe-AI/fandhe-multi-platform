//! iOS 向け近くの端末との直接通信のスタブ（Issue #511 の a。b-ios が埋める）。
//!
//! 呼び出し元: `device/local_peer.rs` の iOS 経路が `present` を呼ぶ。現状は `Unsupported` を
//! 返すだけで、`present` が失敗すると dispatch が同期失敗として確定する（成功を偽装しない）。
//! 契約の SSOT は `fandhe_device::local_peer`。
//!
//! b-ios が埋める範囲（このファイルの中身だけ。dispatch・画面・fallback 行には触らない）:
//! - `apple_local_peer::present` への委譲（macOS と共用の MultipeerConnectivity `.m`）
//! - Info.plist の `NSBonjourServices`（`_fandhe-p2p._tcp` と `_fandhe-p2p._udp`）と
//!   `NSLocalNetworkUsageDescription`
//! - 背景遷移したら `Internal` で確定する中断処理
//! - `device.rs` の `apple_local_peer` にある `cfg_attr(ios, allow(dead_code))` を外す

use std::time::Duration;

use fandhe_device::local_peer::{LocalPeer, UnsupportedLocalPeer};
use fandhe_device::DeviceError;

/// 交換を依頼する。b-ios が実装するまでは `Unsupported`。
pub(crate) fn present(
    _request_id: u64,
    _own_name: &str,
    _outgoing: &str,
    _window: Duration,
) -> Result<(), DeviceError> {
    UnsupportedLocalPeer.exchange("").map(|_| ())
}
