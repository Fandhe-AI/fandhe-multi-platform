//! Android 向け動画の撮影実装のスタブ（Issue #821、親 #169）。`device/video_capture.rs` の
//! Android 経路（`platform`）から呼ばれ、#823 が実体で置き換える。それまでは
//! `UnsupportedVideoCapturer` を返し、成功を装わない。
//!
//! # #823 が埋める範囲
//!
//! Java helper と `RequestTracker`、`CAMERA` の実行時許可、`ACTION_VIDEO_CAPTURE` +
//! `EXTRA_OUTPUT`（既存 `FileProvider` に `cache/video_capture/` を 1 行足す）+
//! `EXTRA_DURATION_LIMIT`、`resolveActivity` で事前判定せず `ActivityNotFoundException` を
//! `NoCameraApp` へ写す、`MediaMetadataRetriever` は `release()`（minSdk 24）、撮影前に置き場を
//! 空にする、`onDestroy` で確定、JNI 署名は Java・no-op スタブ・実体の 3 か所で一致、
//! `<queries>` と `RECORD_AUDIO` は足さない。成功・取り消し・失敗・破棄の全経路で一時ファイルを消す。
//! 契約は `fandhe_device::video_capture`。OS から Rust へ渡す数値は同モジュールの
//! `result_from_code` の表を使い、`complete_from_os` は 1 要求につき 1 回だけ呼ぶ。
//! 表示の直前に `try_begin_present` を呼び、`false` なら出さない。長さ・寸法・サイズは OS の
//! メタデータから読み、経過時間で作らない。パス・OS のエラー文をログに出さない。期限・タイマーを
//! 持たない。
//!
//! 触るのは本ファイルと OS 固有の付帯物だけで、dispatch・fallback 行・`device.rs` の `pub mod` 列・
//! `android.rs` の reset 行・共通コア画面・`registry.rs` は触らない。

use std::sync::Arc;

use fandhe_device::video_capture::{UnsupportedVideoCapturer, VideoCapturer};

/// Android の `VideoCapturer` 実装を返す（スタブ。#823 が実体へ置き換える）。
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
