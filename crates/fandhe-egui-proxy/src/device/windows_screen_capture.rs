//! Windows 向け画面録画のスタブ（Issue #1335）。Windows は今回対象外
//! （#1339 は not planned）のため常に `Unsupported`。
//! `device/screen_capture.rs` の dispatch から呼ばれる。

use fandhe_device::screen_capture::{ScreenCapturer, UnsupportedScreenCapturer};

/// OS 別エンジンを返す。
pub(crate) fn capturer() -> &'static dyn ScreenCapturer {
    &UnsupportedScreenCapturer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_is_unsupported() {
        assert!(capturer()
            .start(1, std::time::Duration::from_secs(1))
            .is_err());
    }
}
