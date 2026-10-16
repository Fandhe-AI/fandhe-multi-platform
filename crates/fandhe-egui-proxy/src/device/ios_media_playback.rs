//! iOS 向け音声再生実装のスタブ（Issue #267 で枠、親 #86 の b-ios が埋める）。
//! `device/media_playback.rs::platform_playback` の iOS 経路から呼ばれる。
//! 埋めるときは `apple_media_playback` を使い、背景遷移・割り込みでの `Interrupted` と
//! `crate::ios::request_repaint` を足す。それまでは `Unsupported` を返し、成功値へは倒さない。

use std::sync::Arc;

use fandhe_device::media_playback::{MediaPlayback, UnsupportedMediaPlayback};

/// iOS の実装を返す。
pub(crate) fn implementation() -> Arc<dyn MediaPlayback> {
    Arc::new(UnsupportedMediaPlayback)
}
