//! Android 向け画面輝度実装のスタブ（Issue #633、親 #126）。`UnsupportedScreenBrightness` を返すだけで、
//! 成功を装わない。dispatch（`device/screen_brightness.rs`）・共通コア画面・trait 契約
//! （`fandhe_device::screen_brightness`）は (a) で最終形にしてあり、後続の #635 は本ファイルの
//! `implementation()` の中身と OS の付帯物だけを埋める（dispatch・画面・fallback 行・
//! `device.rs` の `pub mod` 列は触らない）。
//!
//! # 後続 #635 が埋める範囲
//!
//! - `ScreenBrightnessHelper.java`
//! - `WindowManager.LayoutParams.screenBrightness`（解除は `BRIGHTNESS_OVERRIDE_NONE`）
//! - UI スレッドへの post を期限付きで待ち、適用開始と放棄を CAS で排他にすること
//! - Activity 再生成での static 持ち越し
//! - 上書きしていない間は `BrightnessReading::level` を `None` にすること
//! - JNI 署名を Java・no-op スタブ・実体の 3 か所でそろえること

use std::sync::Arc;

use fandhe_device::screen_brightness::{ScreenBrightness, UnsupportedScreenBrightness};

/// `device/screen_brightness.rs` の Android 経路から呼ばれる。#635 までは非対応を返す。
pub(crate) fn implementation() -> Arc<dyn ScreenBrightness> {
    Arc::new(UnsupportedScreenBrightness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_never_succeeds() {
        let imp = implementation();
        assert!(imp.set_override(0.5).is_err());
        assert!(imp.clear_override().is_err());
        assert!(imp.read().is_err());
    }
}
