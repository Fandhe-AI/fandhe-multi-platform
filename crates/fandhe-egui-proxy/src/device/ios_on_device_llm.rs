//! iOS 向け端末内 LLM 実装（Issue #346 b-ios）。
//! `device/on_device_llm.rs` の iOS 経路から呼ばれ、macOS と共用の FoundationModels 実装
//! （`apple_on_device_llm::AppleOnDeviceLlm`）を返す。dispatch・画面・文言は触らない。
//!
//! 契約表 4（再描画）: 結果はワーカーから `CompletionSender::send` で届き、`send` が同じ
//! `egui::Context` の `request_repaint` を呼ぶため、入力が無くても次フレームが起きる。Rust の共有状態へ
//! 書き込む OS コールバックは無く（Swift の完了は同期呼び出しの中でセマフォを鳴らすだけ）、
//! `crate::ios::request_repaint` を別に呼ぶ必要はない。
//!
//! 契約表 1（ライフサイクル）: 背景遷移で `rateLimited` になった場合は混雑の失敗として出す。
//! suspend をはさんで期限を過ぎた場合はドライバが期限切れの失敗で確定し、遅れて届いた結果は
//! 世代照合で捨てる。iOS 26 未満は `Unsupported`（Swift の `#available` で判定）。
//! 守る範囲・守らない範囲は `fandhe_device::on_device_llm` の trait doc を正とする。

use std::sync::Arc;

use fandhe_device::on_device_llm::OnDeviceLlm;

/// iOS の `OnDeviceLlm` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn OnDeviceLlm> {
    Arc::new(super::apple_on_device_llm::AppleOnDeviceLlm)
}
