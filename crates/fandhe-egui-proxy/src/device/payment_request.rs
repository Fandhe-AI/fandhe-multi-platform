//! Phase 11（親 #114）型サンプル「物・サービスの決済」（Issue #541）の effect ドライバ。
//! `fandhe_core::screens::payment_request`（端末依存ゼロ）の要求を観測し、`fandhe_device::payment_request`
//! の [`PaymentSheet`] を呼んで結果を `CompletionSender` 経由で返す
//! （`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `fandhe_device::payment_request` のモジュール doc を参照。
//!
//! - Android（Issue #1130 で骨格、#1131 で実装）: コールバック型のため pull 型 [`bridge`]
//!   （`RequestSlot` の facade）で毎フレーム [`drive_callback`] が結果を反映する。
//!   `android_payment_request::begin` が Google Pay API（TEST）を JNI で呼ぶ
//! - それ以外のネイティブ（macOS・Linux・Windows・iOS）は対応表で保留または非対応のため、同期で
//!   [`UnsupportedPaymentSheet`] を呼び「使えない」を表示する
//!
//! **dispatch はこれが最終形**で、後続の子は触らない。Web は `fandhe-web-dom`。

use fandhe_core::screens::payment_request::{
    finish, mark_running, take_request, CoreOutcome, PaymentAction, PaymentRequestState,
};
use fandhe_device::payment_request::PaymentOutcome;
#[cfg(any(not(target_os = "android"), test))]
use fandhe_device::payment_request::{PaymentSheet, UnsupportedPaymentSheet};
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

/// OS 側コールバックとの橋渡し（Android、pull 型、`RequestSlot` の facade）。
/// `deliver_*` は `android_payment_request` の JNI 受け口が呼ぶ。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) mod bridge {
    use super::{check_outcome, pay_outcome, CoreOutcome, PaymentOutcome};
    use crate::device::request_slot::RequestSlot;
    use fandhe_device::DeviceError;

    pub(crate) type Outcome = Result<CoreOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 「使えるか」の結果を置く。不一致・未登録・既に結果ありは捨てて `false`。
    pub(crate) fn deliver_check(request_id: u64, result: Result<bool, DeviceError>) -> bool {
        SLOT.deliver(request_id, check_outcome(result))
    }

    /// 「決済シート」の結果を置く。規則は [`deliver_check`] と同じ。
    pub(crate) fn deliver_pay(
        request_id: u64,
        result: Result<PaymentOutcome, DeviceError>,
    ) -> bool {
        SLOT.deliver(request_id, pay_outcome(result))
    }

    /// Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、旧 `request_id`
    /// の完了を無視させる。
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

