//! iOS 向けバックグラウンド送信実装の骨格（Issue #835 の a が置き、Issue #836 の b-ios が埋める）。
//! `device/background_upload.rs::implementation` の iOS 経路から呼ばれる。現状は `Unsupported` を返す
//! 薄いスタブで、ダミーの成功にはしない。対応表の iOS セルは `possible` のまま（`impl` に書かない）。
//!
//! # b-ios への申し送り
//!
//! - `apple_background_upload::instance()` へ委譲する（macOS と共用の background `NSURLSession`）
//! - `apple/fandhe_apple_background_upload.m` の `TARGET_OS_IOS` 区画に `handle_events` を置き、
//!   `ios/fandhe_ios_url_scheme.m` の application delegate から identifier によって振り分ける。
//!   background-download の受け口を壊さない
//! - 背景起動の完了通知 handler は 1 回だけ呼ぶ
//! - OS コールバックの後に `crate::ios::request_repaint` を呼ぶ（入力が無いと egui が描画しない。契約表 4）
//! - `UIBackgroundModes` は足さない。スワイプ終了による取り消しは OS が行う（守らない範囲）

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::background_upload::{BackgroundUpload, UnsupportedBackgroundUpload};

/// iOS の実装を返す。b-ios が実装するまでは `Unsupported`。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn BackgroundUpload> {
    Arc::new(UnsupportedBackgroundUpload)
}
