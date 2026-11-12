//! user-idle の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。OS 全体の無操作時間を 1 秒間隔で読む。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`user_idle_show_effect`）。macOS（CoreGraphics）と Windows（`GetLastInputInfo`）が実装し、
//! Web 実装は `fandhe-web-dom` に残る（`contract` の純関数・定数を共用）。iOS・Android は OS 全体の値を返す API が無く `Unsupported`。
//! 守る範囲は不正値（NaN・負・折り返し）を成功に畳まないこと。守らない範囲は別セッション・RDP の入力。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_user_idle()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "macos", target_os = "windows"), deny(unsafe_code))]

mod contract;
mod driver;
mod error;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::UserIdleError;
