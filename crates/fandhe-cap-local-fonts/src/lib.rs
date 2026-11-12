//! local-fonts（OS 搭載フォントのファミリー名一覧） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`local_fonts_show_effect`）と、契約（`families_from`・`require_named`）を共有する
//! `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。ボタン押下ごとに OS を 1 回読む。
//! 守る範囲は取れなかった結果を「0 件」の成功にしないことと 1 プロセス内の取得で、フォントの追加・削除の追従・
//! OS 間の一覧の一致・描画への利用は守らない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_local_fonts()` は状態型に依存しないため、
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
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::LocalFontsError;
