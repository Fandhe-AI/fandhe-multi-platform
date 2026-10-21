//! Phase 11（親 #114）型サンプル「物・サービスの決済」（Issue #541）の effect ドライバ。
//! `fandhe_core::screens::payment_request`（端末依存ゼロ）の要求を観測し、`fandhe_device::payment_request`
//! の [`PaymentSheet`] を呼んで結果を `CompletionSender` 経由で返す
//! （`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `fandhe_device::payment_request` のモジュール doc を参照。
//!
//! 全ネイティブ OS（macOS・Linux・Windows・iOS・Android）は対応表で保留または非対応のため、同期で
//! [`UnsupportedPaymentSheet`] を呼び「使えない」を表示する。**dispatch はこれが最終形**で、後続の子は
//! 触らない。Web は `fandhe-web-dom`。ワーカーを持たないため `RequestSlot` は使わない。

use fandhe_core::screens::payment_request::{
    finish, mark_running, take_request, CoreOutcome, PaymentAction, PaymentRequestState,
};
use fandhe_device::payment_request::{PaymentOutcome, PaymentSheet, UnsupportedPaymentSheet};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 内部名を出さない固定文言（security.md A09）。
const FAILED_TEXT: &str = "決済シートを完了できませんでした";

fn check_outcome(result: Result<bool, DeviceError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(true) => Ok(CoreOutcome::Available),
        Ok(false) | Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

fn pay_outcome(result: Result<PaymentOutcome, DeviceError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(PaymentOutcome::TestAuthorized) => Ok(CoreOutcome::TestAuthorized),
        Ok(PaymentOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Ok(PaymentOutcome::Failed) | Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

fn drive_sync(
    state: &PaymentRequestState,
    sender: &CompletionSender<PaymentRequestState>,
    sheet: &dyn PaymentSheet,
) {
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    let outcome = match view.action {
        PaymentAction::Check => check_outcome(sheet.can_make_payments()),
        PaymentAction::Pay => pay_outcome(sheet.request()),
    };
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut PaymentRequestState| {
        finish(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ。全ネイティブで常に「使えない」を表示する。
pub fn payment_request_effect(
    state: &PaymentRequestState,
    sender: &CompletionSender<PaymentRequestState>,
) {
    drive_sync(state, sender, &UnsupportedPaymentSheet);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::payment_request as screen;

    #[test]
    fn outcomes_cover_all_arms() {
        assert_eq!(check_outcome(Ok(true)), Ok(CoreOutcome::Available));
        assert_eq!(check_outcome(Ok(false)), Ok(CoreOutcome::Unsupported));
        assert_eq!(
            check_outcome(Err(DeviceError::Unsupported("payment_request"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            check_outcome(Err(DeviceError::Internal("payment_request failed"))),
            Err(FAILED_TEXT.to_string())
        );
        assert_eq!(
            pay_outcome(Ok(PaymentOutcome::TestAuthorized)),
            Ok(CoreOutcome::TestAuthorized)
        );
        assert_eq!(
            pay_outcome(Ok(PaymentOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            pay_outcome(Ok(PaymentOutcome::Failed)),
            Err(FAILED_TEXT.to_string())
        );
        assert_eq!(
            pay_outcome(Err(DeviceError::Unsupported("payment_request"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(!FAILED_TEXT.contains("payment"));
    }

    fn run(sheet: &dyn PaymentSheet, press: fn(&mut PaymentRequestState)) -> PaymentRequestState {
        let mut s = PaymentRequestState::default();
        press(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive_sync(&s, &sender, sheet);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        s
    }

    struct Stub(fn() -> Result<PaymentOutcome, DeviceError>);
    impl PaymentSheet for Stub {
        fn can_make_payments(&self) -> Result<bool, DeviceError> {
            (self.0)().map(|_| true)
        }
        fn request(&self) -> Result<PaymentOutcome, DeviceError> {
            (self.0)()
        }
    }

    #[test]
    fn unsupported_host_never_succeeds() {
        for press in [screen::check, screen::pay] {
            let s = run(&UnsupportedPaymentSheet, press);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
        }
    }

    #[test]
    fn failure_goes_to_error() {
        let stub = Stub(|| Err(DeviceError::Internal("payment_request failed")));
        for press in [screen::check, screen::pay] {
            let s = run(&stub, press);
            assert!(s.status.is_empty() && s.error.starts_with(screen::FAILED_PREFIX));
        }
    }

    #[test]
    fn cancelled_is_not_error() {
        let s = run(&Stub(|| Ok(PaymentOutcome::Cancelled)), screen::pay);
        assert_eq!(s.status, screen::CANCELLED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn no_request_never_calls_os() {
        let s = PaymentRequestState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive_sync(&s, &sender, &Stub(|| Ok(PaymentOutcome::TestAuthorized)));
        assert!(rx.try_recv().is_err());
    }
}
