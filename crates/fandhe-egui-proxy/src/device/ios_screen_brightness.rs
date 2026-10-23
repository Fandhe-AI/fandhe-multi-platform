//! iOS 向け画面輝度実装のスタブ（Issue #633、親 #126）。`UnsupportedScreenBrightness` を返すだけで、
//! 成功を装わない。dispatch（`device/screen_brightness.rs`）・共通コア画面・trait 契約
//! （`fandhe_device::screen_brightness`）は (a) で最終形にしてあり、後続の #634 は本ファイルの
//! `implementation()` の中身と OS の付帯物だけを埋める（dispatch・画面・fallback 行・
//! `device.rs` の `pub mod` 列は触らない）。
//!
//! # 後続 #634 が埋める範囲
//!
//! - `ios/fandhe_ios_screen_brightness.m`。取得元は前面の `UIWindowScene.screen` で、
//!   `UIScreen.main` は使わない。scene 探索は `fandhe_ios_common.h`
//! - main スレッド限定の状態（original・applied・desired）と、
//!   `brightnessDidChangeNotification` の observer トークン保持
//! - `didEnterBackground` で利用者の値へ戻し、前面復帰で掛け直す
//! - `NEAR_EQUAL_TOLERANCE` による利用者の変更の判定
//! - 状態を更新した直後の `crate::ios::request_repaint`
//! - `build.rs` への `.m` の追加

use std::sync::Arc;

use fandhe_device::screen_brightness::{ScreenBrightness, UnsupportedScreenBrightness};

/// `device/screen_brightness.rs` の iOS 経路から呼ばれる。#634 までは非対応を返す。
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
