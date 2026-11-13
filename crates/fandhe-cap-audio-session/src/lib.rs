//! audio-session（音声セッションの種別と割り込みの通知）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1192）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`audio_session_show_effect`）、終了時の `fandhe-gallery`
//! （`shutdown`）、Activity 再生成時の `fandhe-gallery` の `android_main`（`reset_pending_tracking`）。
//! Android（`AudioFocusRequest`）・iOS（`AVAudioSession`）が実装し、他の OS は `Unsupported`。
//! 変更前の共有状態（`AVAudioSession` の category 等）は全終了経路で復元し、復元に失敗したら次の開始を受け付けない。
//! 守る範囲は 1 プロセス内の受信順・失敗と非対応を成功に畳まないこと・全終了経路での解除。
//! 守らない範囲は画面を開く前・止めた後の出来事・複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_audio_session()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は iOS の FFI と Android の JNI エクスポート（`nativeOn*`）に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::AudioSessionError;
