//! dynamic-type（OS の文字サイズ設定への追従）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1302）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`text_scale_effect`）と、契約の純関数（`clamp_scale`）を
//! 共有する `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。OS の倍率は pull で読み、拡大率の適用と
//! ドラッグ中の保留は `fandhe_runtime::HostView` 越しに行う（egui を知らない）。iOS（`UIFont`）・Android
//! （`Configuration.fontScale`）・Windows（`UISettings.TextScaleFactor`）が実装し、macOS は公開 API が無く `Unsupported`。
//! 守る範囲は「読めない」を `1.0` で代用しないこと・ドラッグ中に拡大率を変えないこと・描画コンテキスト単位の適用記録。
//! 守らない範囲は稼働中の設定変更への追従（再読込は画面の再表示）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_text_scale()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は iOS の FFI だけ。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]

#[cfg(target_os = "android")]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::DynamicTypeError;