/// Android 版の effect ドライバ本体。`begin` が `Err` なら依頼自体の失敗としてその場で結果を置く。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn drive_callback(
    state: &PaymentRequestState,
    sender: &CompletionSender<PaymentRequestState>,
    slot: &'static crate::device::request_slot::RequestSlot<bridge::Outcome>,
    begin: fn(u64, PaymentAction) -> Result<(), DeviceError>,
) {
    use fandhe_core::screens::payment_request::PaymentPhase;

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_running));
        let request_id = slot.begin();
        if let Err(e) = begin(request_id, view.action) {
            let outcome = match view.action {
                PaymentAction::Check => check_outcome(Err(e)),
                PaymentAction::Pay => pay_outcome(Err(e)),
            };
            slot.deliver(request_id, outcome);
        }
        return;
    }
    if state.phase != PaymentPhase::Running {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        slot,
        sender,
        |_request_id| {},
        move |s: &mut PaymentRequestState, outcome| {
            if s.phase != PaymentPhase::Idle {
                finish(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

#[cfg(any(not(target_os = "android"), test))]
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

/// `registry.rs` から配線される effect ドライバ（Android: pull 型 bridge）。
#[cfg(target_os = "android")]
pub fn payment_request_effect(
    state: &PaymentRequestState,
    sender: &CompletionSender<PaymentRequestState>,
) {
    drive_callback(
        state,
        sender,
        &bridge::SLOT,
        super::android_payment_request::begin,
    );
}

/// `registry.rs` から配線される effect ドライバ（Android 以外。常に「使えない」を表示する）。
#[cfg(not(target_os = "android"))]
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

    /// フェイク begin を使い、独立 slot で bridge 経路を 1 フレームずつ進める。
    fn frame(
        s: &mut PaymentRequestState,
        slot: &'static crate::device::request_slot::RequestSlot<bridge::Outcome>,
        begin: fn(u64, PaymentAction) -> Result<(), DeviceError>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive_callback(s, &sender, slot, begin);
        while let Ok(apply) = rx.try_recv() {
            apply(s);
        }
    }

    fn accept(_: u64, _: PaymentAction) -> Result<(), DeviceError> {
        Ok(())
    }

    #[test]
    fn host_begin_shows_unsupported_never_success() {
        use crate::device::android_payment_request::begin;
        for press in [screen::check, screen::pay] {
            static SLOT: crate::device::request_slot::RequestSlot<bridge::Outcome> =
                crate::device::request_slot::RequestSlot::new();
            let mut s = PaymentRequestState::default();
            press(&mut s);
            frame(&mut s, &SLOT, begin);
            frame(&mut s, &SLOT, begin);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
            SLOT.reset();
        }
    }

    #[test]
    fn cancelled_via_bridge_is_not_error() {
        static SLOT: crate::device::request_slot::RequestSlot<bridge::Outcome> =
            crate::device::request_slot::RequestSlot::new();
        let mut s = PaymentRequestState::default();
        screen::pay(&mut s);
        frame(&mut s, &SLOT, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        assert!(SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::Cancelled))));
        frame(&mut s, &SLOT, accept);
        assert_eq!(s.status, screen::CANCELLED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn failure_permission_invalid_input_never_success() {
        type Case = (PaymentAction, Result<CoreOutcome, String>);
        let cases: [Case; 4] = [
            (PaymentAction::Pay, pay_outcome(Ok(PaymentOutcome::Failed))),
            (
                PaymentAction::Check,
                check_outcome(Err(DeviceError::PermissionDenied("payment_request"))),
            ),
            (
                PaymentAction::Pay,
                pay_outcome(Err(DeviceError::InvalidInput("payment_request"))),
            ),
            (
                PaymentAction::Check,
                check_outcome(Err(DeviceError::Internal("payment_request failed"))),
            ),
        ];
        for (action, outcome) in cases {
            static SLOT: crate::device::request_slot::RequestSlot<bridge::Outcome> =
                crate::device::request_slot::RequestSlot::new();
            let mut s = PaymentRequestState::default();
            match action {
                PaymentAction::Check => screen::check(&mut s),
                PaymentAction::Pay => screen::pay(&mut s),
            }
            frame(&mut s, &SLOT, accept);
            let (id, _) = SLOT.snapshot().unwrap();
            assert!(SLOT.deliver(id, outcome));
            frame(&mut s, &SLOT, accept);
            assert!(s.status.is_empty());
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
            SLOT.reset();
        }
    }

    #[test]
    fn stale_request_id_is_dropped() {
        static SLOT: crate::device::request_slot::RequestSlot<bridge::Outcome> =
            crate::device::request_slot::RequestSlot::new();
        let mut s = PaymentRequestState::default();
        screen::pay(&mut s);
        frame(&mut s, &SLOT, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        let stale = id.wrapping_add(1);
        assert!(!SLOT.deliver(stale, pay_outcome(Ok(PaymentOutcome::TestAuthorized))));
        frame(&mut s, &SLOT, accept);
        assert_eq!(s.phase, screen::PaymentPhase::Running);
        assert!(s.status.is_empty() && s.error.is_empty());
        // first-wins: 2 回目の deliver は捨てる
        assert!(SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::Cancelled))));
        assert!(!SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::TestAuthorized))));
        frame(&mut s, &SLOT, accept);
        assert_eq!(s.status, screen::CANCELLED_STATUS);
    }

    #[test]
    fn reset_drops_pending_and_old_completion() {
        static SLOT: crate::device::request_slot::RequestSlot<bridge::Outcome> =
            crate::device::request_slot::RequestSlot::new();
        let mut s = PaymentRequestState::default();
        screen::pay(&mut s);
        frame(&mut s, &SLOT, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        SLOT.reset();
        assert!(!SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::TestAuthorized))));
        let mut fresh = PaymentRequestState::default();
        frame(&mut fresh, &SLOT, accept);
        assert!(fresh.status.is_empty() && fresh.error.is_empty());
    }

    #[test]
    fn ready_false_is_unsupported_not_available() {
        static SLOT: crate::device::request_slot::RequestSlot<bridge::Outcome> =
            crate::device::request_slot::RequestSlot::new();
        let mut s = PaymentRequestState::default();
        screen::check(&mut s);
        frame(&mut s, &SLOT, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        assert!(SLOT.deliver(id, check_outcome(Ok(false))));
        frame(&mut s, &SLOT, accept);
        assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
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
