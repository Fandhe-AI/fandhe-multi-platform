//! Windows 向け音声再生実装のスタブ（Issue #267 で枠、親 #86 の windows が埋める）。
//! `device/media_playback.rs::platform_playback` の Windows 経路から呼ばれる。
//! 埋めるときは `IMFMediaEngine` を呼ぶ。

use std::sync::Arc;

use fandhe_device::media_playback::{MediaPlayback, UnsupportedMediaPlayback};

/// Windows の実装を返す。
pub(crate) fn implementation() -> Arc<dyn MediaPlayback> {
    Arc::new(UnsupportedMediaPlayback)
}
