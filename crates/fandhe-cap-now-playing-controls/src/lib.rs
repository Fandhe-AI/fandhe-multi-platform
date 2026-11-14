//! now-playing-controls（ロック画面・OS のメディア操作: 曲名と再生状態を OS の「再生中」表示へ渡し、OS から来た再生・一時停止を
//! 画面へ返す）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。macOS・iOS は MediaPlayer、Android は
//! `MediaSession` と通知、Windows は `SystemMediaTransportControls`。Web は `fandhe-web-dom`。
//! `fandhe-gallery` の `audio_play` の effect が、`fandhe-cap-media-playback` の再生の後で [`sync_audio_play`] を呼ぶ
//! （2 機能は driver で互いに依存させない。Apple の `.m` だけが再生状態を読むため、iOS・macOS で `fandhe-cap-media-playback` に依存する）。
//!
//! 守る範囲: 1 プロセス内の操作順、取り下げ後の古い操作で再開しないこと、嘘の成功の禁止、session と通知の解放。
//! 守らない範囲: 通知を消した後の再表示、通知を切った端末でのシェード表示、背景中の操作、複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_now_playing_controls()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える（OS の操作は共有の受信口 [`commands`] から取り出す）。

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
#[cfg(any(target_os = "macos", target_os = "ios", test))]
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
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
pub use error::NowPlayingControlsError;
