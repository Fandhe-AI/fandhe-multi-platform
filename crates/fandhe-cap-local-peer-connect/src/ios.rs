//! iOS 向け近くの端末との直接通信（Issue #512。親 #110 の b-ios）。
//!
//! 呼び出し元: `driver.rs` の iOS 経路が `present` を呼ぶ。実体は macOS と共用の
//! `apple.rs`（MultipeerConnectivity の `.m`）へ委譲するだけで、結果の通知
//! （`fandhe_runtime::request_repaint` を含む）・窓の計時・エラー写像・背景遷移の中断
//! （`DidEnterBackground` で `Internal` 確定）は委譲先の `.m` が担う。
//! 契約の SSOT は `contract.rs`。

use std::time::Duration;

use crate::error::LocalPeerConnectError;

/// 交換を依頼する。完了は `driver.rs` の `Pending` へ届く。
pub(crate) fn present(
    request_id: u64,
    own_name: &str,
    outgoing: &str,
    window: Duration,
) -> Result<(), LocalPeerConnectError> {
    crate::apple::present(request_id, own_name, outgoing, window)
}
