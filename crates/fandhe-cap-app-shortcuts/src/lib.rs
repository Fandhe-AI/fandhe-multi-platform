//! app-shortcuts（アプリショートカット: Dock メニュー・ホーム画面長押し・ジャンプリスト・ランチャーの長押し）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。「デモを開く」1 項目を OS へ出す。選ばれたショートカットは
//! 固定の URL（`fandhe://shortcut/open-demo`）として url-scheme の受信箱へ入り、検査は url-scheme の `UrlInbox` が行う。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS は `macos` と `macos/*.m`、
//! iOS は Info.plist の静的宣言、Android は `android` と `android/java`、Windows は `windows`）を 1 つに集める。
//! 画面の状態型・定数を参照してよいのは `driver` だけ（契約と OS 実装は [`ShortcutSpec`] を引数で受ける）。
//! 入口 `pub fn platform_app_shortcuts()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。
//!
//! # 依存方向
//!
//! macOS だけ `fandhe-cap-url-scheme` に依存する（`.m` が受け口 `fandhe_rust_url_scheme_received` を呼ぶため。機能 crate 同士の
//! 依存、`consumer-app-structure.md` §8）。url-scheme の effect からは本 crate を呼ばない。`fandhe-gallery` の
//! `screens/registry.rs` が url-scheme の effect の後に [`drive_app_shortcuts`] を続けて呼ぶ（app-shortcuts だけを選ぶアプリへ
//! url-scheme を持ち込まず、url-scheme だけを選ぶアプリへ app-shortcuts を持ち込まない）。
//!
//! 守る範囲: 1 プロセス 1 回の登録、失敗・API 未満を成功にしないこと、URL・例外の文言をログに出さないこと。
//! 守らない範囲: 画面を一度開くまで項目が出ないこと、他アプリが同じ URL を送った場合との区別、OS・ランチャーごとの表示差、
//! 失敗後のプロセス内再試行、ポーリング間隔ぶんの反映遅れ。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
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
pub use error::AppShortcutsError;
