//! flashlight（ライト・トーチの点灯 / 消灯）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! iOS は `AVCaptureDevice.torchMode`、Android は `CameraManager.setTorchMode`、Windows は `Lamp`、Web は
//! `fandhe-web-dom` が `MediaStreamTrack` で実装する（Web の本体は `fandhe-web-dom` に残り、本 crate の契約の定数を使う）。
//! `fandhe-gallery` の画面登録から effect `flashlight_toggle_effect` が呼ばれる。
//!
//! 守る範囲: 1 プロセス内の順序、失敗を成功と見せないこと、背景中と破棄後に点けないこと（OS から読み戻した値だけを「点灯中」と出す）。
//! 守らない範囲: 外部からの点け消しへの追従遅れ、他アプリのカメラ使用中の一時的な食い違い、熱による減光、プロセス終了時の後片付け。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_flashlight()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "ios", target_os = "windows"), deny(unsafe_code))]

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
pub use error::FlashlightError;
