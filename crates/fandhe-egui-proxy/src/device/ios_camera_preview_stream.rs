//! iOS 向けカメラのライブ映像実装のスタブ（Issue #812、親 #168）。`device/camera_preview_stream.rs` の
//! iOS 経路（`platform_camera_preview`）から呼ばれ、#813 が実体で置き換える。それまでは
//! `UnsupportedCameraPreview` を返し、成功を装わない。
//!
//! # #813 が埋める範囲
//!
//! `.m` + `extern "C"`、`AVCaptureSession` と serial queue、BGRA と `downscale_packed`、
//! `videoZoomFactor` と `lockForConfiguration`、中断・実行時エラーの通知、背景遷移、
//! `NSCameraUsageDescription` の文言、`@autoreleasepool`。契約は `fandhe_device::camera_preview_stream`。
//! OS のコールバックは `device/camera_preview_stream.rs` の `register` で得た `id` を持ち、
//! `deliver_frame` / `deliver_event` を呼ぶ（イベントコードは `event_from_code` の表を使う）。
//!
//! 触るのは本ファイルと OS 固有の付帯物だけで、dispatch・共通コア画面・fallback 行・`device.rs` の
//! `pub mod` 列は触らない。

use std::sync::Arc;

use fandhe_device::camera_preview_stream::{CameraPreview, UnsupportedCameraPreview};

/// iOS の `CameraPreview` 実装を返す（スタブ。#813 が実体へ置き換える）。
pub(crate) fn implementation() -> Arc<dyn CameraPreview> {
    Arc::new(UnsupportedCameraPreview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::camera_preview_stream::{
        PreviewEvent, PreviewFrame, PreviewSink, PreviewStartError,
    };

    struct NopSink;
    impl PreviewSink for NopSink {
        fn frame(&self, _: PreviewFrame) {}
        fn event(&self, _: PreviewEvent) {}
    }

    #[test]
    fn stub_never_succeeds() {
        let r = implementation().start(Arc::new(NopSink));
        assert!(matches!(r, Err(PreviewStartError::Unsupported)));
    }
}
