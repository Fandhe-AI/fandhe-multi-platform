//! recent-documents の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。OS の「最近使った書類」へサンプルを登録し、載ったかを読み戻す。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`recent_documents_show_effect`）。OS へ渡すパスは
//! `fandhe-cap-open-in-shell` の検査済み `ShellPath` だけで、パス・一覧の他の項目はエラー・ログに出さない。
//! 守る範囲は検査済みの型以外を OS へ渡さないこと・登録できなかったことを成功と見せないこと・COM 等の確保物の解放、
//! 守らない範囲は OS 側の一覧の反映の遅れ・他アプリが一覧を書き換えること（`contract` の doc を参照）。macOS・Windows が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_recent_documents()` は状態型に依存しないため、
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
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::RecentDocumentsError;
