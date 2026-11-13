//! system-tray（メニューバー / 通知領域アイコン）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`system_tray_effect`）と、アプリ終了時に共有アイコンを
//! 片付ける `fandhe-gallery` の終了フック（`stop_shared_system_tray`）。macOS と Windows だけ実装し、
//! 他 OS は `UnsupportedSystemTray`（嘘の成功にしない）。守る範囲は 1 プロセス内の表示 / 非表示の順序と
//! アイコンの解放で、複数プロセス間・OS 側の非同期確定との時間差は守らない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_system_tray()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "ios", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(target_os = "macos")]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::SystemTrayError;
