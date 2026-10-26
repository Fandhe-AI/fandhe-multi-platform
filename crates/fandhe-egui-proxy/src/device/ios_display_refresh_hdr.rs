//! iOS 向け「画面のリフレッシュレート・HDR・広色域」の骨格（親 #177、Issue #870）。
//! `device/display_refresh_hdr.rs` の iOS 経路（dispatch）から呼ばれる。実体は (b-ios, #871) が
//! 埋めるまで `Unsupported` を返すスタブで、成功を装わない。契約の SSOT は
//! `fandhe_device::display_refresh_hdr`。
//!
//! # (b-ios) への約束
//!
//! - 触ってよいのは本ファイル・`ios/fandhe_ios_display_refresh_hdr.m`・`build.rs` の iOS 分岐だけ。
//!   dispatch・fallback 行・`device.rs`・共通コア画面・`registry.rs` は触らない
//! - JSON（`display-refresh-hdr.json`）の iOS の api と note に従う
//! - main スレッドで同期に読む。窓の画面は `window.windowScene.screen`
//! - 今の Hz は常に `None`（`CADisplayLink` の毎フレーム観測を要し、推定しない）。
//!   `currentEDRHeadroom` は使わない。`displayGamut` が `.unspecified` なら広色域は `None`
//! - 共通 helper（`hz_from_raw`・`hdr_from_headroom`・`require_any`）を使う

use fandhe_device::display_refresh_hdr::{DisplayRefreshHdr, UnsupportedDisplayRefreshHdr};

/// iOS の `DisplayRefreshHdr` 実装を返す。実体が入るまでは `Unsupported`。
pub(crate) fn implementation() -> Box<dyn DisplayRefreshHdr> {
    Box::new(UnsupportedDisplayRefreshHdr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_unsupported() {
        assert!(matches!(
            implementation().read(),
            Err(DeviceError::Unsupported("display_refresh_hdr"))
        ));
    }
}
