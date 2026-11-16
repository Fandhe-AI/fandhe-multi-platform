//! background-download（OS 管理のバックグラウンドダウンロード）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。
//! 転送の持ち主は OS で、アプリの終了・再生成をまたいで続く（iOS・macOS は背景 `NSURLSession`、Android は `DownloadManager`、
//! Windows は BITS）。完了した転送は固定名のファイルへ保存し、実長で成否を決める（0 バイト・HTTP エラーを成功にしない）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（`apple`・`ios`・`android` と `android/java`・
//! `windows`）を 1 つに集める。Web は `fandhe-web-dom` の `background_download`（Service Worker の Background Fetch）が本 crate の
//! 契約を直接使う。画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_background_download()` は状態型に依存しない。
//!
//! # 依存方向と起動前の登録
//!
//! iOS は `fandhe-platform-apple` の application delegate に背景 session の受け手を登録する（[`register`]）。`fandhe-gallery` の `main` が
//! `UIApplicationMain` より前に呼ぶ（relaunch の完了通知は起動と同じターンで届くため）。Android は `BackgroundDownloadHelper` の static
//! メソッドを JNI で呼び、利用者の `MainActivity` に受け口は要らない（provider も作らない。状態は SharedPreferences の現在の id だけ）。
//!
//! 守る範囲: 1 プロセス内の開始・取り消し・状態の順序、嘘の成功の禁止（0 バイト・HTTP エラー・保存失敗）、未登録の identifier の完了を
//! 成功に見せないこと。守らない範囲: 起動直後の再接続（非同期）を待たない開始、背景で失敗した理由と前回の完了表示（再起動で失われる）、複数プロセス。

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
#[cfg(any(target_os = "ios", target_os = "macos", test))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::BackgroundDownloadError;
#[cfg(target_os = "ios")]
pub use ios::register;

// `.m` が外部参照する `FandheScene*Notification`・delegate の登録関数は `fandhe-platform-apple` が定義する。
// 公開関数を実際に参照して、この機能だけを選んだ iOS の成果物でもリンクから落ちないようにする
// （`capability-crate-migration.md` §21）。
#[cfg(target_os = "ios")]
const _: fn() = fandhe_platform_apple::ios_scene_install;
