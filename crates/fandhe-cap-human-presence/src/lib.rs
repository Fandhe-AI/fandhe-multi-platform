//! human-presence（人の在席検知）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`human_presence_read_effect`）。Windows（`HumanPresenceSensor`）だけが
//! 実装し、他の OS は `Unsupported`。失敗は契約の `HumanPresenceFailure` で表すため、`DeviceError` 相当の型は持たない。
//! 守る範囲は「読めない・拒否・未登録」を在席として返さないこと。守らない範囲は継続購読（1 回の読み取りだけ）。
//!
//! 契約（`contract`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_human_presence()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

#![forbid(unsafe_code)]

mod contract;
mod driver;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
