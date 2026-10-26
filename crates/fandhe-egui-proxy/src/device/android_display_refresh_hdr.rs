//! Android 向け「画面のリフレッシュレート・HDR・広色域」の骨格（親 #177、Issue #870）。
//! `device/display_refresh_hdr.rs` の Android 経路（dispatch）から呼ばれる。実体は
//! (b-android, #872) が埋めるまで `Unsupported` を返すスタブで、成功を装わない。契約の SSOT は
//! `fandhe_device::display_refresh_hdr`。
//!
//! # (b-android) への約束
//!
//! - 触ってよいのは本ファイル・Java ヘルパー・JNI・`build.rs` の Android 分岐だけ。dispatch・
//!   fallback 行・`device.rs`・共通コア画面・`registry.rs` は触らない
//! - JSON（`display-refresh-hdr.json`）の Android の api と note に従う
//! - minSdk 24 のため API レベルで分岐する。`Activity.getDisplay()`（API 30+）は使わず
//!   `getWindowManager().getDefaultDisplay()` を使う
//! - JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - 共通 helper（`hz_from_raw`・`require_any`）を使う

use fandhe_device::display_refresh_hdr::{DisplayRefreshHdr, UnsupportedDisplayRefreshHdr};

/// Android の `DisplayRefreshHdr` 実装を返す。実体が入るまでは `Unsupported`。
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
