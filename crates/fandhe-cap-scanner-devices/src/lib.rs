//! scanner-devices の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。接続したスキャナー・カメラ機器から画像を 1 枚取り込む。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`scanner_devices_import_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（gallery の `android_main` から、Android のみ）。機器名・画像・id・コードはログに出さない。
//! 守る範囲は 1 プロセス内の要求の順序・失敗を成功と見せないこと・資源の解放・Activity 破棄時の保留要求の確定、
//! 守らない範囲は探索窓の後に現れた機器・複数台のうち先頭以外・他アプリが同じ機器を開いていることによる失敗（`contract` の doc を参照）。
//! macOS・iOS・Windows・Android が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_scanner_devices()` は状態型に依存しないため、
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
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ScannerDevicesError;
