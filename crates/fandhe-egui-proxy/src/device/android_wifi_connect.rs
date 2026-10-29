//! Android 向け「指定した Wi-Fi への接続」実装のスタブ（Issue #1064 の骨格、実装は #1065）。
//! `device/wifi_connect.rs` の dispatch（Android 経路）から呼ばれ、契約の SSOT は
//! `fandhe_device::wifi_connect`。現状は `UnsupportedWifiConnect`（成功を返さない）を返す。
//!
//! # #1065 への約束
//!
//! Rust 側で触るのはこのファイルだけ。dispatch・fallback 行・`device.rs` の `pub mod` 列・共通コア画面・
//! `registry.rs` は触らない。Java 側（`WifiConnectHelper.java` の新規作成、`MainActivity.java` への委譲、
//! Manifest の `CHANGE_NETWORK_STATE`）と対応表の Android セルは #1065 が担当する。契約は trait doc の
//! 「OS ごとの写像」節に従う。要点は次のとおり。
//!
//! - `RequestSlot` で id を採番し、JNI 経由で Java の `RequestTracker` と照合する
//! - 期限は持たない（`android_wifi_info.rs` の `READ_TIMEOUT` は写さない）
//! - 成功は最初の `onAvailable` だけ。`onUnavailable` の後に電源を読み直し、`RADIO_OFF` とそれ以外の
//!   `CONNECT_FAILED` に分ける
//! - 16 進 64 文字の鍵は OS へ渡さず `CONNECT_FAILED`。判定は Rust 側で行い、JNI へは bool で渡す
//!   （定数を Java に複製しない）
//! - API 28 以下は `Unsupported`
//! - SSID・パスワードはログ・例外文に出さず、trim しない

use std::sync::Arc;

use fandhe_device::wifi_connect::{UnsupportedWifiConnect, WifiConnect};

/// Android の `WifiConnect` 実装を返す（#1065 が実体に差し替える）。
pub(crate) fn implementation() -> Arc<dyn WifiConnect> {
    Arc::new(UnsupportedWifiConnect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::wifi_connect::validate;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_not_a_fake_success() {
        let req = validate("home", "").expect("検査を通る");
        assert!(matches!(
            implementation().connect(&req),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
