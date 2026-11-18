//! local-storage-kv（アプリ専用領域へのキー・値の保存）の機能 crate（REQ-21 / TASK-21.4、Issue #1332）。
//!
//! 役割は 2 つ。(1) OS ごとの保存先の解決 [`data_dir`]（Android は `getFilesDir()` 相当、iOS はアプリコンテナの
//! `Documents/`、デスクトップは `$HOME/.fandhe`）と、それを使う入口 [`platform_local_storage_kv`]。
//! (2) ノート保存の型サンプル画面（`fandhe_core::screens::storage_note`）への橋渡し [`storage_note_state`]。
//! 呼び出し元は `fandhe-gallery` の画面登録と、保存先を共有する他の機能 crate（background-task）。
//! Web は `fandhe-web-dom` の `storage_note.rs` が `NoteStorage` を直接実装して持つ（`localStorage`）。
//!
//! 守る範囲: キーの検証（`validate_key`）と、読み書きの失敗を成功へ畳まないこと。
//! 守らない範囲: 複数プロセスからの同時書き込みの調停、OS によるアプリデータの消去、暗号化（秘密値は
//! `fandhe-cap-secure-credential` を使う）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・ノート保存の橋渡し（`driver`）・保存先の解決（`android`・`ios`）を
//! 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。

#![forbid(unsafe_code)]

#[cfg(target_os = "android")]
mod android;
mod contract;
/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;

pub use contract::*;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;
pub use error::LocalStorageKvError;

/// 現在の OS の保存先を返す。Android は内部ストレージ（`getFilesDir()` 相当）、iOS は `Documents/`、
/// それ以外は `$HOME/.fandhe`。解決できなければ `Err`（ダミーの保存先へは倒さない）。
pub fn data_dir() -> Result<DataDir, LocalStorageKvError> {
    #[cfg(target_os = "android")]
    return android::data_dir();
    #[cfg(target_os = "ios")]
    return ios::documents_dir();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return FsStorage::desktop_default()
        .map_err(|_| LocalStorageKvError::Unsupported("home directory not found"));
}

/// 画面の状態型に依存しない入口。[`data_dir`] を保存先にした `FsStorage` を返す。
pub fn platform_local_storage_kv() -> Result<FsStorage, LocalStorageKvError> {
    data_dir().map(FsStorage::new)
}
