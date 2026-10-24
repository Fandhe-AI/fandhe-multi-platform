//! iOS 向けシステム音量実装のスタブ（Issue #785 の骨格、実装は #786）。
//! `device/volume_control.rs` の dispatch（iOS 経路）から呼ばれ、契約は
//! `fandhe_device::volume_control`。現状は `UnsupportedVolumeControl`（成功を返さない）を返す。
//!
//! # #786 への約束
//!
//! 触るのはこのファイルと `ios/fandhe_ios_volume_control.m`、`build.rs` の iOS 分岐だけ。
//! dispatch・fallback 行・`device.rs` の `pub mod` 列・共通コア画面・`registry.rs` は触らない。
//! 契約は `fandhe_device::volume_control` の「OS ごとの写像」節に従う。要点は次のとおり。
//!
//! - `AVAudioSession.outputVolume` を `level` にし、`muted` は `None`、`mode` は常に `OsSlider`
//! - `MPVolumeView` は画面に 1 個だけ置き、音声セッションの category・有効化は変えない
//! - UI を要する呼び出しは main queue で行い、取得後に `crate::ios::request_repaint` で次フレームを起こす
//! - `step` は OS へ渡さず `Ok(StepOutcome::NotChangeable(reading))` を返す

use std::sync::Arc;

use fandhe_device::volume_control::{UnsupportedVolumeControl, VolumeControl};

/// iOS の `VolumeControl` 実装を返す（#786 が実体に差し替える）。
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
        assert!(v.step(StepDirection::Up).is_err());
    }
}
