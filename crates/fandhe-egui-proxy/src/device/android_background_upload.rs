//! Android 向けバックグラウンド送信実装の骨格（Issue #835 の a が置き、Issue #837 の b-android が埋める）。
//! `device/background_upload.rs::implementation` の Android 経路から呼ばれる。現状は `Unsupported` を返す
//! 薄いスタブで、ダミーの成功にはしない。対応表の Android セルは `possible` のまま（`impl` に書かない）。
//!
//! # b-android への申し送り
//!
//! - WorkManager の `enqueueUniqueWork(REPLACE)` と `NetworkType.CONNECTED`。Worker は `HttpURLConnection` の
//!   PUT（固定長ストリーミング）
//! - `WorkInfo` の写像: ENQUEUED / BLOCKED は `Waiting`、RUNNING は `Sending`（`getProgress`）、
//!   SUCCEEDED / FAILED は `outputData` から `completion_status`、CANCELLED は `Cancelled`
//! - `getWorkInfosForUniqueWork` で読み直し、LiveData の変化を `set_signal` の合図にする（状態を運ばない）
//! - 送るファイルの中身は `fandhe_device::background_upload::file_contents` と一致させる
//!   （同じ式 `i % 251` を Java で作るか、JNI で byte[] を渡す）
//! - JNI の署名は 3 か所（Java・no-op スタブ・実体）で揃える
//! - Activity の再生成で保留中の要求をどうするかを決めて揃える（契約表 1）
//! - 前面サービスと権限は足さない

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::background_upload::{BackgroundUpload, UnsupportedBackgroundUpload};

/// Android の実装を返す。b-android が実装するまでは `Unsupported`。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn BackgroundUpload> {
    Arc::new(UnsupportedBackgroundUpload)
}
