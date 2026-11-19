//! web-auth-session（OAuth 用 Web 認証セッション）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。
//! PKCE 付きの認可ページを OS のブラウザ系 API（iOS・macOS は `ASWebAuthenticationSession`、Android は Custom Tabs、Windows は既定ブラウザ +
//! loopback）で開き、コールバック URL を検査して結果（成功・キャンセル・失敗）を返す。トークン交換は範囲外。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバと要求の追跡（`driver` と `driver::bridge`）・OS 実装
//! （iOS・macOS は `ios`・`macos` と `*.m`、Android は `android` と `android/java`、Windows は `windows`）を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。公開入口は状態型に依存せず、外のアプリは `begin_web_auth` で開始し、
//! `poll_web_auth(id)` で結果を 1 度だけ受け取る（`cancel_web_auth`・Windows のループバックは `complete_web_auth_loopback`）。
//! `platform_web_auth_session()` は乱数・SHA-256・`start`/`cancel` の OS 口で、結果の受け口は上の `poll_web_auth`。
//!
//! # 依存方向と Android の受け口
//!
//! `fandhe-device`（検査の上限 `MAX_URL_BYTES`・`SCHEME`・`base64url`）にだけ依存し、url-scheme の crate には依存しない
//! （選んでいないアプリへ url-scheme の受信箱を持ち込まない）。Android は `WebAuthSessionLifecycle`（非公開 provider）が
//! `onNewIntent` のリスナーと Paused / Resumed を受け、`fandhe://web-auth` のコールバックを確定する。同じ filter を宣言する
//! url-scheme 側は、コールバックを受信箱へ流さない（`UrlSchemeHelper.deliver` が同じ条件で除外。code・state を別画面に出さない）。
//! `shutdown` は入口 crate `fandhe` の終了処理（`src/entry/lifecycle.rs`）が呼ぶ。Activity 再生成では要求を引き継ぎ（Java が状態を static に持ち、結果は新しい Activity の `onNewIntent` へ届く）、呼び出し側の掃除は要らない。
//!
//! 守る範囲: 1 プロセス内で 1 件ずつ処理すること、嘘の成功を出さないこと、期限・取り消し・終了でサーバーと OS のセッションを残さないこと、
//! 秘密値（URL・state・code）をログと `Debug` に出さないこと。
//! 守らない範囲:
//! - 画面（effect）の要求は Activity の破棄で画面状態ごと失われるため、Java の `onHostDestroyed` が画面の要求だけを回収する
//!   （OS のセッション・待ち受け・slot。表示中の画面に依存しない。公開入口の要求は回収せず引き継ぐ。結果は新しい画面状態へ届けない）
//! - 結果が一度も返らない場合（Activity が再生成されずに終了した・プロセス死）、公開入口の要求は期限（300 秒）まで残り、
//!   期限は `poll_web_auth` を呼んだときに確定する
//! - Android でブラウザのタブが裏に残ること、既定ブラウザの UI の差、複数プロセス、プロセス死後のコールバック
//! - 他アプリによる `fandhe` scheme の横取り（PKCE で緩和）、トークン交換

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
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::{
    begin_web_auth, cancel_web_auth, complete_web_auth_loopback, platform_web_auth_session,
    poll_web_auth, shutdown,
};
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::WebAuthSessionError;
