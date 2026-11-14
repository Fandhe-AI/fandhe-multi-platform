//! tilt-motion（傾きセンサ: 端末の傾き（重力ベクトル）を読み、画面の玉を転がす）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。iOS は `CMMotionManager` の gravity を毎フレーム pull し、Android は
//! `SensorManager` の重力センサ値を Java から push で受ける。デスクトップはスライダー値を `DummyMotion` へ書いて代替し、
//! Web は `fandhe-web-dom`（`DeviceOrientationEvent`）。`fandhe-gallery` の画面登録から effect `tilt_ball_effect` が呼ばれる。
//! Android の画面ゲート（[`set_screen_active`]）は `fandhe-gallery` の `android_main` が起動画面の決定直後に呼ぶ。
//!
//! iOS の共有 `CMMotionManager`（`ios/fandhe_ios_motion.m`）は motion-raw-sensors の iOS 実装も使うため、
//! motion-raw-sensors が iOS の target 依存で本 crate に依存する。
//!
//! 守る範囲: FFI 境界を越えた値の正規化（NaN・範囲外）、main スレッド以外の iOS センサ呼び出しの禁止、傾きを使わない画面での
//! 再描画要求の抑制。守らない範囲: センサの精度・周期、複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_tilt_motion()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(target_os = "android")]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;

#[cfg(target_os = "android")]
pub use android::set_screen_active;
pub use contract::*;
pub use driver::*;
pub use error::TiltMotionError;
