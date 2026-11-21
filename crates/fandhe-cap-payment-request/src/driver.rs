//! Phase 11（親 #114）型サンプル「物・サービスの決済」（Issue #541）の effect ドライバ。
//! `fandhe_core::screens::payment_request`（端末依存ゼロ）の要求を観測し、`contract.rs`
//! の [`PaymentSheet`] を呼んで結果を `CompletionSender` 経由で返す
//! （`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `contract.rs` のモジュール doc を参照。
//!
//! - Android（Issue #1130 で骨格、#1131 で実装）: コールバック型のため pull 型 [`bridge`]
//!   （`RequestSlot` の facade）で毎フレーム [`drive_callback`] が結果を反映する。
//!   `android_payment_request::begin` が Google Pay API（TEST）を JNI で呼ぶ
//! - それ以外のネイティブ（macOS・Linux・Windows・iOS）は対応表で保留または非対応のため、同期で
//!   [`UnsupportedPaymentSheet`] を呼び「使えない」を表示する
//!
//! **dispatch はこれが最終形**で、後続の子は触らない。Web は `fandhe-web-dom`。

use crate::contract::{PaymentOutcome, PaymentSheet, UnsupportedPaymentSheet};
use crate::error::PaymentRequestError;
use fandhe_core::screens::payment_request::{
    CoreOutcome, PaymentAction, PaymentRequestState, finish, mark_running, take_request,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた `PaymentSheet` 実装を返す。
///
/// Android は Google Pay API がコールバック型のため `PaymentSheet` を実装せず、iOS・macOS は対応表で保留、
/// Windows は非対応のため、どの OS でもこの trait 経由は `Unsupported` 固定を返す。
/// 実際の確認・決済シートは [`start_payment_check`] / [`start_payment`] と [`poll_payment`] の組で行う。Web の実装は `fandhe-web-dom` にあり、この契約の定数と写像だけを共有する。
/// 画面の状態型に依存しない入口（`docs/architecture/consumer-app-structure.md` §8.2）。
pub fn platform_payment_request() -> std::sync::Arc<dyn PaymentSheet> {
    register_teardown();
    std::sync::Arc::new(UnsupportedPaymentSheet)
}

/// 公開入口 [`poll_payment`] が返す結果（画面の状態型に依存しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentPoll {
    /// 支払いに使える。
    Available,
    /// テスト環境で承認された（決済の成立ではない）。
    TestAuthorized,
    /// キャンセル・中断。
    Cancelled,
    /// この環境では非対応（Google Pay が使えない Android・Android 以外）。
    Unsupported,
}

/// フレームワークの外のアプリ用: 「使えるか」の確認を OS へ依頼し、要求 id を返す。
/// 進行中、または結果が未取得の要求があれば `None`（置き換えない）。同期で失敗した場合も id を返し、失敗は [`poll_payment`] で受け取る。
/// 画面用の effect（[`payment_request_effect`]）と同じ slot を使うが、持ち主（`SlotOwner`）で分け、互いの要求を置き換えも横取りもしない（先行があれば後からの側が失敗または `None`）。
pub fn start_payment_check() -> Option<u64> {
    register_teardown();
    start(PaymentAction::Check)
}

/// フレームワークの外のアプリ用: 決済シートの表示を OS へ依頼し、要求 id を返す。規則は [`start_payment_check`] と同じ。
pub fn start_payment() -> Option<u64> {
    register_teardown();
    start(PaymentAction::Pay)
}

fn start(action: PaymentAction) -> Option<u64> {
    let request_id = bridge::SLOT.try_begin_unless(|_| true)?;
    #[cfg(target_os = "android")]
    let started = crate::android::begin(request_id, matches!(action, PaymentAction::Pay));
    #[cfg(not(target_os = "android"))]
    let started: Result<(), PaymentRequestError> = Err(PaymentRequestError::Unsupported);
    if let Err(e) = started {
        let outcome = match action {
            PaymentAction::Check => check_outcome(Err(e)),
            PaymentAction::Pay => pay_outcome(Err(e)),
        };
        bridge::SLOT.deliver(request_id, outcome);
    }
    Some(request_id)
}

/// [`start_payment_check`] / [`start_payment`] の結果を取り出す。未着・別の id・取り出し済みは `None`。
/// 取り出すと要求は終わり、次の `start_*` を受け付ける。失敗は固定文言（内部名を出さない）。
pub fn poll_payment(request_id: u64) -> Option<Result<PaymentPoll, String>> {
    register_teardown();
    let outcome = bridge::SLOT.take_ready(request_id)?;
    Some(outcome.map(|o| match o {
        CoreOutcome::Available => PaymentPoll::Available,
        CoreOutcome::TestAuthorized => PaymentPoll::TestAuthorized,
        CoreOutcome::Cancelled => PaymentPoll::Cancelled,
        CoreOutcome::Unsupported => PaymentPoll::Unsupported,
    }))
}

