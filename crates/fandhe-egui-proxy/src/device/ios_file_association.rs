//! iOS 向けファイルの関連付け受信（Issue #417。親 #102 の b-ios）。
//! `device/file_association.rs::platform` の iOS 経路から呼ばれ、macOS と共用の受け口
//! （`apple_file_association.rs` の `INBOX`）を返す。受信は `apple/fandhe_apple_file_association.m`
//! の `TARGET_OS_IOS` 分岐が scene の通知から行う。

use fandhe_device::file_association::FileAssociation;

/// iOS の実装を返す。
pub(crate) fn implementation() -> &'static dyn FileAssociation {
    &super::apple_file_association::AppleFileAssociation
}
