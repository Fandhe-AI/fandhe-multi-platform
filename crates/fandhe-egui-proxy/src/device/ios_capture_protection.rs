//! iOS 向け画面キャプチャの検知実装のスタブ（Issue #582 (a)、実体は #583）。
//! `device/capture_protection.rs` の iOS 経路から呼ばれる。
//!
//! #583 が埋める範囲: `.m` で `sceneCaptureState`（iOS 17+）を読み、スクリーンショット通知を
//! 初回に 1 回だけ登録して atomic で数える（登録トークンは static の strong 参照で持つ）。
//! dispatch・共通コア画面・文言には触らない（(a) で最終形にしてある）。
//! 実体ができるまでは `Unsupported` を返し、成功を偽らない。

use std::sync::Arc;

use fandhe_device::capture_protection::{CaptureProtection, UnsupportedCaptureProtection};

/// iOS の `CaptureProtection` 実装を返す（#583 で差し替える）。
pub(crate) fn implementation() -> Arc<dyn CaptureProtection> {
    Arc::new(UnsupportedCaptureProtection)
}
