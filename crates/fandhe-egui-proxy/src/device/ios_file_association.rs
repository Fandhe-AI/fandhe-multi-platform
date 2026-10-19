//! iOS 向けファイルの関連付け受信のスタブ（Issue #416。b-ios が埋める）。
//! `device/file_association.rs::platform` の iOS 経路から呼ばれ、今は `Unsupported` を返す
//! （成功を装うダミーにはしない）。
//!
//! # b-ios への申し送り
//!
//! - 受け口と FFI 関数は macOS と共用（`apple_file_association.rs` の `INBOX` と
//!   `fandhe_rust_file_association_received`）。`implementation()` が返す型を作り、
//!   `apple/fandhe_apple_file_association.m` の `TARGET_OS_IOS` 分岐に delegate の
//!   `application:openURL:options:` と起動時の `launchOptions` を書く
//! - `isFileURL` でない URL（url-scheme 用）は触らず、ファイル URL だけをここへ回す
//! - Info.plist に `CFBundleDocumentTypes`（`public.plain-text`、`LSHandlerRank=Alternate`）と
//!   `LSSupportsOpeningDocumentsInPlace=false` を足す。届いた Inbox のコピーは読んだ後に消す
//! - 結果が入った直後に `crate::ios::request_repaint` を呼ぶ（契約表 4）
//! - 本ファイルの `implementation()` のシグネチャは変えない（dispatch は触らない）

use fandhe_device::file_association::{FileAssociation, UnsupportedFileAssociation};

/// iOS の実装を返す。b-ios が実体に差し替える。
pub(crate) fn implementation() -> &'static dyn FileAssociation {
    &UnsupportedFileAssociation
}
