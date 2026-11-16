//! url-scheme（カスタム URL scheme `fandhe://` の受信）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1333）。
//!
//! 扱う範囲は「OS から push で届いた URL 文字列を検査し、最新 1 件を保持して画面が読めるようにする」ところまで。
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS / iOS は `apple`、Android は
//! `android` と `android/java` の Java、Windows は `windows`）・宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を
//! 1 crate に集める。Web は `fandhe-web-dom` の `url_scheme`（`web+fandhe`）が `fandhe-device` の検査を直接使う。
//!
//! # 依存方向
//!
//! 本 crate → `fandhe-core`（画面の状態関数）・`fandhe-device`（検査と受信箱 `UrlInbox`）・`fandhe-runtime`（`CompletionSender`）。
//! egui には依存しない。呼び出し元は `fandhe-gallery`（effect の登録、macOS・iOS の `register()`、Windows の起動時
//! `windows::win::startup`）。app-shortcuts・spotlight-index・file-association（Windows）は、本 crate の受け口へ URL を渡すため
//! 本 crate に依存する（機能 crate 同士の依存、`consumer-app-structure.md` §8）。本 crate はそれらへ依存しない
//! （Windows のファイル起動は [`windows::FileRoute`] の口で受け取る）。
//!
//! # Android の受け口
//!
//! `UrlSchemeLifecycle`（非公開 provider）が共通層の `deliverIntents` で起動時と `onNewIntent` を `UrlSchemeHelper.deliver` へ配る
//! （#1325）。`deliver` は `ACTION_VIEW` のうち https（deep-link の受信箱へ）と `fandhe://web-auth`（web-auth-session の
//! コールバック。code・state を本受信箱へ流さない）を除いて渡す。利用者の `MainActivity` には何も書かせない。
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の受信順、不正 URL を表示しない、コールドスタート分を落とさない、判定と更新を同じ lock の内側で行うこと、
//! 転送失敗を成功にしないこと。
//! 守らない: ポーリング間隔内に連続して届いた場合に最後の 1 件だけ表示されること、複数プロセス間（Windows の別プロセス起動は
//! 転送側の責務）、他アプリが同じ scheme を宣言した場合の振り先、プロセス死亡後の URL の再表示。

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

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
/// Windows の起動引数・単一インスタンス転送・レジストリ登録。file-association（Windows）が部品を使うため公開する。
pub mod windows;

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::register;
pub use contract::*;
pub use driver::*;
pub use error::UrlSchemeError;
