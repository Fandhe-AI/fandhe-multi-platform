//! Windows 向け動画の変換のスタブ（Issue #1014 (a)。実装は #1015）。
//! `device/video_transcode.rs::platform` の Windows 経路から呼ばれ、いまは `UnsupportedVideoTranscode` を返す
//! （ダミーの成功へは倒さない）。
//!
//! #1015 の申し送り:
//! - 2 段で行う: `MediaStreamSource` + `PrepareMediaStreamSourceTranscodeAsync` で元動画を一時ファイルへ、
//!   `PrepareStreamTranscodeAsync` で小さい動画へ
//! - 一時ファイルは `video_transcode::temp_pair` と `LEFTOVERS`（`release` / `ensure_released`）を使う。
//!   使い始めるとき、`video_transcode.rs` の `Leftovers` 系の `allow(dead_code)` の cfg に windows を加える
//! - `map_err` は `windows_media_transcode` と同じ写像表（N エディションは `Unsupported`）
//! - `Cancel()` 後は `CANCEL_GRACE` だけ完了を待ち、確認できなければ未解放として記録する
//! - 元フレームは `fandhe_device::video_transcode::fill_source_frame_bgra` で作る

use std::sync::Arc;

use fandhe_device::video_transcode::{UnsupportedVideoTranscode, VideoTranscode};

/// Windows の `VideoTranscode` 実装を返す（#1015 まではスタブ）。
pub(crate) fn implementation() -> Arc<dyn VideoTranscode> {
    Arc::new(UnsupportedVideoTranscode)
}
