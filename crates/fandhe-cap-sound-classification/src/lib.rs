//! sound-classification の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。マイクで拾った環境音を SoundAnalysis の内蔵分類器で分類する（上位 3 件のラベルと信頼度）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`sound_classification_show_effect`）。守る範囲は嘘の成功の禁止・全終了経路での資源の解放・旧世代の通知を返さないこと、
//! 守らない範囲は背景での分類・中断後の自動再開・複数プロセス（`apple` の doc を参照）。iOS・macOS が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_sound_classification()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "ios", target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod apple;
mod contract;
mod driver;
#[cfg(target_os = "ios")]
mod ios;

pub use contract::*;
pub use driver::*;
