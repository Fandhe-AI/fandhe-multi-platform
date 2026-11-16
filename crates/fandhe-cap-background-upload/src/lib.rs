//! background-upload（OS 管理のバックグラウンド送信）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。
//! 固定内容の 1 MiB のファイルを OS に送らせ、アプリの終了・再生成をまたいで続ける（iOS・macOS は背景 `NSURLSession`、
//! Android は WorkManager の Worker）。状態の SSOT は OS で、画面は合図（`set_signal`）を受けて読み直す。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（`apple`・`ios`・`android` と `android/java`）を
//! 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_background_upload()` は状態型に依存しない。
//!
//! # 依存方向と起動前の登録
//!
//! iOS は `fandhe-platform-apple` の application delegate に背景 session の受け手を登録する（[`register`]）。`fandhe-gallery` の `main` が
//! `UIApplicationMain` より前に呼ぶ。Android は `BackgroundUploadLifecycle`（非公開 provider）が Activity ごとに Helper を持ち、
//! 作り直し後に合図を登録し直す。利用者の `MainActivity` に受け口は要らない。WorkManager の依存は宣言の `gradle` にある。
//!
//! 守る範囲: 1 プロセス内の開始・取り消し・状態の順序、嘘の成功の禁止、Helper が無い要求を失敗で確定すること、未登録の identifier の
//! 完了を成功に見せないこと。守らない範囲: 背景で失敗した理由と前回の完了表示（再起動で失われる）、複数プロセス、OS の送信の遅延。

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
#[cfg(any(target_os = "ios", target_os = "macos", test))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::BackgroundUploadError;
#[cfg(target_os = "ios")]
pub use ios::register;

// `.m` が外部参照する `FandheScene*Notification`・delegate の登録関数は `fandhe-platform-apple` が定義する。
// 公開関数を実際に参照して、この機能だけを選んだ iOS の成果物でもリンクから落ちないようにする
// （`capability-crate-migration.md` §21）。
#[cfg(target_os = "ios")]
const _: fn() = fandhe_platform_apple::ios_scene_install;
