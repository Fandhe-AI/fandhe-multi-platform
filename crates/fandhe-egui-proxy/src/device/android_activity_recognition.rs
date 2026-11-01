//! Issue #1126（親 #124）: Android 向け行動認識の骨格。`device/activity_recognition.rs` の
//! Android 経路が [`implementation`] を呼ぶ。現状は `Unsupported` を返す薄いスタブで、
//! b-android が Play services `ActivityRecognitionClient` の実体（JNI・Receiver）に差し替える。
//! 値の写像は `fandhe_device::activity_recognition::classify_detected` を通す。

use std::sync::Arc;

use fandhe_device::activity_recognition::{ActivityRecognizer, UnsupportedActivityRecognizer};

/// Android の実装を返す（b-android が実体に差し替える）。
pub(crate) fn implementation() -> Arc<dyn ActivityRecognizer> {
    Arc::new(UnsupportedActivityRecognizer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::activity_recognition::ActivityFailure;

    #[test]
    fn host_never_succeeds() {
        let r = implementation();
        assert_eq!(r.start(), Err(ActivityFailure::Unsupported));
        assert_eq!(r.read(), Err(ActivityFailure::Unsupported));
    }
}
