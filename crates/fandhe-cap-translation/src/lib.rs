//! 端末内翻訳の機能 crate（REQ-21 / TASK-21.4、Issue #1330、手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 固定の原文を元言語・訳先言語を明示して端末内で 1 回訳す。契約（`contract`）・失敗の種類（`error`）・
//! effect ドライバ（`driver`）・OS 実装（macOS / iOS は Swift 薄層経由の `apple`・`ios`、Android は
//! `android.view.translation` を呼ぶ `android`）を 1 つに集める。Windows・Linux は `Unsupported` 固定。
//! Web の実体は `fandhe-web-dom` が持ち、本 crate の契約（`check_result`・`validate_request` 等）だけを共有する。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`translation_effect`）と、
//! Activity 再生成時の `reset_pending_tracking`。入口 [`platform_translation`] は画面の状態型に依存しないため、
//! フレームワークの外のアプリも使える。画面の状態型を参照してよいのは `driver` だけ。
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序・嘘の成功の禁止・OS 資源の解放。守らない範囲: OS ごとの所要時間、
//! 期限後も OS 側の翻訳が走り切ること、複数プロセス（`contract` の doc 参照）。
//!
//! Android の `AndroidManifest.xml` への provider（`TranslationLifecycle`）は宣言（`[package.metadata.fandhe]`）で足す。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::TranslationError;
