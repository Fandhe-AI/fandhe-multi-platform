//! iOS 向け動画の撮影実装のスタブ（Issue #821、親 #169）。`device/video_capture.rs` の
//! iOS 経路（`platform`）から呼ばれ、#822 が実体で置き換える。それまでは
//! `UnsupportedVideoCapturer` を返し、成功を装わない。
//!
//! # #822 が埋める範囲
//!
//! `.m` + `extern "C"`、`UIImagePickerController`（`sourceType = .camera`、
//! `mediaTypes = [public.movie]`、`videoMaximumDuration`）、`AVCaptureDevice.requestAccess`、
//! `AVURLAsset` の読み込み API で長さ・`naturalSize`・`preferredTransform`、撮影 UI を閉じてから
//! 結果を返す、main queue と `@autoreleasepool`、完了直後の `crate::ios::request_repaint`、
//! `mediaURL` の削除（成功・取り消し・失敗の全経路）、usage description の文言追加。
//! 契約は `fandhe_device::video_capture`。OS から Rust へ渡す数値は同モジュールの
//! `result_from_code` の表を使い、`complete_from_os` は 1 要求につき 1 回だけ呼ぶ。
//! 表示の直前に `try_begin_present` を呼び、`false` なら出さない。長さ・寸法・サイズは OS の
//! メタデータから読み、経過時間で作らない。パス・OS のエラー文をログに出さない。期限・タイマーを
//! 持たない。
//!
//! 触るのは本ファイルと OS 固有の付帯物だけで、dispatch・fallback 行・`device.rs` の `pub mod` 列・
//! 共通コア画面・`registry.rs` は触らない。

use std::sync::Arc;

use fandhe_device::video_capture::{UnsupportedVideoCapturer, VideoCapturer};

/// iOS の `VideoCapturer` 実装を返す（スタブ。#822 が実体へ置き換える）。
pub(crate) fn implementation() -> Arc<dyn VideoCapturer> {
    Arc::new(UnsupportedVideoCapturer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::video_capture::VideoCaptureFailure;

    #[test]
    fn stub_never_succeeds() {
        assert_eq!(
            implementation().capture(1, 30),
            Err(VideoCaptureFailure::Unsupported)
        );
    }
}
