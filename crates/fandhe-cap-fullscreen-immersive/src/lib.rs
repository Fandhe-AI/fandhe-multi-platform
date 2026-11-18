//! fullscreen-immersive の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1334）。システムバーを隠す全画面表示の切り替えと読み戻し。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`fullscreen_toggle_effect`）と、Web の `fandhe-web-dom`（Fullscreen API へ直接配線）。
//! macOS・Windows は描画層の `fandhe_runtime::HostView`（`request_window(Fullscreen)` / `window_state`）、iOS・Android は OS 実装
//! （Android は `FullscreenLifecycle` が共通層 `HostInsets` へ配り先を登録して表示状態を読む）。他の OS は `Unsupported`。
//! 守る範囲は依頼と完了の分離・失敗や読めない状態を全画面と見せないこと・期限後に適用しないこと、
//! 守らない範囲はスワイプでバーが一時表示される間の表示ずれ・再生成での全画面の自動再適用（`contract` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_fullscreen_immersive()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::platform_fullscreen_immersive;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::FullscreenImmersiveError;
