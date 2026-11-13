//! video-transcode の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。実行時に作った動画を小さい解像度の mp4 へ 1 回変換する。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`video_transcode_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（gallery の `android_main` から、Android のみ）。Web の実装本体は `fandhe-web-dom` に残り、
//! 契約の定数と出力の検査だけを本 crate から使う。映像・出力・OS の生エラー値はログに出さない。
//! 守る範囲は失敗を成功値へ畳まないこと・一時ファイルの解放（未解放は static で追跡し、確認できるまで次の要求を受け付けない）・
//! FFI へ渡す長さの上限、守らない範囲は端末ごとの所要時間・ビットレート・符号形式の違い・複数プロセス間（`contract` の doc を参照）。
//! macOS・iOS・Windows・Android が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_video_transcode()` は状態型に依存しないため、
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
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::VideoTranscodeError;
