//! digital-credentials（デジタル身分証の提示要求）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`digital_credentials_effect`）と、契約の純関数と型
//! （`CredentialOutcome`・`FEATURE`・応答の検査）を共有する `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。
//! ネイティブ OS は全て `Unsupported`（Android は対応表で `possible` のまま未実装）で、成功へ倒さない。
//! 守る範囲は応答の中身（`data`）・例外の `message`・nonce をログや画面に出さないこと。守らない範囲は提示先の検証。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_digital_credentials()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

#![forbid(unsafe_code)]

mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::DigitalCredentialsError;
