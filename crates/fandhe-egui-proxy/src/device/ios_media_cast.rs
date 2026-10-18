//! iOS 向け外部機器への再生（Issue #386 の骨格。実装は親 #99 の b-ios）。
//! `device/media_cast.rs::platform_cast` の iOS 経路から呼ばれる。現状は `UnsupportedMediaCast` を返す
//! スタブで、対応表の iOS セルは `possible` のまま（`impl` に書かない）。
//!
//! # b-ios が埋めるときの注意
//!
//! - macOS と共用の `apple/fandhe_apple_media_cast.m` を `TARGET_OS_IOS` で足し、`apple_media_cast` の cfg を広げる
//! - `AVAudioSession`（playback・longFormAudio）の category / mode / options を保存し、失敗を含む全経路で復元する
//! - 背景遷移で止めて `CastPlayback::Interrupted` にする（`UIBackgroundModes` の `audio` は使わない）
//! - route change を受けたら `crate::ios::request_repaint` で次フレームを起こす（契約表 4）

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::media_cast::{MediaCast, UnsupportedMediaCast};

/// iOS の実装を返す（b-ios まではスタブ）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn MediaCast> {
    Arc::new(UnsupportedMediaCast)
}
