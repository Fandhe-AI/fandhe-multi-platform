//! Android 向け動画・音声の変換の骨格（Issue #311 で置いたスタブ。実装は b-android の子 issue）。
//! `device/media_transcode.rs` の Android 経路（dispatch は #311 で最終形）から呼ばれる。
//! b-android の子 issue が MediaCodec + MediaMuxer の実装で `implementation` の中身を置き換える。
//! dispatch・画面・文言は触らない。
//!
//! 現状は `Unsupported` を返す（嘘の成功にならない）。

use std::sync::Arc;

use fandhe_device::media_transcode::{MediaTranscode, UnsupportedMediaTranscode};

/// Android の `MediaTranscode` 実装を返す。b-android の子 issue まで非対応。
pub(crate) fn implementation() -> Arc<dyn MediaTranscode> {
    Arc::new(UnsupportedMediaTranscode)
}
