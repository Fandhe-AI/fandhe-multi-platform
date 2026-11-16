//! notification（ローカル通知: タイトルと本文を OS へ渡して表示を依頼する。プッシュ通知は扱わない）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。macOS は `osascript`、Windows は WinRT Toast、iOS は `UserNotifications`、
//! Android は `NotificationManagerCompat`（権限ダイアログの結果をコールバックで受ける iOS・Android は `bridge`）。Web は `fandhe-web-dom`。
//! `fandhe-gallery` の画面登録から effect `notification_send_effect` が呼ばれる。Windows のスタートメニューのショートカットによる
//! AUMID の登録（[`ensure_app_aumid`]・[`APP_AUMID`]・[`setting_result_for`]）は alarm も使うため、Windows 限定で公開する。
//!
//! 守る範囲: 嘘の成功を出さないこと（拒否・無効を成功にしない）、子プロセス・ショートカットの確実な回収、タイトル・本文をログ・エラーに出さないこと。
//! 守らない範囲: 通知が実際に表示されたかの確認（`Submitted` までで `Shown` は返さない）、通知タップへの応答、プッシュ通知。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_notification()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
// 実体は std だけで OS 非依存。ホストの `cargo test` でも純関数と子プロセスの挙動を検証できるよう、テストでも載せる。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::NotificationError;
#[cfg(target_os = "windows")]
pub use windows::{APP_AUMID, ensure_app_aumid, setting_result_for};
