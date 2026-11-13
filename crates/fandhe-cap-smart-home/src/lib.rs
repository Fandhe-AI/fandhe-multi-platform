//! smart-home の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。iOS の HomeKit から家と機器の名前を読む（操作はしない）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`smart_home_list_effect`）。守る範囲は 1 要求 1 回の読み取りと期限内の確定、嘘の成功の禁止、
//! 守らない範囲は家・機器の追加と操作、許可ダイアログの表示有無（`contract` の doc を参照）。iOS だけが実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_smart_home()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]

mod contract;
mod driver;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
