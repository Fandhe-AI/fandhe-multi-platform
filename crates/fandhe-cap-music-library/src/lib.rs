//! music-library（端末内の曲のタイトルとアーティストを先頭 20 件だけ読む）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! macOS は iTunesLibrary（`ITLibrary`）、iOS は MediaPlayer（`MPMediaQuery`）、Android は MediaStore、Windows はファイルのタグ走査。Web は対象外。再生・アートワーク・総件数は扱わない。
//! 呼び出し元は `fandhe-gallery` の画面登録（`registry.rs` の `effect` 区画）が登録する effect ドライバ（`driver`）で、ワーカースレッドから OS 実装を呼ぶ。
//!
//! 守る範囲: 0 曲を出すのは許可済みで OS が空を返したときだけ（拒否・期限切れ・失敗を 0 曲へ畳まない）、期限（[`LIST_TIMEOUT`]）以内に必ず戻ること、曲名・アーティストをログへ出さないこと。
//! 守らない範囲: OS ごとの並び順の一致、Windows の走査上限を超えた先、クラウド由来の曲の有無、複数プロセス。詳細は `contract.rs` の契約を参照。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_music_library()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

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
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::platform_music_library;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::MusicLibraryError;
