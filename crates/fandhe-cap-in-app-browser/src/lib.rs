//! in-app-browser（アプリ内ブラウザ）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! iOS は `SFSafariViewController`、Android は Custom Tabs（`CustomTabsIntent`）で http / https の URL を開く。
//! `fandhe-gallery` の画面登録から effect `in_app_browser_show_effect` が呼ばれ、Activity 再生成時は `android_main` が
//! `reset_pending_tracking` で旧要求を捨てる。
//!
//! 守る範囲: 1 プロセス内の要求順序、非前景・ブラウザ無しを成功に畳まないこと、期限切れと表示開始の排他。
//! 守らない範囲: 開いた後に Rust から閉じること、閲覧内容と読み込み失敗の検知、既定ブラウザの UI の差、複数プロセス。URL はログに出さない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_in_app_browser()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::InAppBrowserError;
