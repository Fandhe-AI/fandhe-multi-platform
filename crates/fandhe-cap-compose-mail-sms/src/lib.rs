//! compose-mail-sms の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。OS のメール・SMS の作成画面を開く
//! （対応表の id は compose-mail-sms と compose-mail-attachment の 2 つで、同じモジュールを共有するため 1 つの crate にまとめた）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`compose_mail_sms_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（gallery の `android_main` から、Android のみ）。Web の実装本体は `fandhe-web-dom` に残り、
//! 契約の `ComposeRequest`・`ComposeAttachment` だけを本 crate から使う。宛先・件名・本文・添付・例外の文言はログに出さない。
//! 守る範囲は 1 プロセス内の要求の順序・NoHandler や非前景を成功に畳まないこと・添付の一時ファイルの追跡と回収
//! （このプロセスが作った分だけ）、守らない範囲は作成画面へ渡した後の送信可否・複数プロセス・OS の一時領域に残る他者のファイル
//! （`contract` の doc を参照）。macOS・Windows・iOS・Android が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_compose_mail_sms()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

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

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
mod compose_attachment_file;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ComposeMailSmsError;
