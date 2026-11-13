//! file-preview の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。固定ファイルのサムネイルとプレビューを OS に頼む。
//!
//! 呼び出し元は画面側の effect（`driver`）とフレームワーク外のアプリ（`platform_file_preview()`）。
//! 守る範囲は、固定ファイル 2 つ・固定 3 操作に限ること、事前判定をせず OS の結果を正にすること、失敗・サムネイルなし・
//! 他アプリへの委譲を成功表示へ畳まないこと。守らない範囲は、任意パス・動画・PDF・複数ファイルの扱いと、
//! OS がプレビューを閉じた通知より先の状態の推測（詳細は `contract`）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_file_preview()` は状態型に依存しないため、
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

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::FilePreviewError;
