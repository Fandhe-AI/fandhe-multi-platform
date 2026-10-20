//! Android 向け近くの端末との直接通信のスタブ（Issue #511 の a。b-android が埋める）。
//!
//! 呼び出し元: `device/local_peer.rs` の Android 経路が `present` を呼ぶ。現状は `Unsupported` を
//! 返すだけで、`present` が失敗すると dispatch が同期失敗として確定する（成功を偽装しない）。
//! 契約の SSOT は `fandhe_device::local_peer`。
//!
//! b-android が埋める範囲（このファイルの中身と Java 側だけ。dispatch・画面・fallback 行には触らない）:
//! - `LocalPeerHelper.java`（Wi-Fi Aware。main Handler 1 本で窓を測る）と JNI エクスポート
//!   （結果は `PENDING.offer` / `finish` へ届ける）
//! - Manifest の権限

use std::time::Duration;

use fandhe_device::local_peer::{LocalPeer, UnsupportedLocalPeer};
use fandhe_device::DeviceError;

/// 交換を依頼する。b-android が実装するまでは `Unsupported`。
pub(crate) fn present(
    _request_id: u64,
    _own_name: &str,
    _outgoing: &str,
    _window: Duration,
) -> Result<(), DeviceError> {
    UnsupportedLocalPeer.exchange("").map(|_| ())
}
