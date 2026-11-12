//! app-attest の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。端末・アプリの真正性証明（Android の Key Attestation）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`app_attest_effect`）。Android のみ実装（`AppAttestHelper` の static を JNI で呼ぶ）で、
//! 他の OS は `Unsupported`。Activity の作り直しでは `fandhe-gallery` の `android_main` が `reset_pending_tracking` を呼ぶ。
//! 守る範囲は 1 プロセス内の要求と完了の順序・失敗を成功に畳まないこと・鍵エントリの解放。
//! 守らない範囲はチェーンの信頼性（サーバー側の判断）・StrongBox の不使用・複数プロセス間（`android` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_app_attest()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は持たない（JNI の unsafe は `fandhe-platform-android` に集約されている）。
#![forbid(unsafe_code)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::AppAttestError;
