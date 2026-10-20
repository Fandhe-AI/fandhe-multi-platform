//! iOS 向け LAN のサービス探索（Issue #488。親 #109 の b-ios）。
//!
//! 呼び出し元: `device/lan_discovery.rs` の iOS 経路が `present` を呼ぶ。実体は macOS と共用の
//! `apple_lan_discovery`（`nw_browser` で発見し dns_sd で解決する `.m`）へ委譲するだけで、
//! 結果の通知（`crate::ios::request_repaint` を含む）・窓の計時・エラー写像はそちらが担う。
//! 契約の SSOT は `fandhe_device::lan_discovery`。

use std::time::Duration;

use fandhe_device::DeviceError;

/// 探索を依頼する。完了は `device/lan_discovery.rs` の `OBSERVED` / `finish` へ届く。
pub(crate) fn present(request_id: u64, duration: Duration) -> Result<(), DeviceError> {
    super::apple_lan_discovery::present(request_id, duration)
}
