//! compass-heading（方位: 端末が向いている方位を度で読み、精度と 8 方位の表記を画面に出す）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。iOS は CoreLocation の `CLHeading`、Android は回転ベクトルセンサー、
//! Windows は WinRT の `Compass`、Web は `fandhe-web-dom`（`DeviceOrientationEvent`）。`fandhe-gallery` の画面登録から
//! effect `compass_show_effect` が呼ばれる。値が届かない時間の追跡（`WaitTracker`・`NO_READING_TIMEOUT_MS`）は ar・
//! environment-sensors も使うため `fandhe-device` に残し、`contract` から再公開する。
//!
//! 守る範囲: 購読の開始・停止を `wanted` の立ち上がり・立ち下がりでだけ行うこと、値なしと失敗の区別（0 度や前回値で埋めない）、
//! 背景遷移での購読の解除。守らない範囲: Windows の 2° 刻み、OS 間の値の一致、複数タブ・複数プロセス、表示の遅れ。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_compass_heading()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::CompassHeadingError;
