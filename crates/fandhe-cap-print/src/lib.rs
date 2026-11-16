//! print の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1334）。OS の印刷ダイアログへ文章と画面の画像を渡す（対応表の print と print-screen を同居させる）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`print_page_effect`・`print_screen_page_effect`）と、Web の `fandhe-web-dom`（契約の検査だけを共有）。
//! 画面の読み戻しは描画層の `fandhe_runtime::HostView`（`request_screenshot` / `tagged_screenshot`）で行い、寸法・長さの検査は `PrintImage::new`。
//! 守る範囲は検査済みの値だけを OS へ渡すこと・取り込みの期限と世代照合・嘘の成功の禁止・OS が読み終えるまで画素を保持すること、
//! 守らない範囲は紙に出たか・ジョブ投入後のプリンタ側の失敗・背景にいる間の取り込み（`contract` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_print()` は状態型に依存しないため、
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
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::PrintError;
