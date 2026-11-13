//! external-app-launch（電話発信・地図など他アプリの起動）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`external_app_launch_show_effect`）、Activity 再生成時の
//! `fandhe-gallery` の `android_main`（`reset_pending_tracking`）、契約の固定 URI（`PHONE_URI`・`GEO_URI`）を共有する
//! `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。Android（`startActivity`）・iOS（`UIApplication`）・
//! macOS（`NSWorkspace`）・Windows（`ShellExecuteExW`）が実装する。開く先は固定の 2 種で、外部から URL・Intent を受けない。
//! 守る範囲は 1 プロセス内の要求順序・非前景や失敗を成功に畳まないこと・Activity 再生成時の旧要求の破棄。
//! 守らない範囲は他アプリへ渡した後の利用者の操作（結果は `HandedOff` までしか言えない）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_external_app_launch()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は iOS・macOS・Windows の FFI と Android の JNI エクスポート（`nativeOn*`）に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ExternalAppLaunchError;
