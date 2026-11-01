//! Issue #1130（親 #114）: Android 向け決済シートの骨格。`device/payment_request.rs` の
//! `drive_callback`（Android 経路）から呼ばれる。**#1131 が埋めるまで常に `Unsupported` を返し、
//! 成功を装わない**。
//!
//! #1131 が埋める範囲（これ以外は触らない。dispatch・共通コア画面・`device.rs` の `pub mod`・
//! `fandhe_device::payment_request` の trait doc は (a) で確定済み）:
//!
//! - Java の `PaymentRequestHelper`（GMS 可否・`isReadyToPay`・`loadPaymentData`・`ENVIRONMENT_TEST`）
//! - `MainActivity` の最小追記
//! - JNI 受け口から `payment_request::bridge::deliver_check` / `deliver_pay` を呼ぶこと
//! - 要求 JSON は `fandhe_device::payment_request::ANDROID_*_JSON` を JNI で渡す（Java に複製しない）
//! - manifest の `com.google.android.gms.wallet.api.enabled`
//! - 対応表 JSON（`payment-request.json`）の android セル

use fandhe_core::screens::payment_request::PaymentAction;
use fandhe_device::DeviceError;

/// OS へ依頼する。`Ok` は受理済みで結果は後から `bridge::deliver_*` で届く。
/// 骨格では常に `Err(Unsupported)`（呼び出し元がその場で結果を置く）。
pub(crate) fn begin(_request_id: u64, _action: PaymentAction) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("payment_request"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_never_succeeds() {
        for a in [PaymentAction::Check, PaymentAction::Pay] {
            assert!(matches!(
                begin(1, a),
                Err(DeviceError::Unsupported("payment_request"))
            ));
        }
    }
}
