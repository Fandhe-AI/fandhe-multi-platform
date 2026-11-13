//! removable-storage（外部ストレージの着脱の通知と一覧）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`removable_storage_show_effect`）。Android（`StorageVolumeCallback`・
//! `StorageManager`）・macOS（`NSWorkspace` の通知）・Windows（`WM_DEVICECHANGE`）が実装し、他の OS は `Unsupported`。
//! 一覧は覚えず、出来事が届くたびに OS から読み直す。購読は `register` で得た id で照合し、停止後・再生成前の古い id の通知は捨てる。
//! 守る範囲は 1 プロセス内の受信順・失敗と非対応を成功に畳まないこと・全終了経路でのコールバック解除。
//! 守らない範囲は画面を開く前・止めた後の出来事・通知と読み直しの間の時間差・複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_removable_storage()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートと、macOS の `.m` から呼ばれる受け口（`driver.rs`）に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::RemovableStorageError;
