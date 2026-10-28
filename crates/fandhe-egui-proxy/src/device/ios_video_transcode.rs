//! iOS 向け動画の変換のスタブ（Issue #1014 (a)。実装は #1016 b-ios）。
//! `device/video_transcode.rs::platform` の iOS 経路から呼ばれ、いまは `UnsupportedVideoTranscode` を返す
//! （ダミーの成功へは倒さない）。
//!
//! #1016 の申し送り:
//! - `apple_video_transcode::AppleVideoTranscode`（macOS と共用）を返すよう `implementation()` を差し替える。
//!   `apple_video_transcode` の `allow(dead_code)`（`device.rs`）も同時に外す
//! - 契約表 4: ワーカーの完了は `CompletionSender::send` が再描画を起こす経路か確かめる
//! - 背景遷移では中断しない（契約表 1。期限切れはドライバが確定し、遅れて届いた結果は世代照合で捨てる）
//! - `build.rs`（`.m` は iOS 分岐でもコンパイル済み）・dispatch・画面は触らない

use std::sync::Arc;

use fandhe_device::video_transcode::{UnsupportedVideoTranscode, VideoTranscode};

/// iOS の `VideoTranscode` 実装を返す（#1016 まではスタブ）。
pub(crate) fn implementation() -> Arc<dyn VideoTranscode> {
    Arc::new(UnsupportedVideoTranscode)
}
