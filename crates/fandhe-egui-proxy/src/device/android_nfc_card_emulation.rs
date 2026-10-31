//! Android 向け NFC カードエミュレーション（HCE）の骨格（Phase 11 の拡充ループ、親 #151、Issue #1128）。
//! `device/nfc_card_emulation.rs` の Android 経路から呼ばれ、いまは `UnsupportedCardEmulation` を返すだけ。
//!
//! 後続の Android 子 issue（#1129）は **このファイルだけ** を埋め、dispatch・共通コア画面・
//! `device.rs` の `pub mod` 列には触れない。埋める範囲:
//!
//! - `with_activity`（`android_car_display` と同形、`// SAFETY:` 付き）
//! - JNI の受け口 1 本 `nfcCardEmulationStatus(String aid)`。署名は Java・Rust の 2 か所で一致させ、
//!   未取得は番兵値ではなく `Option` へ写す純関数で扱う
//! - Activity の不在・JNI 失敗は `Internal`（`Unsupported` にしない）
//! - Java の `HostApduService`（`BIND_NFC_SERVICE` で bind をシステムに限る）・manifest・
//!   `res/xml/apduservice.xml`（AID の複製は `include_str!` テストで固定）・strings・JSON の android セルと note

use std::sync::Arc;

use fandhe_device::nfc_card_emulation::{CardEmulation, UnsupportedCardEmulation};

/// Android の実装。いまは非対応固定（#1129 が置き換える）。
pub(crate) fn implementation() -> Arc<dyn CardEmulation> {
    Arc::new(UnsupportedCardEmulation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_never_succeeds() {
        assert!(matches!(
            implementation().status(),
            Err(DeviceError::Unsupported("nfc_card_emulation"))
        ));
    }
}
