//! open-in-shell の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。ファイルを OS の既定アプリで開く・ファイル管理画面で選択して見せる。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`open_in_shell_effect`）と、recent-documents の機能 crate
//! （`ShellPath`・`prepare_sample_file_in`・`to_win32_path` を使う）。OS へ渡すパスは検査・正規化済みの `ShellPath` だけで、
//! 任意の文字列・URL・引数を受ける口は持たない。パス・OS の文言はエラー・ログに出さない。
//! 守る範囲は検査済みの型以外を OS へ渡さないこと・失敗を成功と見せないこと・サンプルの作成でシンボリックリンクを辿らないこと、
//! 守らない範囲は開いた後の利用者の操作・既定アプリの有無の事前確認（`contract` の doc を参照）。macOS・Windows が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_open_in_shell()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "macos", target_os = "windows"), deny(unsafe_code))]

mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
// `to_win32_path`（OS に触れない純関数）を recent-documents が全 OS のビルドで参照するため、Windows 以外でも載せる。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::OpenInShellError;
pub use windows::to_win32_path;
