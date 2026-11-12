//! passkey-signin（パスキーの登録とサインイン） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`passkey_signin_effect`）と、Activity 再生成時に `reset_pending_tracking` を呼ぶ
//! アプリの `android_main`。契約の純関数（`outcome_from_dom_error`・`verify_response`）は Web 実装（`fandhe-web-dom`、本体は web-dom に残る）と
//! 共有する。base64url は `digital_credentials`・`web_auth_session` も使うため `fandhe-device::passkey` に残し、ここから再公開する。
//! Android は Credential Manager を使い、Activity ごとの helper（`PasskeyLifecycle`）で要求と OS の UI を持つ。
//! 守る範囲は不整合な応答・失敗・非対応を成功にしないことと 1 プロセス内の同時 1 要求で、署名と rpIdHash の検証・複数プロセスは
//! 守らない。challenge・資格情報 ID・署名・応答 JSON はログ・`Debug` に出さず、永続化しない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_passkey()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "windows"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::PasskeySigninError;
