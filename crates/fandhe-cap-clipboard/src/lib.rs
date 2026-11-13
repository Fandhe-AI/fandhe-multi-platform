//! clipboard の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。OS のクリップボードのテキストを読み書きする。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`clipboard_text_effect`）。クリップボードには他アプリのパスワード等が
//! 載り得るため、内容・エラーの OS 文言をログに出さず、契約の「非テキストの内容は `Ok(None)`」と、接続できないときに
//! ダミー実装へ倒さず失敗を返すこと（嘘の成功の禁止）を守る。守らない範囲は他アプリが同時に内容を書き換えること・
//! iOS の許可アラートの待ち時間（`contract` の doc を参照）。Web の実装本体は `fandhe-web-dom` に残る。
//! macOS・Windows・Linux（`arboard`）・iOS（`UIPasteboard`）・Android（`ClipboardManager`）が実装する。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_clipboard()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::ClipboardError;
