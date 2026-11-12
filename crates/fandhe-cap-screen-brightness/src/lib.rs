//! screen-brightness の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。画面輝度の上書きと読み取り。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`screen_brightness_effect`）。iOS（`UIScreen.brightness`）と Android
//! （`WindowManager.LayoutParams.screenBrightness`、Activity ごとの `ScreenBrightnessHelper` を `ScreenBrightnessLifecycle` が束ねる）が実装し、
//! 他の OS は `Unsupported`。守る範囲は 1 プロセス内の順序・失敗を成功と見せないこと・期限後に適用しないこと。
//! 守らない範囲は背景にいる間の表示、自動輝度や利用者の操作による一時的な食い違い、複数ウィンドウ。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_screen_brightness()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::ScreenBrightnessError;
