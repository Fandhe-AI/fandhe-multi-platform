//! Android 向け折りたたみ姿勢の OS 別モジュール（Issue #1097 のスタブ、実装は #1098）。
//! `device/foldable_posture.rs` の `implementation()` が Android ビルドでのみ呼ぶ。
//!
//! #1098 への約束: Rust で触るのは本ファイルだけ。写像は `fandhe_device::foldable_posture`
//! のモジュール doc「Android の写像」と守る範囲に従う。Java 側（`MainActivity` の `onStart` で
//! `WindowLayoutInfo` を購読、`onStop` で解除してキャッシュを捨てる）と JNI を足し、
//! 未受信は `Ok(None)`、取れない・壊れた値は `Internal` を返す。それまでは `Unsupported` で、
//! 成功値を偽らない。

use std::sync::Arc;

use fandhe_device::foldable_posture::{FoldablePostureSource, UnsupportedFoldablePosture};

/// Android の実装を返す。現在はスタブ（常に `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn FoldablePostureSource> {
    Arc::new(UnsupportedFoldablePosture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_not_a_fake_success() {
        assert!(matches!(
            implementation().read(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
