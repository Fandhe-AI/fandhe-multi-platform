//! Windows 向けバックグラウンドダウンロードのスタブ（Issue #390。親 #100 の windows #394 が埋める）。
//! `device/background_download.rs::implementation` の Windows 経路から呼ばれ、現状は `Unsupported`
//! （成功を装わない）。dispatch と `device.rs` の `pub mod` 列は触らずに済む。
//!
//! # windows への申し送り
//!
//! - BITS（`IBackgroundCopyManager`）を使う。`BackgroundDownloader` は package identity が要るため使わない
//! - ジョブの GUID を保存して再起動後に再接続する。`Complete()` を呼ぶまでファイルは使えない
//! - 確認用サーバー（`serve-brotli.mjs`）は HEAD でサイズを返し、GET で Content-Range に対応する必要がある
//! - 完了判定は `completion_status(None, bytes)`。`Cancel` は `Cancelled`（失敗にしない）

#[cfg(any(target_os = "windows", test))]
use std::sync::Arc;

#[cfg(any(target_os = "windows", test))]
use fandhe_device::background_download::{BackgroundDownload, UnsupportedBackgroundDownload};

/// Windows の実装を返す。windows の子が差し替えるまでは `Unsupported`。
#[cfg(any(target_os = "windows", test))]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    Arc::new(UnsupportedBackgroundDownload)
}
