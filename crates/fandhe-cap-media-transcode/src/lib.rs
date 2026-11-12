//! media-transcode（実行時に作った音声 1 本の AAC（m4a）への変換と出力の検証） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`media_transcode_effect`）と、Activity 再生成時に `reset_pending_tracking` を呼ぶ
//! アプリの `android_main`。契約（出力の m4a 検査・失敗の細分類）は Web 実装（`fandhe-web-dom`）と共有する。MP4 box の走査は
//! 動画変換 `fandhe-device::video_transcode` も使うため `fandhe-device::media_transcode` に残している。
//! 守る範囲は 0 バイト・形式違い・長さ不一致・期限切れを成功にしないことと、一時ファイルとコーデックの解放で、OS ごとの
//! 所要時間とビットレートの違い・複数プロセスは守らない。出力のバイト列と OS のエラー文はログへ出さない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_media_transcode()` は状態型に依存しないため、
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

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::MediaTranscodeError;
