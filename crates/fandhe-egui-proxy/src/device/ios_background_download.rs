//! iOS 向けバックグラウンドダウンロードのスタブ（Issue #390。親 #100 の b-ios #391 が埋める）。
//! `device/background_download.rs::implementation` の iOS 経路から呼ばれ、現状は `Unsupported`
//! （成功を装わない）。`apple_background_download`（macOS と共用の background `NSURLSession`、`build.rs`
//! は iOS 分岐でも `.m` をコンパイル済み）へ委譲するのが b-ios の仕事で、dispatch・`build.rs`・
//! `device.rs` の `pub mod` 列は触らずに済む。
//!
//! # b-ios への申し送り
//!
//! - `Info.plist` に `NSAllowsLocalNetworking` が要る。`scripts/ios/verify-device-launch.sh` の
//!   `plutil -insert` と衝突しうるので、キーの有無を見てから足す
//! - アプリが再起動・バックグラウンド起動されたら `handleEventsForBackgroundURLSession` で
//!   同じ identifier の session へ再接続する。[`register`] は `UIApplicationMain` より前に呼ばれる
//! - 完了通知で `crate::ios::request_repaint` を呼び次フレームを起こす（契約表 4）
//! - 利用者がマルチタスク画面から終了すると転送は取り消される（OS の挙動。doc に書く）

#[cfg(any(target_os = "ios", test))]
use std::sync::Arc;

#[cfg(any(target_os = "ios", test))]
use fandhe_device::background_download::{BackgroundDownload, UnsupportedBackgroundDownload};

/// iOS の実装を返す。b-ios が差し替えるまでは `Unsupported`。
#[cfg(any(target_os = "ios", test))]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    Arc::new(UnsupportedBackgroundDownload)
}

/// `main()` が `UIApplicationMain` より前に呼ぶ登録口。b-ios が中身を埋める。
#[cfg(any(target_os = "ios", test))]
pub fn register() {}
