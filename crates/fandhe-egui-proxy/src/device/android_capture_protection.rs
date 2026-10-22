//! Android 向け画面キャプチャの防止・検知実装のスタブ（Issue #582 (a)、実体は #584）。
//! `device/capture_protection.rs` の Android 経路から呼ばれる。
//!
//! #584 が埋める範囲: `CaptureProtectionHelper.java`（`FLAG_SECURE`、API 34+/35+ の検知）、
//! JNI 署名を Java 側・no-op スタブ・実体の 3 か所でそろえること、Manifest の 2 権限、
//! Activity 再生成時の再適用。dispatch・共通コア画面・文言には触らない（(a) で最終形にしてある）。
//! 実体ができるまでは `Unsupported` を返し、成功を偽らない。

use std::sync::Arc;

use fandhe_device::capture_protection::{CaptureProtection, UnsupportedCaptureProtection};

/// Android の `CaptureProtection` 実装を返す（#584 で差し替える）。
pub(crate) fn implementation() -> Arc<dyn CaptureProtection> {
    Arc::new(UnsupportedCaptureProtection)
}
