//! motion-raw-sensors（ジャイロ・磁気・線形加速度の生値: 種類ごとに購読を始め、最新の 3 軸の値を画面に出す）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。iOS は `CMMotionManager`（共有の `CMMotionManager` は `fandhe-cap-tilt-motion` が持つため
//! iOS で依存する）、Android は `SensorManager`、Windows は WinRT の `Gyrometer`・`Magnetometer`・`Accelerometer`。macOS は対象外、
//! Web は `fandhe-web-dom`。`fandhe-gallery` の画面登録から effect `motion_raw_sensors_show_effect` が呼ばれる。
//! Windows の最小化の判定は共通層（`fandhe_platform_windows::minimize`）の受け口へ登録して受け取り、
//! environment-sensors とは互いに依存しない。
//!
//! 守る範囲: 種類ごとの購読の開始・停止、値なしと失敗の区別（0 や前回値で埋めない）、背景遷移での保持の解除、古い値を出さないこと。
//! 守らない範囲: センサの精度・周期、OS 間の値の一致、複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_motion_raw_sensors()` は状態型に依存しないため、
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
pub use error::MotionRawSensorsError;
