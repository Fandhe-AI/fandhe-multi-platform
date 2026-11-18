//! file-association（ファイルの関連付け: OS から「このアプリで開く」と渡された `.txt` の受信）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。扱う範囲は「OS から push で届いたファイルを検査して読み、最新 1 件を保持して
//! 画面が読めるようにする」ところまで（登録・解除は Windows の画面操作のみ）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`・`ios` と `apple/*.m`、
//! Windows は `windows`、Android は未対応のスタブ）・宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める。
//! Web は `fandhe-web-dom` の `file_association`（launchQueue）が本 crate の `FileInbox`・`accepts` を使う。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_file_association()` は状態型に依存しない。
//!
//! # 依存方向
//!
//! Windows だけ `fandhe-cap-url-scheme` に依存する（単一インスタンスの named pipe・レジストリの部品を使うため。機能 crate 同士の依存、
//! `consumer-app-structure.md` §8）。url-scheme は本 crate へ依存しない。ファイル起動の解釈は [`windows::register_route`] で
//! url-scheme へ渡す（`fandhe-gallery` の `main` が起動直後に呼ぶ）。iOS・macOS の `register()` も `main` が起動前に呼ぶ。
//!
//! 守る範囲・守らない範囲は `contract` と各 OS 実装の doc を参照（1 プロセス内の受信順・不正入力の非表示・コールドスタート分の保持を守り、
//! 複数プロセス・UserChoice・他アプリによる上書きは守らない）。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "ios", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[cfg_attr(target_os = "ios", allow(dead_code))]
mod apple;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
/// Windows の登録・ファイル起動の経路。gallery の `main` が `register_route` を呼ぶため公開する。
pub mod windows;

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::register;
pub use contract::*;
pub use driver::platform_file_association;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::FileAssociationError;

// `.m` が外部参照する `FandheScene*Notification`・delegate の登録関数は `fandhe-platform-apple` が定義する。
// 公開関数を実際に参照して、この機能だけを選んだ iOS の成果物でもリンクから落ちないようにする
// （`capability-crate-migration.md` §21）。
#[cfg(target_os = "ios")]
const _: fn() = fandhe_platform_apple::ios_scene_install;
