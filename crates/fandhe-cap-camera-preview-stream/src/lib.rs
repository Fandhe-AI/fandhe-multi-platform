//! camera-preview-stream（カメラのライブ映像・フレーム取得、ズーム操作を含む）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。iOS・macOS は `AVCaptureSession`（同じ `.m` を共有）、Android は
//! Camera2 + `ImageReader`、Windows は WinRT `MediaCapture` + `MediaFrameReader`、Web は `fandhe-web-dom` が `getUserMedia` で実装する
//! （Web の本体は `fandhe-web-dom` に残り、縮小の部品だけここから再公開する）。
//! `fandhe-gallery` の画面登録から effect `camera_preview_stream_show_effect` が呼ばれ、終了時に `shutdown`、
//! Activity 再生成時に `reset_pending_tracking` を最終成果物が呼ぶ。
//!
//! 守る範囲: 1 プロセス内の順序、失敗・中断・非対応を成功へ畳まないこと、全終了経路でのカメラの解放。
//! 守らない範囲: フレームの間隔・時刻、前面・背面の切り替え、露出・焦点・ライト、複数カメラ・複数画面、OS が止めた後の自動再開。
//! 画素はプライバシー情報なのでログ・`Debug` に出さない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_camera_preview_stream()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

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
mod contract;
mod driver;
#[cfg(any(target_os = "ios", target_os = "macos", test))]
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
