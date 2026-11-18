//! location（現在地の取得。位置情報画面と、ジオフェンス画面が共用）の機能 crate（REQ-21 / TASK-21.4、Issue #1332）。
//! iOS・macOS は CoreLocation、Android は `LocationManager`（Play services 不使用）、Windows は PowerShell 子プロセス経由の
//! `Geolocator`。Web は `fandhe-web-dom` の `location.rs` が実装本体を持ち、本 crate の契約を参照する。
//! 呼び出し元は `fandhe-gallery` の画面登録（`location_effect`）と、測位を共用する `fandhe-cap-geofence`
//! （`shared_location_precise`・`restart_location`・`start_apple_location_once`・`to_failure_kind`・`POLL_INTERVAL` を公開している）。
//!
//! 守る範囲: 嘘の成功を出さないこと（権限拒否・サービス無効・未対応を座標へ畳まない）、バックグラウンドでは測位しないこと、
//! 座標をログへ出さないこと、終了時に子プロセス・リスナーを残さないこと。
//! 守らない範囲: 測位精度そのもの・network provider が精度なしの結果を返す場合の欠落、OS 側の測位の非同期確定との時間差。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_location()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;

pub use contract::*;
#[cfg(target_os = "android")]
pub use driver::reset_retry_tracking;
#[cfg(any(
    target_os = "windows",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
))]
pub use driver::restart_location;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use driver::start_apple_location_once;
pub use driver::{
    POLL_INTERVAL, platform_location, shared_location, shared_location_precise,
    stop_shared_location,
};
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::LocationError;
