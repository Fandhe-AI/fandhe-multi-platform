//! multi-display（接続中の画面一覧） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`multi_display_show_effect`）と、契約（検証・`READ_INTERVAL`・`ScreenInfo`）を共有する
//! `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。間隔つき pull で OS を読み、接続・切断の通知は購読しない。
//! 守る範囲は不正な画面情報を成功にしないことと 1 プロセス内の取得で、反映の遅れ（間隔 + 1 フレーム）・
//! OS 間の画面数の一致・複数プロセスは守らない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_multi_display()` は状態型に依存しないため、
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
#[cfg(target_os = "ios")]
mod ios;
#[cfg(any(target_os = "macos", target_os = "ios", test))]
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::MultiDisplayError;
