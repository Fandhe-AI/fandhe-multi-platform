//! Windows 向け「指定した Wi-Fi への接続」実装のスタブ（Issue #791 の骨格、実装は #792）。
//! `device/wifi_connect.rs` の dispatch（Windows 経路）から呼ばれ、契約は
//! `fandhe_device::wifi_connect`。現状は `UnsupportedWifiConnect`（成功を返さない）を返す。
//!
//! # #792 への約束
//!
//! 触るのはこのファイルだけ。dispatch・fallback 行・`device.rs` の `pub mod` 列・共通コア画面・
//! `registry.rs` は触らない。契約は `fandhe_device::wifi_connect` の「OS ごとの写像」節に従う。
//! 要点は次のとおり。
//!
//! - `WlanConnect` の戻り値だけでは成功にせず、`connection_complete` の通知を待つ。SSID と
//!   プロファイル種別で通知を照合する
//! - 登録 → 接続 → 解除 → context の解放 → ハンドルを閉じる、の順を全経路（期限切れ・失敗を含む）で守る
//! - プロファイル XML は SSID を 16 進で入れ、パスワードは XML エスケープして入れる。WPA2-Personal
//!   （AES）と暗号なしだけを扱う
//! - HRESULT・reason code は契約の失敗への写像に使うが、ログ・エラーには出さない

use std::sync::Arc;

use fandhe_device::wifi_connect::{UnsupportedWifiConnect, WifiConnect};

/// Windows の `WifiConnect` 実装を返す（#792 が実体に差し替える）。
pub(crate) fn implementation() -> Arc<dyn WifiConnect> {
    Arc::new(UnsupportedWifiConnect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::wifi_connect::validate;

    #[test]
    fn stub_is_not_a_fake_success() {
        let req = validate("home", "").expect("検査を通る");
        assert!(implementation().connect(&req).is_err());
    }
}
