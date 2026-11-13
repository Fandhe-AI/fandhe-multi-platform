//! nfc の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。NFC タグの NDEF を 1 枚だけ読む。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`nfc_read_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（gallery の `android_main` から、Android のみ）。Web の実装本体は `fandhe-web-dom` に残り、
//! 契約の定数・写像表だけを本 crate から使う。タグの中身・UID・id・コードはログに出さない。
//! 守る範囲は 1 プロセス内の要求の順序・失敗を成功と見せないこと・reader mode の解除・Activity 破棄時の保留要求の確定、
//! 守らない範囲はかざしたが届く前の離脱・2 枚同時にかざしたときの選択・他アプリや OS が先にタグを処理した場合（`contract` の doc を参照）。
//! Android が実装し、他のネイティブ OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_nfc()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::NfcError;
