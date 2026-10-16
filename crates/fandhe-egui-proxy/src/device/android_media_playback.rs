//! Android 向け音声再生実装のスタブ（Issue #267 で枠、親 #86 の b-android が埋める）。
//! `device/media_playback.rs::platform_playback` の Android 経路から呼ばれる。
//! 埋めるときは `MediaPlayer` + `MediaDataSource` を JNI で呼び、`onPause` で `Interrupted`、再生成で release する。

use std::sync::Arc;

use fandhe_device::media_playback::{MediaPlayback, UnsupportedMediaPlayback};

/// Android の実装を返す。
pub(crate) fn implementation() -> Arc<dyn MediaPlayback> {
    Arc::new(UnsupportedMediaPlayback)
}
