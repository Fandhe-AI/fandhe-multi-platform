//! Windows 向け LAN のサービス探索のスタブ（Issue #486。親 #109 の windows の子）。
//!
//! 呼び出し元: `device/lan_discovery.rs` の Windows 経路が `present` を呼ぶ。windows の子は
//! `DnsServiceBrowse` で埋めて、このファイルの中身だけを置き換える。dispatch・画面・文言・
//! `device.rs` の `pub mod` 列・対応表の他 OS のセルには触らない。
//!
//! 実装されるまでは `Unsupported` を返す。ダミーの成功（0 件の `Ok`）は返さない
//! （`device-bridge-contracts.md` 6 節）。契約の SSOT は `fandhe_device::lan_discovery`。

use std::time::Duration;

use fandhe_device::lan_discovery::{LanDiscovery, UnsupportedLanDiscovery};
use fandhe_device::DeviceError;

/// 探索を依頼する。完了は `device/lan_discovery.rs` の `OBSERVED` / `finish` へ届ける契約。
/// スタブは依頼を受けず、常に `Unsupported` を返す。
pub(crate) fn present(_request_id: u64, _duration: Duration) -> Result<(), DeviceError> {
    UnsupportedLanDiscovery.browse().map(|_| ())
}

/// アプリ終了時に進行中の探索を止める（`device::shutdown` 経由。スタブでは何もしない）。
pub(crate) fn stop_shared() {}
