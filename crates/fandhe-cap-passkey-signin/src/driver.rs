//! Phase 11（親 #92）型サンプル「パスキー・サインイン連携」（Issue #342）の effect ドライバ。
//! `fandhe_core::screens::passkey_signin`（端末依存ゼロ）の要求を観測し、`fandhe_cap_passkey_signin`
//! の [`Passkey`] を呼んで結果を `CompletionSender` 経由で返す
//! （`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `fandhe_cap_passkey_signin` のモジュール doc を参照。
//!
//! - Windows: OS の UI はブロッキングのため、ワーカースレッドで [`Passkey::run`] を呼び、結果を
//!   pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム反映する。実装は
//!   `windows_passkey::implementation()`（#343 で実装済み。**dispatch はここで最終形にしてあり、
//!   後続 issue は OS 別モジュールだけを埋める**）
//! - Android（#1280）: Credential Manager はコールバック型のため、`Passkey` trait は実装せず、
//!   `android_passkey::begin` が JNI で依頼して結果を同じ [`bridge`] へ [`drive_callback`] が毎フレーム反映する
//! - macOS / Linux / iOS: 対応表で保留または非対応のため、同期で `UnsupportedPasskey`
//!   を呼び「使えない」を表示する。Web は `fandhe-web-dom`（#344）

use crate::contract::{PasskeyOp, PasskeyOutcome, PasskeySummary};
use crate::error::PasskeySigninError;
use fandhe_core::screens::passkey_signin::{
    CoreOutcome, CoreSummary, CredentialMemo, PasskeyAction, PasskeySigninState, take_request,
};

use fandhe_runtime::CompletionSender;

fn op_of(action: PasskeyAction) -> PasskeyOp {
    match action {
        PasskeyAction::Register => PasskeyOp::Register,
        PasskeyAction::SignIn => PasskeyOp::SignIn,
    }
}

fn to_summary(s: PasskeySummary) -> CoreSummary {
    CoreSummary {
        rp_id: s.rp_id,
        credential_id: CredentialMemo(s.credential_id.as_bytes().to_vec()),
        user_present: s.user_present,
        user_verified: s.user_verified,
        signature_len: s.signature_len,
    }
}

/// `PasskeySigninError` を画面用の結果へ写像する。内部構造・入力値は出さず固定文言だけにする
/// （security.md A09）。応答不整合は成功にせず失敗として見せる。
fn to_outcome(result: Result<PasskeyOutcome, PasskeySigninError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(PasskeyOutcome::Registered(s)) => Ok(CoreOutcome::Registered(to_summary(s))),
        Ok(PasskeyOutcome::AssertionReturned(s)) => {
            Ok(CoreOutcome::AssertionReturned(to_summary(s)))
        }
        Ok(PasskeyOutcome::NotAllowed) => Ok(CoreOutcome::NotAllowed),
        Err(PasskeySigninError::Unsupported) => Ok(CoreOutcome::Unsupported),
        Err(PasskeySigninError::InvalidInput) => {
            Err(fandhe_core::screens::passkey_signin::MISMATCH_TEXT.to_string())
        }
        Err(_) => Err("パスキーの操作を完了できませんでした".to_string()),
    }
}

/// ワーカースレッド・OS コールバックと UI スレッドの橋渡し（Windows・Android、pull 型、
/// `RequestSlot` の facade）。他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "windows", target_os = "android", test))]
#[cfg_attr(
    not(any(target_os = "windows", target_os = "android")),
    allow(dead_code)
)]
pub(crate) mod bridge {
    use crate::contract::PasskeyOutcome;
    use crate::error::PasskeySigninError;
    use fandhe_core::screens::passkey_signin::CoreOutcome;

