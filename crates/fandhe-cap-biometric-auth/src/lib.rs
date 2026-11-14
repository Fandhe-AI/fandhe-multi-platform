//! 生体認証の機能 crate（REQ-21 / TASK-21.4、Issue #1330、手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 生体認証（指紋・顔）の成否を 1 回確かめる。契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・
//! OS 実装（iOS・macOS は `LAContext`、Android は `BiometricPrompt`、Windows は Windows Hello）を 1 つに集める。
//! 認証 UI はいずれもコールバック型のため、`BiometricAuth` trait を通さず pull 型の bridge（`RequestSlot`）で結果を受ける
//! （trait 経由は Linux などの `Unsupported` 固定だけ）。Web は未対応。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`biometric_check_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（Android）。入口 [`platform_biometric_auth`] は画面の状態型に依存しない。画面の状態型を
//! 参照してよいのは `driver` だけ。
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序・嘘の成功の禁止（キャンセル・使えない・不一致を区別し、古い世代の結果は
//! 捨てる）・生体データや OS の文言を持ち回らないこと。守らない範囲: Windows の「認証した」が PIN を含みうること
//! （`driver::outcome_from_verification` の doc）、再生成をまたいだ保留中の要求の復元（破棄する契約）。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
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
pub use error::BiometricAuthError;
