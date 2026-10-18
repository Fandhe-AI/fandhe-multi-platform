//! Android 向けバックグラウンドダウンロードのスタブ（Issue #390。親 #100 の b-android #392 が埋める）。
//! `device/background_download.rs::implementation` の Android 経路から呼ばれ、現状は `Unsupported`
//! （成功を装わない）。dispatch と `device.rs` の `pub mod` 列は触らずに済む。
//!
//! # b-android への申し送り
//!
//! - `DownloadManager`（GMS 非依存）の pull 型。ダウンロード id は `SharedPreferences` に保存し、
//!   再起動後は `query` で再接続する。取り消しは `remove`（途中のファイルも消える）
//! - 保存先はアプリ専用の外部領域（保存の権限は不要）。固定名は `background_download::FILE_NAME`
//! - ローカルの確認用サーバーにだけ平文を許す network security config を置く
//! - `DownloadManager.Request` は http と https のみ。完了判定は `completion_status(None, bytes)`
//! - 利用者の取り消しは `Cancelled`（失敗にしない）。JNI 署名は Java・no-op スタブ・実体で揃える

#[cfg(any(target_os = "android", test))]
use std::sync::Arc;

#[cfg(any(target_os = "android", test))]
use fandhe_device::background_download::{BackgroundDownload, UnsupportedBackgroundDownload};

/// Android の実装を返す。b-android が差し替えるまでは `Unsupported`。
#[cfg(any(target_os = "android", test))]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    Arc::new(UnsupportedBackgroundDownload)
}
