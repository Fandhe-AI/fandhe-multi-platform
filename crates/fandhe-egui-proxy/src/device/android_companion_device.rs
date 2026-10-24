//! Android 向け付属機器のペアリング（`CompanionDeviceManager`）の骨格（親 #143、Issue #740）。
//! `device/companion_device.rs` の Android 経路（dispatch）から呼ばれる。実体は #741 が本ファイルと
//! `CompanionDeviceHelper.java` に足す。現状は常に `Unsupported` を返し、成功を装わない。
//!
//! # #741 が埋めるときの約束
//!
//! - 触ってよいのは本ファイルの中身と Java 側だけ。dispatch・fallback 行・`device.rs` の列・
//!   `android.rs` の reset 行・共通コア画面・`registry.rs` は触らない
//! - 機能は `CompanionDeviceHelper.java` を新設して分け、要求 id の追跡は `RequestTracker` を使う。
//!   JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - API 32 以下・機能なしは `Unsupported`。位置情報サービスと Bluetooth の判定は
//!   `bluetooth-classic` の Android 実装を下敷きにする
//! - `onAssociationPending` で `IntentSender` を起動する直前に
//!   `super::companion_device::try_begin_present(request_id)` を呼び、`false` なら起動しない
//! - 名前は `AssociationInfo.getDisplayName()` から取り、`DeviceName::from_os` で正規化する
//! - `disassociate(int)` は `super::companion_device::complete_from_os` より前に行う。古い要求の
//!   結果でも外す。外せなければ `Internal`
//! - 完了は 1 回だけ `complete_from_os` で送る。`RESULT_CANCELED` は `Ok(Cancelled)`、
//!   再生成・終了は `Interrupted`、未知のコードは `Internal`（成功へ倒さない）
//! - 足してよいのは Manifest の `uses-feature android.software.companion_device_setup`
//!   （`required=false`）だけ。`uses-permission` と依存は足さない
//! - 機器名と OS のエラー文をログに出さない

use std::sync::Arc;

use fandhe_device::companion_device::{CompanionDevicePicker, UnsupportedCompanionDevicePicker};

/// Android 用の実装を返す。#741 までは `Unsupported` 固定。
pub(crate) fn implementation() -> Arc<dyn CompanionDevicePicker> {
    Arc::new(UnsupportedCompanionDevicePicker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::companion_device::CompanionDeviceFailure;

    #[test]
    fn stub_never_succeeds() {
        assert_eq!(
            implementation().pick(1),
            Err(CompanionDeviceFailure::Unsupported)
        );
    }
}
