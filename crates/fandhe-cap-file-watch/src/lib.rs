//! file-watch（選んだフォルダの作成・更新・削除の監視） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`file_watch_show_effect`）と、契約の純関数を共有する
//! `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。フォルダ選択は `fandhe-device` の `file_picker` に残る共有部品を使う。
//! 守る範囲は監視の開始失敗を成功にしないことと、停止・破棄時の OS 監視資源の解放で、OS 側の通知の取りこぼし・
//! 複数プロセス・フォルダ外の変更は守らない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_file_watch()` は状態型に依存しないため、
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
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::FileWatchError;
