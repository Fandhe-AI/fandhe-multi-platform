//! Android 向けシステム音量実装のスタブ（Issue #785 の骨格、実装は #787）。
//! `device/volume_control.rs` の dispatch（Android 経路）から呼ばれ、契約は
//! `fandhe_device::volume_control`。現状は `UnsupportedVolumeControl`（成功を返さない）を返す。
//!
//! # #787 への約束
//!
//! 触るのはこのファイルと `VolumeControlHelper.java`、JNI だけ。dispatch・fallback 行・
//! `device.rs` の `pub mod` 列・共通コア画面・`registry.rs` は触らない。契約は
//! `fandhe_device::volume_control` の「OS ごとの写像」節に従う。要点は次のとおり。
//!
//! - `STREAM_MUSIC` の `getStreamVolume / getStreamMaxVolume` を `level` にし、`isStreamMute` は
//!   API 23 未満（minSdk 未満）では `None`、`isVolumeFixed` が真なら `NotChangeable`
//! - `adjustStreamVolume(ADJUST_RAISE | ADJUST_LOWER, 0)` で 1 段階動かし、同じ呼び出しの中で読み戻す
//! - `SecurityException`（おやすみモード）は `PermissionDenied`
//! - JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる

use std::sync::Arc;

use fandhe_device::volume_control::{UnsupportedVolumeControl, VolumeControl};

/// Android の `VolumeControl` 実装を返す（#787 が実体に差し替える）。
pub(crate) fn implementation() -> Arc<dyn VolumeControl> {
    Arc::new(UnsupportedVolumeControl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::volume_control::StepDirection;

    #[test]
    fn stub_is_not_a_fake_success() {
        let v = implementation();
        assert!(v.read().is_err());
        assert!(v.step(StepDirection::Down).is_err());
    }
}
