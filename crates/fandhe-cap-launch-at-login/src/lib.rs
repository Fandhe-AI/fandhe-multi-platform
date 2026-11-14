//! launch-at-login（ログイン時の自動起動）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! macOS は `SMAppService.mainApp`、Windows は HKCU の Run キーで実装する。iOS・Android・Web は対象外で `Unsupported`。
//! `fandhe-gallery` の画面登録から effect `launch_at_login_show_effect` が呼ばれ、画面のボタン操作ごとに契約を別スレッドで呼ぶ。
//!
//! 守る範囲: 1 プロセス内の操作順、失敗・承認待ち・利用者による無効を成功と見せないこと、別の場所の登録を消さないこと。
//! 守らない範囲: OS 設定で変えた状態が画面に出るまでの時間差、複数インストール・複数プロセスの同時操作
//! （対応表 `launch-at-login.json`）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_launch_at_login()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "macos", target_os = "windows"), deny(unsafe_code))]

mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::LaunchAtLoginError;
