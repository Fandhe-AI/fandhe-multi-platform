//! screen-capture（画面の録画・キャプチャ） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`screen_capture_effect`）と、Activity 再生成時に `reset_pending_tracking` を呼ぶ
//! アプリの `android_main`。契約（`build_capture_result`）は Web 実装（`fandhe-web-dom`、本体は web-dom に残る）と共有する。
//! Android は MediaProjection の同意ダイアログと前景サービス（`ScreenCaptureService`）を持ち、Activity ごとの helper
//! （`ScreenCaptureLifecycle`）が前面離脱・破棄で録画を中断する。経過時間は `fandhe-runtime` の単調時計の実測で進める。
//! 守る範囲は 0 バイト・中断・失敗を成功にしないことと、キャプチャ資源・サービスの解放で、録画内容の検証・複数プロセスは
//! 守らない。録画ファイルのパスと内容はログへ出さない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_screen_capture()` は状態型に依存しないため、
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
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ScreenCaptureError;
