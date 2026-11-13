//! file-provider の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。アプリ専用の保存先を OS のファイル画面（Files アプリ・
//! 他アプリの保存先選択）に出す。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`file_provider_show_effect`）。Android は本 crate の
//! `FileProviderDocumentsProvider`（`DocumentsProvider`、`MANAGE_DOCUMENTS` を持つ system だけが bind できる）が同じフォルダを見せる。
//! 名前・書類 ID・パスはログ・エラー文に出さない。守る範囲は初期化失敗を 0 件の成功にしないこと・通知失敗を成功と並べて見せること、
//! 守らない範囲は他アプリが書き込み中の途中サイズ・通知から画面更新までの時間差（`contract` の doc を参照）。Android が実装し、他の OS は `Unsupported`。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_file_provider()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// JNI の unsafe は共通層（`fandhe_platform_android::with_activity_class`）へ移ったため、全 OS で unsafe を禁じる。
#![forbid(unsafe_code)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::FileProviderError;
