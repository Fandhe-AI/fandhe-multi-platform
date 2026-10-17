//! iOS 向けピクチャ・イン・ピクチャ実装のスタブ（Issue #364。親 #97 の b-ios が埋める）。
//! `device/picture_in_picture.rs::platform_pip` の iOS 経路から呼ばれ、現状は `Unsupported` を返す。
//! 埋めるときの注意: macOS と共用の `apple/fandhe_apple_picture_in_picture.m` を使い、`Info.plist` の
//! `UIBackgroundModes` に `audio` を足す（背景での音声再生には使わない）。OS コールバックの直後は
//! `crate::ios::request_repaint` で次フレームを起こす。小窓中の背景遷移は中断にしない。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::picture_in_picture::{PictureInPicture, UnsupportedPictureInPicture};

/// iOS の実装を返す（現状は非対応）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn PictureInPicture> {
    Arc::new(UnsupportedPictureInPicture)
}
