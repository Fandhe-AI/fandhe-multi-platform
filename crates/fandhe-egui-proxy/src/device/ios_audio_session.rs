//! iOS 向け音声セッション実装のスタブ（Issue #840、親 #172）。`device/audio_session.rs` の
//! iOS 経路（`platform_session`）から呼ばれ、#841 が実体で置き換える。それまでは
//! `UnsupportedAudioSession` を返し、成功を装わない。
//!
//! 契約は `fandhe_device::audio_session` のモジュール doc（方式・結果の種類・解放・守る範囲 / 守らない範囲・
//! 後続 OS の要点）。触るのは本ファイルと OS 固有の付帯物だけで、dispatch・fallback 行・`device.rs` の
//! `pub mod` 列・共通コア画面・`registry.rs` は触らない。

use std::sync::Arc;

use fandhe_device::audio_session::{AudioSession, UnsupportedAudioSession};

/// iOS の `AudioSession` 実装を返す（スタブ。#841 が実体へ置き換える）。
pub(crate) fn implementation() -> Arc<dyn AudioSession> {
    Arc::new(UnsupportedAudioSession)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::audio_session::Support;

    #[test]
    fn stub_never_succeeds() {
        assert_eq!(implementation().support(), Support::NONE);
        assert!(implementation().read_current().is_err());
    }
}
