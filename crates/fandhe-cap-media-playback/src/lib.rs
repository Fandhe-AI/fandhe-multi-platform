//! media-playback の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 役割: 同梱 WAV 1 本の再生・一時停止・停止を端末機能として提供する。呼び出し元は画面の effect ドライバ（`driver`）と、フレームワーク外のアプリ（入口関数）。
//!
//! 守る範囲: 1 プロセス内の再生状態の順序、失敗・未対応を成功へ畳まないこと、OS の再生資源の解放。
//! 守らない範囲: 動画・バックグラウンド再生・プレイリスト・ネットワーク再生・シーク（契約は `contract` を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_media_playback()` は状態型に依存しないため、
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
#[cfg(any(target_os = "ios", target_os = "macos"))]
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
pub use error::MediaPlaybackError;
