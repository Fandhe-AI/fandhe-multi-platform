//! in-app-review の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。OS のレビュー依頼を 1 回呼ぶ。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`in_app_review_effect`）。守る範囲は押下 1 回につき呼び出し 1 回と嘘の成功の禁止、
//! 守らない範囲はダイアログの表示有無・投稿の有無（`contract` の doc を参照）。iOS・macOS が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_in_app_review()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;

pub use contract::*;
pub use driver::*;
pub use error::InAppReviewError;
