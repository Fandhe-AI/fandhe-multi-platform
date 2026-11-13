//! serial-port の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。専用ワーカーでシリアルポートの一覧と 1 往復を行う。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`serial_port_show_effect`）と、終了時の `fandhe-gallery` の終了フック（`shutdown`）。
//! macOS（IOKit 列挙 + POSIX 端末入出力）と Windows（`GetCommPorts` 系）が実装し、Web 実装（Web Serial）は `fandhe-web-dom` に残る。
//! 守る範囲は固定設定（8N1）・3 秒の期限・開く前の設定の復元・嘘の成功の禁止。守らない範囲は複数プロセスからの同一ポート、待ち切れない OS 呼び出し（終了時は OS が fd を閉じる）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_serial_port()` は状態型に依存しないため、
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
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::SerialPortError;
