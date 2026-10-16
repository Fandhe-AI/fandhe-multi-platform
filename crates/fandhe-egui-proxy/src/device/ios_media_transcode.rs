//! iOS 向け動画・音声の変換実装（Issue #313 b-ios）。
//! `device/media_transcode.rs` の iOS 経路から呼ばれ、macOS と共用の `AVAudioFile` 実装
//! （`apple_media_transcode::AppleMediaTranscode`）を返す。dispatch・画面・文言は触らない。
//!
//! 契約表 4（再描画）: 結果はワーカーから `CompletionSender::send` で届き、`send` が同じ
//! `egui::Context` の `request_repaint` を呼ぶため、入力が無くても次フレームが起きる。Rust の共有状態へ
//! 書き込む OS コールバック（delegate・通知）は無く（`.m` の `on_output` は同期呼び出しの中で
//! `Vec` へコピーするだけ）、`crate::ios::request_repaint` を別に呼ぶ必要はない。
//!
//! 契約表 1（ライフサイクル）: 背景遷移で中断しない。suspend をはさんで期限を過ぎた場合はドライバが
//! 期限切れの失敗で確定し、遅れて届いた結果は世代照合で捨てる。
//! 契約表 5（FFI）: 呼び出しは Rust のワーカーで行い、`.m` 側は `@autoreleasepool` の内側。UI を要さないため
//! main queue は使わない。守る範囲・守らない範囲は `fandhe_device::media_transcode` の trait doc を正とする。

use std::sync::Arc;

use fandhe_device::media_transcode::MediaTranscode;

/// iOS の `MediaTranscode` 実装を返す。deploymentTarget が iOS 17 のため OS 版による `Unsupported` は起きない。
pub(crate) fn implementation() -> Arc<dyn MediaTranscode> {
    Arc::new(super::apple_media_transcode::AppleMediaTranscode)
}
