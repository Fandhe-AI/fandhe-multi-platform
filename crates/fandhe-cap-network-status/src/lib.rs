//! network-status（接続の有無・種別・従量制を読む）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! iOS・macOS は `NWPathMonitor`、Android は `ConnectivityManager`、Windows は `NetworkInformation`。Web は `fandhe-web-dom` 側が実装し、契約の定数を共有する。
//! 呼び出し元は `fandhe-gallery` の画面登録（`registry.rs` の `effect` 区画）が登録する effect ドライバ（`driver`）で、[`READ_INTERVAL`] ごとに pull で読む。
//!
//! 守る範囲: 取得できないときに「接続あり」と偽らないこと（`Unknown`）、値をログへ出さないこと、資源（`NWPathMonitor`）をプロセス寿命で 1 つに限ること。
//! 守らない範囲: インターネット到達性、変化の反映遅れ（最大 [`READ_INTERVAL`] 程度）、複数プロセスの表示差。詳細は `contract.rs` の契約を参照。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_network_status()` は状態型に依存しないため、
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

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos", test))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::NetworkStatusError;
