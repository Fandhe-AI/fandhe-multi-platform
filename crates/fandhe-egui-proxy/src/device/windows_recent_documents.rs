//! Windows 向け「最近使った項目」の骨格（Issue #988、実装は #989）。
//! `device/recent_documents.rs`（effect ドライバ）の windows 経路から UI スレッドで同期に呼ばれ、
//! 契約は `fandhe_device::recent_documents`。本ファイルは当面 `Unsupported` を返す薄い関数だけを持つ
//! （非対応を成功に見せない）。
//!
//! # #989 への申し送り（このファイルだけを埋める。dispatch・画面・文言・trait doc・`device.rs` は触らない）
//!
//! - 登録は `SHAddToRecentDocs(SHARD_PATHW, パス)`、読み戻しは
//!   `IApplicationDocumentLists::GetList(ADLT_RECENT)`。登録の直後に読み戻して `Listed` / `NotListed`
//! - `pv` に NULL を渡さない（全アプリの利用履歴を消す）。引数は `ShellPath` からしか作らない
//! - 比較は両側を正規化する（`canonicalize` の `\\?\` を外し、大文字小文字を無視）
//! - 要素の `GetDisplayName` が 1 件でも失敗したら `Internal`（`NotListed` に畳まない）
//! - `IObjectArray` / `IShellItem` / `CoTaskMemFree` を全経路で解放し、COM の STA 初期化は対にする
//! - `windows` クレートの feature は追加しない

use fandhe_device::recent_documents::{RecentDocuments, UnsupportedRecentDocuments};

/// Windows の `RecentDocuments` 実装を返す（`device/recent_documents.rs` の dispatch から呼ばれる）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn implementation() -> Box<dyn RecentDocuments> {
    Box::new(UnsupportedRecentDocuments)
}