    use fandhe_runtime::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` をワーカーへ渡す（Windows 用。Android は `SLOT` を直接使う）。
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// ワーカーからの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<PasskeyOutcome, PasskeySigninError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Activity 再生成直後に呼ぶ。保留中の要求は復元せず破棄し、旧 `request_id` の完了を無視させる。
    #[cfg(target_os = "android")]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ。slot と `android_passkey` の保留を捨てる（Java 側は `onDestroy` で追跡を外し済み）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
    crate::android::reset();
}

/// Android 版の effect ドライバ本体。`begin` が `Err` なら依頼自体の失敗としてその場で結果を置く
/// （RP ID 未設定・不正は `Unsupported`、OS を呼ばずに「使えない」になる）。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn drive_callback(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
    slot: &'static fandhe_runtime::request_slot::RequestSlot<bridge::Outcome>,
    begin: fn(u64, PasskeyOp) -> Result<(), PasskeySigninError>,
) {
    use fandhe_core::screens::passkey_signin::{PasskeyPhase, finish, mark_running};

    if let Some(view) = take_request(state) {
        // `mark_running` の反映前に次フレームが同じ要求を観測しても、二重に OS の UI を出さない。
        if slot.snapshot().is_some() {
            return;
        }
        let _ = sender.send(Box::new(mark_running));
        let request_id = slot.begin();
        if let Err(e) = begin(request_id, op_of(view.action)) {
            slot.deliver(request_id, to_outcome(Err(e)));
        }
        return;
    }
    if state.phase != PasskeyPhase::Running {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish(
        slot,
        sender,
        |_request_id| {},
        move |s: &mut PasskeySigninState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != PasskeyPhase::Idle {
                finish(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から配線される effect ドライバ（Android: pull 型 bridge）。
#[cfg(target_os = "android")]
pub fn passkey_signin_effect(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
) {
    drive_callback(state, sender, &bridge::SLOT, crate::android::begin);
}

/// Windows 版の effect ドライバ。要求を観測したらワーカーで OS の UI を出し、保留中は毎フレーム結果を拾う。
#[cfg(target_os = "windows")]
pub fn passkey_signin_effect(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
) {
    use fandhe_core::screens::passkey_signin::{PasskeyPhase, finish, mark_running};

    if let Some(view) = take_request(state) {
        // `mark_running` の反映前に次フレームが同じ要求を観測しても、二重に OS の UI を出さない。
        if bridge::SLOT.snapshot().is_some() {
            return;
        }
        let _ = sender.send(Box::new(mark_running));
        let request_id = bridge::begin();
        let op = op_of(view.action);
        // HWND の取得などは UI スレッドで済ませ、ブロッキングの呼び出しだけをワーカーへ渡す。
        let passkey = crate::windows::implementation();
        let spawned = std::thread::Builder::new()
            .name("passkey".into())
            .spawn(move || {
                bridge::deliver(request_id, passkey.run(op));
            });
        if spawned.is_err() {
            bridge::deliver(request_id, Err(PasskeySigninError::Internal));
        }
        return;
    }

    if state.phase != PasskeyPhase::Running {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut PasskeySigninState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != PasskeyPhase::Idle {
                finish(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// 同期経路（macOS・Linux・iOS）: [`Passkey`] を呼んで結果を反映する。
#[cfg(any(not(any(target_os = "windows", target_os = "android")), test))]
#[cfg_attr(any(target_os = "windows", target_os = "android"), allow(dead_code))]
fn drive_sync(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
    passkey: &dyn crate::contract::Passkey,
) {
    use fandhe_core::screens::passkey_signin::{finish, mark_running};

    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    let outcome = to_outcome(passkey.run(op_of(view.action)));
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut PasskeySigninState| {
        finish(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ（Windows・Android 以外）。保留・非対応の OS では
/// 常に「使えない」を表示する。
#[cfg(not(any(target_os = "windows", target_os = "android")))]
pub fn passkey_signin_effect(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
) {
    drive_sync(state, sender, &crate::contract::UnsupportedPasskey);
}

#[cfg(test)]
mod tests {
    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    use super::*;
    use crate::contract::CredentialId;

    fn summary() -> PasskeySummary {
        PasskeySummary {
            rp_id: "localhost".into(),
            credential_id: CredentialId::from_bytes(vec![1, 2]),
            user_present: true,
            user_verified: true,
            signature_len: Some(64),
        }
    }

    #[test]
    fn to_outcome_covers_all_arms() {
        assert!(matches!(
            to_outcome(Ok(PasskeyOutcome::Registered(summary()))),
            Ok(CoreOutcome::Registered(s)) if s.credential_id.0 == vec![1, 2]
        ));
        assert!(matches!(
            to_outcome(Ok(PasskeyOutcome::AssertionReturned(summary()))),
            Ok(CoreOutcome::AssertionReturned(_))
        ));
        assert!(matches!(
            to_outcome(Ok(PasskeyOutcome::NotAllowed)),
            Ok(CoreOutcome::NotAllowed)
        ));
        assert!(matches!(
            to_outcome(Err(PasskeySigninError::Unsupported)),
            Ok(CoreOutcome::Unsupported)
        ));
        // 不整合（clientData・フラグ・資格情報のどれ）も同じ固定文言の失敗になる。
        assert_eq!(
            to_outcome(Err(PasskeySigninError::InvalidInput)),
            Err(fandhe_core::screens::passkey_signin::MISMATCH_TEXT.to_string())
        );
        let failed = to_outcome(Err(PasskeySigninError::Internal)).unwrap_err();
        assert!(!failed.contains("passkey"));
    }

    #[test]
    fn bridge_drops_stale_request_id_and_second_deliver() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id + 1000, Ok(PasskeyOutcome::NotAllowed)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, Ok(PasskeyOutcome::NotAllowed)));
        assert!(!bridge::deliver(
            id,
            Ok(PasskeyOutcome::Registered(summary()))
        ));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(CoreOutcome::NotAllowed)));
        bridge::SLOT.clear(id);
    }

    mod callback_path {
        use super::*;
        use fandhe_core::screens::passkey_signin as screen;
        use fandhe_runtime::request_slot::RequestSlot;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// 独立 slot で 1 フレーム進め、送られた更新を状態へ適用する。
        fn frame(
            s: &mut PasskeySigninState,
            slot: &'static RequestSlot<bridge::Outcome>,
            begin: fn(u64, PasskeyOp) -> Result<(), PasskeySigninError>,
        ) {
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive_callback(s, &sender, slot, begin);
            while let Ok(apply) = rx.try_recv() {
                apply(s);
            }
        }

        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn ok_begin(_id: u64, _op: PasskeyOp) -> Result<(), PasskeySigninError> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn unsupported_begin(_id: u64, _op: PasskeyOp) -> Result<(), PasskeySigninError> {
            Err(PasskeySigninError::Unsupported)
        }
        fn failing_begin(_id: u64, _op: PasskeyOp) -> Result<(), PasskeySigninError> {
            Err(PasskeySigninError::Internal)
        }

        #[test]
        fn accepted_request_runs_then_delivered_result_is_applied_once() {
            static SLOT: RequestSlot<bridge::Outcome> = RequestSlot::new();
            let mut s = PasskeySigninState::default();
            screen::sign_in(&mut s);
            frame(&mut s, &SLOT, ok_begin);
            assert_eq!(s.phase, screen::PasskeyPhase::Running);
            let (id, _) = SLOT.snapshot().expect("begin 済み");
            // 保留中に再度観測しても begin は二度呼ばれない。
            let before = CALLS.load(Ordering::SeqCst);
            frame(&mut s, &SLOT, ok_begin);
            assert_eq!(CALLS.load(Ordering::SeqCst), before);
            assert!(SLOT.deliver(id, to_outcome(Ok(PasskeyOutcome::NotAllowed))));
            frame(&mut s, &SLOT, ok_begin);
            assert_ne!(s.phase, screen::PasskeyPhase::Running);
            assert!(SLOT.snapshot().is_none());
        }

        #[test]
        fn begin_error_shows_unsupported_or_failure() {
            static SLOT_U: RequestSlot<bridge::Outcome> = RequestSlot::new();
            let mut s = PasskeySigninState::default();
            screen::register(&mut s);
            frame(&mut s, &SLOT_U, unsupported_begin);
            frame(&mut s, &SLOT_U, unsupported_begin);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.registered.is_none());

            static SLOT_F: RequestSlot<bridge::Outcome> = RequestSlot::new();
            let mut s = PasskeySigninState::default();
            screen::sign_in(&mut s);
            frame(&mut s, &SLOT_F, failing_begin);
            frame(&mut s, &SLOT_F, failing_begin);
            assert!(s.status.is_empty() && s.error.starts_with(screen::FAILED_PREFIX));
        }

        #[test]
        fn result_after_reset_is_dropped() {
            static SLOT: RequestSlot<bridge::Outcome> = RequestSlot::new();
            let id = SLOT.begin();
            SLOT.reset();
            assert!(!SLOT.deliver(id, to_outcome(Ok(PasskeyOutcome::NotAllowed))));
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    mod sync_path {
        use super::*;
        use crate::contract::{Passkey, UnsupportedPasskey};
        use fandhe_core::screens::passkey_signin as screen;

        fn run(p: &dyn Passkey, press: fn(&mut PasskeySigninState)) -> PasskeySigninState {
            let mut s = PasskeySigninState::default();
            press(&mut s);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive_sync(&s, &sender, p);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        struct Fail;
        impl Passkey for Fail {
            fn run(&self, _op: PasskeyOp) -> Result<PasskeyOutcome, PasskeySigninError> {
                Err(PasskeySigninError::Internal)
            }
        }

        #[test]
        fn unsupported_host_never_succeeds() {
            for press in [screen::register, screen::sign_in] {
                let s = run(&UnsupportedPasskey, press);
                assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
                assert!(s.error.is_empty() && s.detail.is_empty() && s.registered.is_none());
            }
        }

        #[test]
        fn failure_goes_to_error() {
            let s = run(&Fail, screen::sign_in);
            assert!(s.status.is_empty() && s.error.starts_with(screen::FAILED_PREFIX));
        }

        #[test]
        fn no_request_never_calls_os() {
            let s = PasskeySigninState::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive_sync(&s, &sender, &Fail);
            assert!(rx.try_recv().is_err());
        }
    }
}
