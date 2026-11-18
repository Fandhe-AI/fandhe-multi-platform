//! screen-orientation の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1334）。画面の向きの固定・解除と読み戻し。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`screen_orientation_show_effect`）と、Web の `fandhe-web-dom`（`parse_reading` などの純関数だけを共有）。
//! iOS・Android が実装し（Android は `ScreenOrientationLifecycle` が Helper を Activity ごとに付ける）、他の OS は `Unsupported`。
//! 守る範囲は依頼と完了の分離・失敗を成功と見せないこと・期限後に適用しないこと、
//! 守らない範囲は分割画面・マルチウィンドウでの OS による無視・再生成での固定の引き継ぎ（`contract` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_screen_orientation()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は iOS の FFI に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]

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
pub use driver::platform_screen_orientation;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::ScreenOrientationError;
