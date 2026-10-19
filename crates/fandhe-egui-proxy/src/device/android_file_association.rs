//! Android 向けファイルの関連付け受信のスタブ（Issue #416。b-android が埋める）。
//! `device/file_association.rs::platform` の Android 経路から呼ばれ、今は `Unsupported` を返す
//! （成功を装うダミーにはしない）。
//!
//! # b-android への申し送り
//!
//! - JNI は `nativeOnFileOpened(int, String, String, int)`（世代、名前、MIME、fd）。`content:` の
//!   URI だけを受け、`file:` は受けない。fd は `detachFd` で渡し、Rust 側で `File::from_raw_fd` して
//!   `FileInbox::receive_file` に渡す（読み取りと検査は共通側）
//! - manifest の intent-filter は `ACTION_VIEW` + `text/plain`（`fandhe_device::file_association::MIME`）
//! - 名前は `OpenableColumns.DISPLAY_NAME`。取れなければ名前なしとして拒否に数える
//! - コールドスタートの `getIntent()` と `onNewIntent` の両方を受ける
//! - 本ファイルの `implementation()` のシグネチャは変えない（dispatch は触らない）

use fandhe_device::file_association::{FileAssociation, UnsupportedFileAssociation};

/// Android の実装を返す。b-android が実体に差し替える。
pub(crate) fn implementation() -> &'static dyn FileAssociation {
    &UnsupportedFileAssociation
}
