//! Android 向けシステムの効果音の骨格（Issue #939。実体は #940）。`device/system_sound.rs` の
//! Android 経路から呼ばれる。現状は `Unsupported` を返すだけで、成功へは倒さない。
//! 契約は `fandhe_device::system_sound`。
//!
//! # #940（b-android）への約束
//!
//! - 触ってよいのはこのファイル、`SystemSoundHelper.java`、`MainActivity` の委譲、対応表の Android セルだけ
//! - JNI 署名 `systemSoundBeep()I` を Java・`MainActivity`・Rust の 3 か所で一致させる
//! - 結果コードの案: 0 = `Requested`（`startTone` が true）、1 = `NotPlayed`（false）、それ以外は `Internal`
//! - `ToneGenerator` は 1 個を遅延生成し、生成に失敗したら保持しない。`onDestroy` で `release()` する

use std::sync::Arc;

use fandhe_device::system_sound::{SystemSound, UnsupportedSystemSound};

/// 骨格の実装を返す。#940 が JNI 実装に置き換える。
pub(crate) fn implementation() -> Arc<dyn SystemSound> {
    Arc::new(UnsupportedSystemSound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn skeleton_is_unsupported() {
        assert!(matches!(
            implementation().beep(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
