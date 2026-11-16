//! spotlight-index（OS の検索への登録: Core Spotlight・AppSearch）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。
//! 固定メモ 1 件の登録と自ドメインの全削除だけを行い、macOS・iOS は検索結果から開かれたとき `fandhe://spotlight/<id>` を
//! url-scheme の受信箱へ渡す。Android は AppSearch（API 31 以上）への登録だけで受信経路は無い。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`・`ios` と
//! `apple/*.m`、Android は `android` と `android/java`）を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。
//! 入口 `pub fn platform_spotlight_index()` は状態型に依存しないため、フレームワークの外のアプリも使える。
//!
//! # 依存方向
//!
//! `fandhe-cap-url-scheme` に依存する（driver が受信箱 `platform_url_scheme()` を読み、`.m` が受け口
//! `fandhe_rust_url_scheme_received` を呼ぶ。機能 crate 同士の依存、`consumer-app-structure.md` §8）。url-scheme は本 crate へ依存しない。
//! macOS・iOS の `register()` は `fandhe-gallery` の `main` が起動前に呼ぶ（通知の購読を `UIApplicationMain` より前に張るため）。
//!
//! 守る範囲: 失敗を成功へ畳まないこと、待ちの上限、1 プロセス内での要求どうしの順序、識別子・題名・本文をログへ出さないこと。
//! 守らない範囲: 検索への実際の表示（OS・OEM 次第）、反映の時間差、期限後に OS 側で済んでいた場合との食い違い、複数プロセス。

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
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::register;

pub use contract::*;
pub use driver::*;
pub use error::SpotlightIndexError;

// `.m` が呼ぶ `fandhe_rust_url_scheme_received` は url-scheme の `apple.rs` にある。公開関数を実際に参照して、
// リンク対象から外れないようにする（`capability-crate-migration.md` §21）。
#[cfg(any(target_os = "ios", target_os = "macos"))]
const _: fn() = fandhe_cap_url_scheme::register;

// `.m` が外部参照する `FandheScene*Notification`・delegate の登録関数は `fandhe-platform-apple` が定義する。
// 公開関数を実際に参照して、この機能だけを選んだ iOS の成果物でもリンクから落ちないようにする
// （`capability-crate-migration.md` §21）。
#[cfg(target_os = "ios")]
const _: fn() = fandhe_platform_apple::ios_scene_install;
