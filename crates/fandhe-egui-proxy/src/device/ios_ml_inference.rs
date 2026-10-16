//! iOS 向け機械学習推論実装（Issue #291 b-ios）。
//! `device/ml_inference.rs` の iOS 経路から呼ばれ、macOS と共用の Core ML 実装
//! （`apple_ml_inference::AppleMlInference`）を返す。dispatch・画面・文言は触らない。
//!
//! 契約表 4（再描画）: 結果はワーカーから `CompletionSender::send` で届き、`send` が同じ
//! `egui::Context` の `request_repaint` を呼ぶため、入力が無くても次フレームが起きる。Rust の共有状態へ
//! 書き込む OS コールバック（delegate・通知）は無く（`.m` の完了ブロックは同期呼び出しの中で
//! セマフォを鳴らすだけ）、`crate::ios::request_repaint` を別に呼ぶ必要はない。
//!
//! 契約表 1（ライフサイクル）: 背景遷移で中断しない。計算ユニットは CPU 固定で、背景でも GPU 制約を受けない。
//! suspend をはさんで期限を過ぎた場合はドライバが期限切れの失敗で確定し、遅れて届いた結果は世代照合で捨てる。
//! 守る範囲・守らない範囲は `fandhe_device::ml_inference` の trait doc を正とする。

use std::sync::Arc;

use fandhe_device::ml_inference::MlInference;

/// iOS の `MlInference` 実装を返す。deploymentTarget が iOS 17 のため OS 版による `Unsupported` は起きない。
pub(crate) fn implementation() -> Arc<dyn MlInference> {
    Arc::new(super::apple_ml_inference::AppleMlInference)
}
