//! safe-area-insets の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1334）。ノッチ・システムバーなどの余白（セーフエリア）を読む。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`safe_area_insets_show_effect`）と、Web の `fandhe-web-dom`（純関数と間隔だけを共有）。
//! 間隔つき pull で、macOS・iOS・Android が実装し、他の OS は `Unsupported`。Android は共通層 `HostInsets` が送る px を
//! 描画層の画素密度（`fandhe_runtime::HostView::native_pixels_per_point`）で dp に換算する（`SafeAreaInsetsLifecycle` が取り付けを起こす）。
//! 守る範囲は未受信を 0 にしないこと・値をログに出さないこと、守らない範囲は反映の遅れ（最大 1 秒 + 1 フレーム）・
//! 描画層の拡大率が 1 でないときの point とのずれ（`contract` / `android` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_safe_area_insets()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(target_os = "macos")]
mod macos;

pub use contract::*;
pub use driver::*;
pub use error::SafeAreaInsetsError;
