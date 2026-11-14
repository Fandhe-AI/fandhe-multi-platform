//! 資格情報（パスワード・トークン等）の安全な保存の機能 crate（REQ-21 / TASK-21.4、Issue #1330、手順は
//! `docs/operations/capability-crate-migration.md`）。
//!
//! OS のセキュアストア（iOS・macOS は Keychain、Android は `EncryptedSharedPreferences`、Windows は
//! `PasswordVault`）へ保存・読み出し・削除する。契約（`contract`）・失敗の種類（`error`）・effect ドライバ
//! （`driver`）・OS 実装（`apple`・`android`・`windows`）を 1 つに集める。Linux・Web は `Unsupported` 固定。
//! 平文ファイルの `fandhe_device::storage` は資格情報の保存先に使えない。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`credential_vault_effect`）。入口
//! [`platform_secure_credential`] は画面の状態型に依存しないため、フレームワークの外のアプリも使える。
//! 画面の状態型を参照してよいのは `driver` だけ。
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序・嘘の成功の禁止（失敗・無い・読めないを区別し、成功へ畳まない）・
//! 秘密値をログ・`Debug`・エラー文言へ出さないこと。守らない範囲: 複数プロセスからの同時書き込み、OS 側の
//! バックアップ・端末移行の挙動の全て。
//!
//! 利用者アプリの責務（Android）: 保存ファイル `fandhe_secure_credential.xml` は端末の Keystore 鍵に紐づくため、
//! バックアップ・端末間移行で復元されると初期化に失敗し続ける。アプリの `res/xml/data_extraction_rules.xml` で
//! クラウドバックアップ・端末間移行の両方から除外し、`<application android:dataExtractionRules>` で指す。
//! `<application>` の属性は機能の宣言では表せないため、本 crate は Manifest へ足さない。

// unsafe は OS 実装の FFI に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::SecureCredentialError;
