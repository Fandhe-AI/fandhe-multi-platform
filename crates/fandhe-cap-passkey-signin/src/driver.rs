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

/// OS 別の [`Passkey`] 実装を返す（画面の状態型に依存しない入口、`docs/architecture/consumer-app-structure.md` §8.2）。
/// Android はコールバック型のため、Credential Manager の完了まで `run` を block する（期限は置かない）。
/// 同時に 1 件だけで、先行が終わるまで後から呼ぶと `Internal` で即座に戻る。UI スレッドから呼ばない。
/// Windows 以外の非対応 OS は常に `Unsupported`。
pub fn platform_passkey() -> Box<dyn crate::contract::Passkey> {
    #[cfg(target_os = "windows")]
    return crate::windows::implementation();
    #[cfg(target_os = "android")]
    return Box::new(CallbackPasskey);
    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    return Box::new(crate::contract::UnsupportedPasskey);
}

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

    use fandhe_runtime::request_slot::RequestSlot;

    /// OS の生の結果。画面用への写像は取り出す側が行う
    /// （画面経由でも公開入口 `poll_passkey` でも同じ slot を使う）。
    pub(crate) type Outcome = Result<PasskeyOutcome, PasskeySigninError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` をワーカーへ渡す（Windows 用。Android は `SLOT` を直接使う）。
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    type Waiter = std::sync::mpsc::Sender<Outcome>;

    /// 画面を経ない呼び出し（`platform_passkey`、Android）の待ち合わせ。画面の [`SLOT`] とは別に持ち、
    /// 互いの保留を上書きしない（`fandhe-cap-calendar` と同形）。ID は [`SLOT`] の払い出しと衝突しないよう上位ビットを立てる。
    static WAITERS: std::sync::Mutex<Vec<(u64, Waiter)>> = std::sync::Mutex::new(Vec::new());
    static NEXT_EXTERNAL_ID: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(1 << 62);

    /// 画面を経ない呼び出しの待ちを登録して `(request_id, 受信側)` を返す。先行（画面の進行中の保留・別の待ち）が
    /// あれば `Internal` で拒否する。判定と登録は `WAITERS` のロックの内側で行い、[`begin_screen`] と直列化する。
    /// 結果は受信側のチャネルに入るため、[`SLOT`] の未取得の結果を置き換えない。
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn begin_external()
    -> Result<(u64, std::sync::mpsc::Receiver<Outcome>), PasskeySigninError> {
        let mut w = WAITERS.lock().unwrap_or_else(|e| e.into_inner());
        if !w.is_empty() || SLOT.snapshot().is_some_and(|(_, r)| r.is_none()) {
            return Err(PasskeySigninError::Internal);
        }
        let id = NEXT_EXTERNAL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        w.push((id, tx));
        Ok((id, rx))
    }

    /// 登録済みの待ちを外す（依頼自体が同期的に失敗したとき）。
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn cancel_external(request_id: u64) {
        WAITERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| *id != request_id);
    }

    /// 画面の依頼を登録する。画面を経ない待ちが残っているとき、または `slot` に進行中の保留があるときは
    /// `None`（置き換えない）。
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn begin_screen(
        slot: &RequestSlot<Outcome>,
        owner: &fandhe_runtime::request_slot::SlotOwner,
    ) -> Option<u64> {
        let w = WAITERS.lock().unwrap_or_else(|e| e.into_inner());
        if !w.is_empty() {
            return None;
        }
        owner.begin(slot)
    }

    /// OS・ワーカーからの結果を置く。画面を経ない待ちの id ならその待ちへ、そうでなければ [`SLOT`] へ。
    /// `request_id` 不一致・未登録・既に結果ありは捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<PasskeyOutcome, PasskeySigninError>,
    ) -> bool {
        let waiter = {
            let mut w = WAITERS.lock().unwrap_or_else(|e| e.into_inner());
            w.iter()
                .position(|(id, _)| *id == request_id)
                .map(|i| w.remove(i).1)
        };
        if let Some(tx) = waiter {
            return tx.send(result).is_ok();
        }
        SLOT.deliver(request_id, result)
    }

    /// Activity 再生成直後に呼ぶ。保留中の要求は復元せず破棄し、旧 `request_id` の完了を無視させる。
    #[cfg(any(target_os = "android", test))]
    pub(crate) fn reset() {
        SLOT.reset();
        // 待っている呼び出しは送信側を捨てて起こす（受信側は `Internal` で戻る）。
        WAITERS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Android 版の [`Passkey`]（[`platform_passkey`]）。Credential Manager の完了まで `run` を block する
/// （期限は置かない。OS の UI の応答待ちを推測の期限で失敗にしない）。RP ID 未設定・不正は `Unsupported`（OS を呼ばない）。
/// 同時に 1 件だけで、先行（画面の保留・別の待ち）が終わるまで後から呼ぶと `Internal` で即座に戻る。
/// Activity 再生成で待っている呼び出しは `Internal` で戻る。
#[cfg(target_os = "android")]
struct CallbackPasskey;

#[cfg(target_os = "android")]
impl crate::contract::Passkey for CallbackPasskey {
    fn run(&self, op: PasskeyOp) -> Result<PasskeyOutcome, PasskeySigninError> {
        let (request_id, rx) = bridge::begin_external()?;
        if let Err(e) = crate::android::begin(request_id, op) {
            bridge::cancel_external(request_id);
            return Err(e);
        }
        rx.recv().unwrap_or(Err(PasskeySigninError::Internal))
    }
}

/// `android_main` から呼ぶ。slot と `android_passkey` の保留を捨てる（Java 側は `onDestroy` で追跡を外し済み）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
    crate::android::reset();
}

/// 画面用 effect が追う自分の分の要求（公開入口 [`platform_passkey`] は別の待ち口を使う）。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// 公開入口などの先行が進行中で、画面の依頼を受けられないときの表示。
#[cfg(any(target_os = "android", test))]
const BUSY_MESSAGE: &str = "他のパスキー操作が処理中です。完了後にもう一度お試しください";

/// 依頼が失われた（再生成・別の依頼への置き換え）ときの表示。
#[cfg(any(target_os = "android", test))]
const LOST_MESSAGE: &str = "パスキー操作が中断されました。もう一度お試しください";

/// Android 版の effect ドライバ本体。`begin` が `Err` なら依頼自体の失敗としてその場で結果を置く
/// （RP ID 未設定・不正は `Unsupported`、OS を呼ばずに「使えない」になる）。先行（公開入口の待ち）が
/// 進行中なら置き換えず、画面には失敗として返す。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn drive_callback(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
    slot: &'static fandhe_runtime::request_slot::RequestSlot<bridge::Outcome>,
    owner: &'static fandhe_runtime::request_slot::SlotOwner,
    begin: fn(u64, PasskeyOp) -> Result<(), PasskeySigninError>,
) {
    use fandhe_core::screens::passkey_signin::{PasskeyPhase, finish, mark_running};

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_running));
        let generation = view.generation;
        let Some(request_id) = bridge::begin_screen(slot, owner) else {
            let _ = sender.send(Box::new(move |s: &mut PasskeySigninState| {
                finish(s, generation, Err(BUSY_MESSAGE.to_string()));
            }));
            return;
        };
        if let Err(e) = begin(request_id, op_of(view.action)) {
            slot.deliver(request_id, Err(e));
        }
        return;
    }
    if state.phase != PasskeyPhase::Running {
        return;
    }
    let generation = state.generation;
    fandhe_runtime::request_slot::poll_and_finish_owned(
        slot,
        owner,
        sender,
        |_request_id| {},
        move |s: &mut PasskeySigninState, raw| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != PasskeyPhase::Idle {
                finish(s, generation, to_outcome(raw));
                true
            } else {
                false
            }
        },
        move |s: &mut PasskeySigninState| {
            finish(s, generation, Err(LOST_MESSAGE.to_string()));
        },
    );
}

/// `registry.rs` から配線される effect ドライバ（Android: pull 型 bridge）。
#[cfg(target_os = "android")]
pub fn passkey_signin_effect(
    state: &PasskeySigninState,
    sender: &CompletionSender<PasskeySigninState>,
) {
    drive_callback(
        state,
        sender,
        &bridge::SLOT,
        &SCREEN_OWNER,
        crate::android::begin,
    );
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
        let passkey = platform_passkey();
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
        move |s: &mut PasskeySigninState, raw| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != PasskeyPhase::Idle {
                finish(s, generation, to_outcome(raw));
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
        assert_eq!(ready, Some(Ok(PasskeyOutcome::NotAllowed)));
        bridge::SLOT.clear(id);
    }

    mod callback_path {
        use super::*;
        use fandhe_core::screens::passkey_signin as screen;
        use fandhe_runtime::request_slot::{RequestSlot, SlotOwner};
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// 独立 slot で 1 フレーム進め、送られた更新を状態へ適用する。
        fn frame(
            s: &mut PasskeySigninState,
            slot: &'static RequestSlot<bridge::Outcome>,
            owner: &'static SlotOwner,
            begin: fn(u64, PasskeyOp) -> Result<(), PasskeySigninError>,
        ) {
            let _g = WAITERS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
            drive_callback(s, &sender, slot, owner, begin);
            while let Ok(apply) = rx.try_recv() {
                apply(s);
            }
        }

        /// 画面を経ない待ち合わせ（プロセス static）を触るテストと `frame` を直列化する。
        static WAITERS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
            static OWNER: SlotOwner = SlotOwner::new();
            let mut s = PasskeySigninState::default();
            screen::sign_in(&mut s);
            frame(&mut s, &SLOT, &OWNER, ok_begin);
            assert_eq!(s.phase, screen::PasskeyPhase::Running);
            let (id, _) = SLOT.snapshot().expect("begin 済み");
            // 保留中に再度観測しても begin は二度呼ばれない。
            let before = CALLS.load(Ordering::SeqCst);
            frame(&mut s, &SLOT, &OWNER, ok_begin);
            assert_eq!(CALLS.load(Ordering::SeqCst), before);
            assert!(SLOT.deliver(id, Ok(PasskeyOutcome::NotAllowed)));
            frame(&mut s, &SLOT, &OWNER, ok_begin);
            assert_ne!(s.phase, screen::PasskeyPhase::Running);
            assert!(SLOT.snapshot().is_none());
        }

        #[test]
        fn begin_error_shows_unsupported_or_failure() {
            static SLOT_U: RequestSlot<bridge::Outcome> = RequestSlot::new();
            static OWNER_U: SlotOwner = SlotOwner::new();
            let mut s = PasskeySigninState::default();
            screen::register(&mut s);
            frame(&mut s, &SLOT_U, &OWNER_U, unsupported_begin);
            frame(&mut s, &SLOT_U, &OWNER_U, unsupported_begin);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.registered.is_none());

            static SLOT_F: RequestSlot<bridge::Outcome> = RequestSlot::new();
            static OWNER_F: SlotOwner = SlotOwner::new();
            let mut s = PasskeySigninState::default();
            screen::sign_in(&mut s);
            frame(&mut s, &SLOT_F, &OWNER_F, failing_begin);
            frame(&mut s, &SLOT_F, &OWNER_F, failing_begin);
            assert!(s.status.is_empty() && s.error.starts_with(screen::FAILED_PREFIX));
        }

        #[test]
        fn public_request_is_not_replaced_and_not_taken_by_screen() {
            static SLOT: RequestSlot<bridge::Outcome> = RequestSlot::new();
            static OWNER: SlotOwner = SlotOwner::new();
            // 公開入口の待ちが進行中のとき、画面の要求は OS を呼ばず「処理中」の失敗になる。
            let public = SLOT.try_begin().expect("空き");
            let before = CALLS.load(Ordering::SeqCst);
            let mut s = PasskeySigninState::default();
            screen::sign_in(&mut s);
            frame(&mut s, &SLOT, &OWNER, ok_begin);
            assert_eq!(CALLS.load(Ordering::SeqCst), before);
            assert_eq!(s.phase, screen::PasskeyPhase::Idle);
            assert!(s.error.contains(BUSY_MESSAGE));
            // 公開側の id は置き換わらず、結果も画面に取られない。
            assert!(SLOT.deliver(public, Ok(PasskeyOutcome::NotAllowed)));
            frame(&mut s, &SLOT, &OWNER, ok_begin);
            let (id, ready) = SLOT.snapshot().expect("公開側の保留が残る");
            assert_eq!(id, public);
            assert!(ready.is_some());
        }

        #[test]
        fn external_wait_is_isolated_and_second_run_does_not_drop_first() {
            let _g = WAITERS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            bridge::reset();
            static SLOT: RequestSlot<bridge::Outcome> = RequestSlot::new();
            static OWNER: SlotOwner = SlotOwner::new();
            let (id1, rx1) = bridge::begin_external().expect("空き");
            // 待ちの間は画面の依頼も 2 つ目の待ちも拒否される。
            assert!(bridge::begin_screen(&SLOT, &OWNER).is_none());
            assert_eq!(
                bridge::begin_external().err(),
                Some(PasskeySigninError::Internal)
            );
            assert!(bridge::deliver(id1, Ok(PasskeyOutcome::NotAllowed)));
            // 2 回目が来ても 1 回目の成功は落ちない。
            let (_id2, _rx2) = bridge::begin_external().expect("1 回目の配送後は空き");
            assert_eq!(rx1.recv(), Ok(Ok(PasskeyOutcome::NotAllowed)));
            // 再生成で待っている呼び出しは起こされる。
            bridge::reset();
            let (_id3, rx3) = bridge::begin_external().expect("再生成後は空き");
            let t = std::thread::spawn(move || rx3.recv());
            std::thread::sleep(std::time::Duration::from_millis(30));
            bridge::reset();
            assert!(t.join().unwrap().is_err());
        }

        #[test]
        fn screen_request_fails_instead_of_hanging_when_slot_is_reset() {
            static SLOT: RequestSlot<bridge::Outcome> = RequestSlot::new();
            static OWNER: SlotOwner = SlotOwner::new();
            let mut s = PasskeySigninState::default();
            screen::sign_in(&mut s);
            frame(&mut s, &SLOT, &OWNER, ok_begin);
            assert_eq!(s.phase, screen::PasskeyPhase::Running);
            // Activity 再生成で slot が空になっても、画面は Running のまま止まらず失敗で確定する。
            SLOT.reset();
            frame(&mut s, &SLOT, &OWNER, ok_begin);
            assert_eq!(s.phase, screen::PasskeyPhase::Idle);
            assert!(s.error.contains(LOST_MESSAGE));
        }

        #[test]
        fn result_after_reset_is_dropped() {
            static SLOT: RequestSlot<bridge::Outcome> = RequestSlot::new();
            let id = SLOT.begin();
            SLOT.reset();
            assert!(!SLOT.deliver(id, Ok(PasskeyOutcome::NotAllowed)));
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
