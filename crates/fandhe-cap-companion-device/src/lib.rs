//! companion-device（付属機器のペアリング）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。付属機器のペアリング（`CompanionDeviceManager`）で BLE 機器を 1 台選ばせ、名前を返す（関連付けは残さない）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`companion_device_show_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（`fandhe-gallery` の `android_main` から）。守る範囲は 1 プロセス内の要求順序・嘘の成功の禁止・旧要求の関連付けの解除、
//! 守らない範囲は API 32 以下・関連付けを残すこと・機器との通信とボンディング・選択画面の表示中のプロセス死（`contract` の doc を参照）。
//! Android 13（API 33）以上が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;

pub use contract::*;
pub use driver::*;
