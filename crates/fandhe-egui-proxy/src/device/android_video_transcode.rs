//! Android 向け動画の変換のスタブ（Issue #1014 (a)。実装は #1017 b-android）。
//! `device/video_transcode.rs::platform` の Android 経路から呼ばれ、いまは `UnsupportedVideoTranscode` を返す
//! （ダミーの成功へは倒さない）。
//!
//! #1017 の申し送り:
//! - `VideoTranscodeHelper.java` と JNI 署名を Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - `MediaCodecInfo.VideoCapabilities.isSizeSupported` が偽なら `Unsupported`（事前の機種判定を置かない）
//! - YUV は `Image` の `pixelStride` / `rowStride` を尊重する
//! - 全経路（成功・失敗・期限切れ）で codec・muxer・一時ファイルを解放する
//! - `reset_pending_tracking` の配線（`android.rs`）は (a) で済んでいる

use std::sync::Arc;

use fandhe_device::video_transcode::{UnsupportedVideoTranscode, VideoTranscode};

/// Android の `VideoTranscode` 実装を返す（#1017 まではスタブ）。
pub(crate) fn implementation() -> Arc<dyn VideoTranscode> {
    Arc::new(UnsupportedVideoTranscode)
}
