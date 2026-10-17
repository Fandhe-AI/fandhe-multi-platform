//! Phase 11（親 #92）型サンプル「パスキー・サインイン連携」（Issue #342）の effect ドライバ。
//! `fandhe_core::screens::passkey_signin`（端末依存ゼロ）の要求を観測し、`fandhe_device::passkey`
//! の [`Passkey`] を呼んで結果を `CompletionSender` 経由で返す
//! （`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `fandhe_device::passkey` のモジュール doc を参照。
//!
//! - Windows: OS の UI はブロッキングのため、ワーカースレッドで [`Passkey::run`] を呼び、結果を
//!   pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム反映する。実装は
//!   `windows_passkey::implementation()`（#343 が埋める。**dispatch はここで最終形にしてあり、
//!   後続 issue は OS 別モジュールだけを埋める**）
//! - macOS / Linux / iOS / Android: 対応表で保留または非対応のため、同期で `UnsupportedPasskey`
//!   を呼び「使えない」を表示する。Web は `fandhe-web-dom`（#344）

use fandhe_core::screens::passkey_signin::{
    take_request, CoreOutcome, CoreSummary, CredentialMemo, PasskeyAction, PasskeySigninState,
};
use fandhe_device::passkey::{PasskeyOp, PasskeyOutcome, PasskeySummary};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

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

/// `DeviceError` を画面用の結果へ写像する。内部構造・入力値は出さず固定文言だけにする
/// （security.md A09）。応答不整合は成功にせず失敗として見せる。
fn to_outcome(result: Result<PasskeyOutcome, DeviceError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(PasskeyOutcome::Registered(s)) => Ok(CoreOutcome::Registered(to_summary(s))),
        Ok(PasskeyOutcome::AssertionReturned(s)) => {
            Ok(CoreOutcome::AssertionReturned(to_summary(s)))
        }
        Ok(PasskeyOutcome::NotAllowed) => Ok(CoreOutcome::NotAllowed),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::InvalidInput(_)) => {
            Err(fandhe_core::screens::passkey_signin::MISMATCH_TEXT.to_string())
        }
        Err(_) => Err("パスキーの操作を完了できませんでした".to_string()),
    }
}

/// ワーカースレッドと UI スレッドの橋渡し（Windows、pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) mod bridge {
    use fandhe_core::screens::passkey_signin::CoreOutcome;
    use fandhe_device::passkey::PasskeyOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` をワーカーへ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// ワーカーからの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(request_id: u64, result: Result<PasskeyOutcome, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }
}

/// Windows 版の effect ドライバ。要求を観測したらワーカーで OS の UI を出し、保留中は毎フレーム結果を拾う。
#[cfg(target_os = "windows")]
pub fn passkey_signin_effect(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
) {
    use fandhe_core::screens::passkey_signin::{finish, mark_running, PasskeyPhase};

    if let Some(view) = take_request(state) {
        // `mark_running` の反映前に次フレームが同じ要求を観測しても、二重に OS の UI を出さない。
        if bridge::SLOT.snapshot().is_some() {
            return;
        }
        let _ = sender.send(Box::new(mark_running));
        let request_id = bridge::begin();
        let op = op_of(view.action);
        // HWND の取得などは UI スレッドで済ませ、ブロッキングの呼び出しだけをワーカーへ渡す。
        let passkey = super::windows_passkey::implementation();
        let spawned = std::thread::Builder::new()
            .name("passkey".into())
            .spawn(move || {
                bridge::deliver(request_id, passkey.run(op));
            });
        if spawned.is_err() {
            bridge::deliver(request_id, Err(DeviceError::Internal("passkey failed")));
        }
        return;
    }

    if state.phase != PasskeyPhase::Running {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
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

/// 同期経路（macOS・Linux・iOS・Android）: [`Passkey`] を呼んで結果を反映する。
#[cfg(not(target_os = "windows"))]
fn drive_sync(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
    passkey: &dyn fandhe_device::passkey::Passkey,
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

/// `registry.rs` から配線される effect ドライバ（Windows 以外）。保留・非対応の OS では
/// 常に「使えない」を表示する。
#[cfg(not(target_os = "windows"))]
pub fn passkey_signin_effect(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
) {
    drive_sync(state, sender, &fandhe_device::passkey::UnsupportedPasskey);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::passkey::CredentialId;

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
            to_outcome(Err(DeviceError::Unsupported("passkey"))),
            Ok(CoreOutcome::Unsupported)
        ));
        for kind in ["passkey client data", "passkey flags", "passkey credential"] {
            assert_eq!(
                to_outcome(Err(DeviceError::InvalidInput(kind))),
                Err(fandhe_core::screens::passkey_signin::MISMATCH_TEXT.to_string())
            );
        }
        let failed = to_outcome(Err(DeviceError::Internal("passkey failed"))).unwrap_err();
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

    #[cfg(not(target_os = "windows"))]
    mod sync_path {
        use super::*;
        use fandhe_core::screens::passkey_signin as screen;
        use fandhe_device::passkey::{Passkey, UnsupportedPasskey};

        fn run(p: &dyn Passkey, press: fn(&mut PasskeySigninState)) -> PasskeySigninState {
            let mut s = PasskeySigninState::default();
            press(&mut s);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive_sync(&s, &sender, p);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        struct Fail;
        impl Passkey for Fail {
            fn run(&self, _op: PasskeyOp) -> Result<PasskeyOutcome, DeviceError> {
                Err(DeviceError::Internal("passkey failed"))
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
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive_sync(&s, &sender, &Fail);
            assert!(rx.try_recv().is_err());
        }
    }
}
