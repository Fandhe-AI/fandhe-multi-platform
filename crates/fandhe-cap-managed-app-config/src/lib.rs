//! managed-app-config（MDM の管理アプリ設定の読み取り）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`managed_app_config_show_effect`）。Android（`RestrictionsManager`）・
//! iOS・macOS（`NSUserDefaults` の管理設定）が固定 3 キーを 1 回読む。他の OS は `Unsupported`。
//! 守る範囲は失敗・型違い・未設定の区別と、管理値・例外の文言をログに出さないこと。
//! 守らない範囲は読み取り後の変化の反映（再押下まで）と OS 呼び出しが戻らない場合の期限。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_managed_app_config()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は iOS・macOS の FFI に限る（Android の JNI は共通層の `with_activity_class` が持つ。`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod macos;

pub use contract::*;
pub use driver::*;
pub use error::ManagedAppConfigError;
