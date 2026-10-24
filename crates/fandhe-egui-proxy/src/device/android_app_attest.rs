//! Android 向け端末・アプリの真正性証明（Issue #677 で骨格、#678 が実体。親 #132）。
//! 現状は `UnsupportedAppAttest` を返すスタブで、成功は返さない。
//!
//! 呼び出し元: `device/app_attest.rs` の Android 経路が [`implementation`] を呼ぶ。Activity 再生成時の
//! 保留破棄は `android.rs` が `app_attest::reset_pending_tracking` で行う（配線済み）。dispatch・
//! fallback・`device.rs` の `pub mod`・共通コアの画面には触らない。契約の SSOT は
//! `fandhe_device::app_attest`。対応表では実体が入るまで `possible` のままにし、`impl` へ書かない。
//!
//! # #678 が埋めるときの約束
//!
//! - Java 側 `AppAttestHelper.java` が要求ごとに一意な alias の EC 鍵を作る。
//!   `setAttestationChallenge` を付け、`getCertificateChain` を呼び、finally で `deleteEntry` する
//! - `fill_random` は Java の `SecureRandom`（固定値・擬似乱数で埋めない）
//! - JNI の署名は Java・本ファイルのスタブ・実体の 3 か所で一致させる
//! - alias・challenge・証明データをログに出さない（security.md A09）
//! - 結果コードの写像は `fandhe_device::app_attest` の写像表に従う。失敗を空チェーンの成功へ畳まない
//! - 後続は本ファイルと Java 側だけを触る

use std::sync::Arc;

use fandhe_device::app_attest::{AppAttest, UnsupportedAppAttest};

/// Android の実装を返す。実体が入るまでは `UnsupportedAppAttest`（常に `Unsupported`）。
pub(crate) fn implementation() -> Arc<dyn AppAttest> {
    Arc::new(UnsupportedAppAttest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::app_attest::{validate_challenge, CHALLENGE_LEN};
    use fandhe_device::DeviceError;
    use std::time::Duration;

    #[test]
    fn stub_never_succeeds() {
        let s = implementation();
        let c = validate_challenge(&[1; CHALLENGE_LEN]).unwrap();
        assert!(matches!(
            s.attest(&c, Duration::from_secs(1)),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            s.fill_random(&mut [0; 8]),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