/// 内部名を出さない固定文言（security.md A09）。
const FAILED_TEXT: &str = "決済シートを完了できませんでした";

fn check_outcome(result: Result<bool, PaymentRequestError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(true) => Ok(CoreOutcome::Available),
        Ok(false) | Err(PaymentRequestError::Unsupported) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

fn pay_outcome(result: Result<PaymentOutcome, PaymentRequestError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(PaymentOutcome::TestAuthorized) => Ok(CoreOutcome::TestAuthorized),
        Ok(PaymentOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(PaymentRequestError::Unsupported) => Ok(CoreOutcome::Unsupported),
        Ok(PaymentOutcome::Failed) | Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

/// OS 側コールバックとの橋渡し（Android、pull 型、`RequestSlot` の facade）。
/// `deliver_*` は `android_payment_request` の JNI 受け口が呼ぶ。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) mod bridge {
    use super::{CoreOutcome, PaymentOutcome, check_outcome, pay_outcome};
    use crate::error::PaymentRequestError;
    use fandhe_runtime::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 「使えるか」の結果を置く。不一致・未登録・既に結果ありは捨てて `false`。
    pub(crate) fn deliver_check(
        request_id: u64,
        result: Result<bool, PaymentRequestError>,
    ) -> bool {
        SLOT.deliver(request_id, check_outcome(result))
    }

    /// 「決済シート」の結果を置く。規則は [`deliver_check`] と同じ。
    pub(crate) fn deliver_pay(
        request_id: u64,
        result: Result<PaymentOutcome, PaymentRequestError>,
    ) -> bool {
        SLOT.deliver(request_id, pay_outcome(result))
    }

    /// Activity 再生成直後に登録表（`fandhe_runtime::teardown`）から呼ばれる。保留中の要求は復元せず破棄し、旧 `request_id`
    /// の完了を無視させる。
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// 登録表（`fandhe_runtime::teardown`）から呼ばれる薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// 画面側の要求の持ち主印（公開入口の要求と同じ [`bridge::SLOT`] を共有するため、自分の分だけを追う）。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// Android 版の effect ドライバ本体。`begin` が `Err` なら依頼自体の失敗としてその場で結果を置く。
/// 公開入口の要求が進行中・未取得なら置き換えず、画面へ失敗を返す。
/// `begin` へ渡す第 2 引数は決済シートなら `true`（画面の型は OS 実装へ渡さない）。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn drive_callback(
    state: &PaymentRequestState,
    sender: &CompletionSender<PaymentRequestState>,
    slot: &'static fandhe_runtime::request_slot::RequestSlot<bridge::Outcome>,
    owner: &'static fandhe_runtime::request_slot::SlotOwner,
    begin: fn(u64, bool) -> Result<(), PaymentRequestError>,
) {
    use fandhe_core::screens::payment_request::PaymentPhase;

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_running));
        let Some(request_id) = owner.begin_keeping_foreign(slot) else {
            let generation = view.generation;
            let _ = sender.send(Box::new(move |s: &mut PaymentRequestState| {
                finish(s, generation, Err(FAILED_TEXT.to_string()));
            }));
            return;
        };
        if let Err(e) = begin(request_id, matches!(view.action, PaymentAction::Pay)) {
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
    fandhe_runtime::request_slot::poll_and_finish_owned(
        slot,
        owner,
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
        // Activity 再生成・公開入口で自分の要求が失われた。処理中のまま止めない。
        move |s: &mut PaymentRequestState| {
            finish(s, generation, Err(FAILED_TEXT.to_string()));
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
    register_teardown();
    drive_callback(
        state,
        sender,
        &bridge::SLOT,
        &SCREEN_OWNER,
        crate::android::begin,
    );
}

/// `registry.rs` から配線される effect ドライバ（Android 以外。常に「使えない」を表示する）。
#[cfg(not(target_os = "android"))]
pub fn payment_request_effect(
    state: &PaymentRequestState,
    sender: &CompletionSender<PaymentRequestState>,
) {
    register_teardown();
    drive_sync(state, sender, platform_payment_request().as_ref());
}

/// 終了処理と Android の再生成時の破棄を登録表（`fandhe_runtime::teardown`）へ登録する（Issue #1453）。
/// 公開の入口・effect・公開型のメソッドの先頭で呼ぶ。初めて使われたときに登録され、2 回目以降は何もしない。
pub(crate) fn register_teardown() {
    #[cfg(target_os = "android")]
    fandhe_runtime::teardown::register(
        fandhe_runtime::teardown::Teardown::new("payment-request")
            .on_android_recreate(crate::reset_pending_tracking),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::payment_request as screen;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    #[test]
    fn outcomes_cover_all_arms() {
        assert_eq!(check_outcome(Ok(true)), Ok(CoreOutcome::Available));
        assert_eq!(check_outcome(Ok(false)), Ok(CoreOutcome::Unsupported));
        assert_eq!(
            check_outcome(Err(PaymentRequestError::Unsupported)),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            check_outcome(Err(PaymentRequestError::Internal("payment_request failed"))),
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
            pay_outcome(Err(PaymentRequestError::Unsupported)),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(!FAILED_TEXT.contains("payment"));
    }

    fn run(sheet: &dyn PaymentSheet, press: fn(&mut PaymentRequestState)) -> PaymentRequestState {
        let mut s = PaymentRequestState::default();
        press(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive_sync(&s, &sender, sheet);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        s
    }

    struct Stub(fn() -> Result<PaymentOutcome, PaymentRequestError>);
    impl PaymentSheet for Stub {
        fn can_make_payments(&self) -> Result<bool, PaymentRequestError> {
            (self.0)().map(|_| true)
        }
        fn request(&self) -> Result<PaymentOutcome, PaymentRequestError> {
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
        let stub = Stub(|| Err(PaymentRequestError::Internal("payment_request failed")));
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
        slot: &'static fandhe_runtime::request_slot::RequestSlot<bridge::Outcome>,
        owner: &'static fandhe_runtime::request_slot::SlotOwner,
        begin: fn(u64, bool) -> Result<(), PaymentRequestError>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive_callback(s, &sender, slot, owner, begin);
        while let Ok(apply) = rx.try_recv() {
            apply(s);
        }
    }

    fn accept(_: u64, _: bool) -> Result<(), PaymentRequestError> {
        Ok(())
    }

    #[test]
    fn host_begin_shows_unsupported_never_success() {
        use crate::android::begin;
        for press in [screen::check, screen::pay] {
            static SLOT: fandhe_runtime::request_slot::RequestSlot<bridge::Outcome> =
                fandhe_runtime::request_slot::RequestSlot::new();
            static OWNER: fandhe_runtime::request_slot::SlotOwner =
                fandhe_runtime::request_slot::SlotOwner::new();
            let mut s = PaymentRequestState::default();
            press(&mut s);
            frame(&mut s, &SLOT, &OWNER, begin);
            frame(&mut s, &SLOT, &OWNER, begin);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
            SLOT.reset();
        }
    }

    #[test]
    fn cancelled_via_bridge_is_not_error() {
        static SLOT: fandhe_runtime::request_slot::RequestSlot<bridge::Outcome> =
            fandhe_runtime::request_slot::RequestSlot::new();
        static OWNER: fandhe_runtime::request_slot::SlotOwner =
            fandhe_runtime::request_slot::SlotOwner::new();
        let mut s = PaymentRequestState::default();
        screen::pay(&mut s);
        frame(&mut s, &SLOT, &OWNER, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        assert!(SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::Cancelled))));
        frame(&mut s, &SLOT, &OWNER, accept);
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
                check_outcome(Err(PaymentRequestError::PermissionDenied)),
            ),
            (
                PaymentAction::Pay,
                pay_outcome(Err(PaymentRequestError::InvalidInput)),
            ),
            (
                PaymentAction::Check,
                check_outcome(Err(PaymentRequestError::Internal("payment_request failed"))),
            ),
        ];
        for (action, outcome) in cases {
            static SLOT: fandhe_runtime::request_slot::RequestSlot<bridge::Outcome> =
                fandhe_runtime::request_slot::RequestSlot::new();
            static OWNER: fandhe_runtime::request_slot::SlotOwner =
                fandhe_runtime::request_slot::SlotOwner::new();
            let mut s = PaymentRequestState::default();
            match action {
                PaymentAction::Check => screen::check(&mut s),
                PaymentAction::Pay => screen::pay(&mut s),
            }
            frame(&mut s, &SLOT, &OWNER, accept);
            let (id, _) = SLOT.snapshot().unwrap();
            assert!(SLOT.deliver(id, outcome));
            frame(&mut s, &SLOT, &OWNER, accept);
            assert!(s.status.is_empty());
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
            SLOT.reset();
        }
    }

    #[test]
    fn screen_does_not_replace_or_take_public_request() {
        static SLOT: fandhe_runtime::request_slot::RequestSlot<bridge::Outcome> =
            fandhe_runtime::request_slot::RequestSlot::new();
        static OWNER: fandhe_runtime::request_slot::SlotOwner =
            fandhe_runtime::request_slot::SlotOwner::new();
        let public = SLOT.try_begin().expect("空き");
        let mut s = PaymentRequestState::default();
        screen::pay(&mut s);
        frame(&mut s, &SLOT, &OWNER, accept);
        // 画面は失敗で確定し、公開入口の要求はそのまま残る。
        assert!(s.error.starts_with(screen::FAILED_PREFIX));
        assert_eq!(SLOT.snapshot().map(|(id, _)| id), Some(public));
        assert!(SLOT.deliver(public, pay_outcome(Ok(PaymentOutcome::Cancelled))));
        frame(&mut s, &SLOT, &OWNER, accept);
        assert_eq!(SLOT.take_ready(public), Some(Ok(CoreOutcome::Cancelled)));
    }

    #[test]
    fn stale_request_id_is_dropped() {
        static SLOT: fandhe_runtime::request_slot::RequestSlot<bridge::Outcome> =
            fandhe_runtime::request_slot::RequestSlot::new();
        static OWNER: fandhe_runtime::request_slot::SlotOwner =
            fandhe_runtime::request_slot::SlotOwner::new();
        let mut s = PaymentRequestState::default();
        screen::pay(&mut s);
        frame(&mut s, &SLOT, &OWNER, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        let stale = id.wrapping_add(1);
        assert!(!SLOT.deliver(stale, pay_outcome(Ok(PaymentOutcome::TestAuthorized))));
        frame(&mut s, &SLOT, &OWNER, accept);
        assert_eq!(s.phase, screen::PaymentPhase::Running);
        assert!(s.status.is_empty() && s.error.is_empty());
        // first-wins: 2 回目の deliver は捨てる
        assert!(SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::Cancelled))));
        assert!(!SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::TestAuthorized))));
        frame(&mut s, &SLOT, &OWNER, accept);
        assert_eq!(s.status, screen::CANCELLED_STATUS);
    }

    #[test]
    fn reset_drops_pending_and_old_completion() {
        static SLOT: fandhe_runtime::request_slot::RequestSlot<bridge::Outcome> =
            fandhe_runtime::request_slot::RequestSlot::new();
        static OWNER: fandhe_runtime::request_slot::SlotOwner =
            fandhe_runtime::request_slot::SlotOwner::new();
        let mut s = PaymentRequestState::default();
        screen::pay(&mut s);
        frame(&mut s, &SLOT, &OWNER, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        SLOT.reset();
        assert!(!SLOT.deliver(id, pay_outcome(Ok(PaymentOutcome::TestAuthorized))));
        let mut fresh = PaymentRequestState::default();
        frame(&mut fresh, &SLOT, &OWNER, accept);
        assert!(fresh.status.is_empty() && fresh.error.is_empty());
    }

    #[test]
    fn ready_false_is_unsupported_not_available() {
        static SLOT: fandhe_runtime::request_slot::RequestSlot<bridge::Outcome> =
            fandhe_runtime::request_slot::RequestSlot::new();
        static OWNER: fandhe_runtime::request_slot::SlotOwner =
            fandhe_runtime::request_slot::SlotOwner::new();
        let mut s = PaymentRequestState::default();
        screen::check(&mut s);
        frame(&mut s, &SLOT, &OWNER, accept);
        let (id, _) = SLOT.snapshot().unwrap();
        assert!(SLOT.deliver(id, check_outcome(Ok(false))));
        frame(&mut s, &SLOT, &OWNER, accept);
        assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn public_entry_reports_unsupported_and_blocks_until_polled() {
        let id = start_payment_check().expect("空きがあれば登録できる");
        assert!(start_payment().is_none(), "未取得の結果を置き換えない");
        assert_eq!(poll_payment(id.wrapping_add(1)), None);
        assert_eq!(poll_payment(id), Some(Ok(PaymentPoll::Unsupported)));
        assert_eq!(poll_payment(id), None);
        let id2 = start_payment().expect("取り出し後は受け付ける");
        assert_eq!(poll_payment(id2), Some(Ok(PaymentPoll::Unsupported)));
    }

    #[test]
    fn no_request_never_calls_os() {
        let s = PaymentRequestState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive_sync(&s, &sender, &Stub(|| Ok(PaymentOutcome::TestAuthorized)));
        assert!(rx.try_recv().is_err());
    }
}
