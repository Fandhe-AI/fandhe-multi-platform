//! Android 向けピクチャ・イン・ピクチャ実装のスタブ（Issue #364。親 #97 の b-android が埋める）。
//! `device/picture_in_picture.rs::platform_pip` の Android 経路から呼ばれ、現状は `Unsupported` を返す。
//! 埋めるときの注意: manifest に `supportsPictureInPicture` を宣言し、`FEATURE_PICTURE_IN_PICTURE` の
//! 有無を見て非対応を返す。`onPictureInPictureModeChanged` で状態を更新し、小窓に入るときの `onPause`
//! を中断として扱わない。Activity の作り直しは `reset_pending_tracking` が受ける。

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::picture_in_picture::{PictureInPicture, UnsupportedPictureInPicture};

/// Android の実装を返す（現状は非対応）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn PictureInPicture> {
    Arc::new(UnsupportedPictureInPicture)
}
