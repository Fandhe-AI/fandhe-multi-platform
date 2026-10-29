//! 型サンプル「スリープ・ロックの通知」の Web 経路（親 #167、Issue #1063）。
//!
//! `fandhe_core::screens::sleep_lock_events_show` の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。呼び出し元は `demo.rs` のエントリポイントと、破棄時の解放を担う
//! `mount::sweep_mounts`。`fandhe-device` の `SleepLockEvents` trait は経由しない（ブラウザの
//! Promise・イベントは同期のトレイトに載らない）。結果の種類は trait doc に従い、
//! `fandhe_device::sleep_lock_events` から使い回すのは `EventLog` と純関数だけ。
//!
//! # 方式
//!
//! Idle Detection API の `screenState`（`locked` / `unlocked`）を `change` ごとに読み直し、同じ購読で
//! 直前に読んだ値との違いだけをロック・解除とする（`change` は `userState` の変化でも届くため）。
//! 呼び出しの部品（`IdleDetector` の取得・例外名）は同じ API を使う [`super::user_idle`] と共有する。
//!
//! - 押下に続く再描画の同期区間で `IdleDetector.requestPermission()`
//! - 許可後に `new IdleDetector()`、`change` の購読、`start({ threshold, signal })`
//! - pagehide で止める（正常）。bfcache から戻っても自動で再開しない。`visibilitychange` では止めない
//!
//! スリープ・復帰・画面の消灯と点灯は Web に通知が無く「この OS では取れない」と出す。開始前は
//! `screenState` が null のためロック状態は「不明」。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 非対応・拒否・失敗・null や未知の値を「ロックされていない」と見せない、1 マウントで二重に
//! `start` しない、古い世代の完了を捨てる、読めない期間をまたぐ差を出来事にしない、開始途中の失敗・
//! 停止・pagehide・マウント破棄で abort・リスナー解除・`clearTimeout` を行う、`start()` に期限
//! （呼び出しを起点に 10 秒）を置く。
//! 守らない範囲は trait doc を参照（許可ダイアログの待ちの期限、Permissions Policy と利用者の拒否の
//! 区別、非表示の間の `change` の遅れと取りこぼし、スクリーンセーバーをロックとして数えること等）。
//!
//! ロック状態・例外の `message` は console にも DOM にも出さない（security.md A09）。

use fandhe_core::screens::sleep_lock_events_show::{CurrentView, ScreenView};
use fandhe_core::screens::sleep_lock_events_show::{
    EventKind, LockView, StartFailure, SupportView, ALL_KINDS,
};
use fandhe_device::sleep_lock_events::{LockState, SleepLockEvent};

/// `start()` の呼び出しを起点にした期限（ms）。許可ダイアログの待ちは含めない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const START_TIMEOUT_MS: i32 = 10_000;

/// effect が 1 回の再描画で取る動作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) enum Action {
    /// 世代で新規に開始する（既存の購読は先に畳む）。
    Start(u64),
    /// 何もしない（実行中。再描画ループを防ぐ）。
    Keep,
    /// 購読を畳む。
    Stop,
}

/// 使える環境（secure context かつ `IdleDetector` あり）から対応表示を作る。使えない環境は
/// 全部 false（画面の `apply_init` が非対応にする）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn support_from(available: bool) -> SupportView {
    let mut events = [false; 6];
    events[SleepLockEvent::Locked.index()] = available;
    events[SleepLockEvent::Unlocked.index()] = available;
    SupportView {
        events,
        lock: available,
        screen: false,
    }
}

/// 開始前の今の状態。`screenState` は開始前 null のため、使える環境のロックは「不明」。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn initial_current(available: bool) -> CurrentView {
    CurrentView {
        lock: if available {
            LockView::Unknown
        } else {
            LockView::NotAvailable
        },
        screen: ScreenView::NotAvailable,
    }
}

/// 読んだロック状態を表示用へ写す。`None`（null・未知の値）は読み取り失敗で、解除へ畳まない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn lock_view(v: Option<LockState>) -> LockView {
    match v {
        Some(LockState::Locked) => LockView::Locked,
        Some(LockState::Unlocked) => LockView::Unlocked,
        Some(LockState::Unknown) => LockView::Unknown,
        None => LockView::ReadFailed,
    }
}

/// `requestPermission()` の結果文字列を写す。granted 以外を成功に畳まない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn permission_outcome(v: Option<&str>) -> Result<(), StartFailure> {
    match v {
        Some("granted") => Ok(()),
        Some("denied") => Err(StartFailure::PermissionDenied),
        _ => Err(StartFailure::Failed),
    }
}

/// DOM 例外の `name` を契約上の失敗へ写す（全呼び出し箇所で本関数に集約する）。`message` は受け取らない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: &str) -> StartFailure {
    match name {
        "NotAllowedError" => StartFailure::PermissionDenied,
        _ => StartFailure::Failed,
    }
}

/// effect の判断。`receiving_gen` は `Receiving` のときだけ現在の世代。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decide(
    start_gen: Option<u64>,
    receiving_gen: Option<u64>,
    session: Option<u64>,
) -> Action {
    if let Some(gen) = start_gen {
        return if session == Some(gen) {
            Action::Keep
        } else {
            Action::Start(gen)
        };
    }
    match receiving_gen {
        Some(g) if session == Some(g) => Action::Keep,
        _ => Action::Stop,
    }
}

/// device の出来事を画面側の鏡像へ写す（`ALL_KINDS` と `ALL_EVENTS` は並びが同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn kind_from_device(e: SleepLockEvent) -> EventKind {
    ALL_KINDS[e.index()]
}

#[cfg(target_arch = "wasm32")]
pub use web::sleep_lock_events_show_effect;

/// マウント破棄時に購読・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::sleep_lock_events_show::{
        apply_init, apply_snapshot, finish_start, mark_left, take_init, take_start, CurrentView,
        Phase, ScreenView, SleepLockEventsShowState, StartFailure,
    };
    use fandhe_device::sleep_lock_events::{
        event_from_lock_transition, lock_state_from_idle_screen_state, EventLog, LockState,
    };
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{
        decide, failure_from_error_name, initial_current, kind_from_device, lock_view,
        permission_outcome, support_from, Action, START_TIMEOUT_MS,
    };
    use crate::compass::is_current;
    use crate::mount::{is_live, Applier};
    use crate::user_idle::{detector_ctor, error_name, get, threshold_ms};

    type State = SleepLockEventsShowState;

    struct Entry {
        /// 動作中の要求の世代。
        session: Option<u64>,
        detector: Option<JsValue>,
        controller: Option<web_sys::AbortController>,
        start_timer: Option<i32>,
        listening_change: bool,
        pagehide_registered: bool,
        init_sent: bool,
        log: EventLog,
        /// 同じ購読で最後に読んだ `screenState`。出来事の判定にだけ使う。
        prev: Option<LockState>,
        on_change: Closure<dyn FnMut()>,
        on_timeout: Closure<dyn FnMut()>,
        on_pagehide: Closure<dyn FnMut()>,
    }

    thread_local! {
        /// マウントごとの状態（wasm32 は単一スレッド）。
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// 既存の Entry にだけ作用する。破棄済みマウントでは `None`（再生成して漏らさない）。
    fn with<R>(id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        ENTRIES.with(|m| m.borrow_mut().get_mut(&id).map(f))
    }

    fn read_lock(detector: &JsValue) -> Option<LockState> {
        lock_state_from_idle_screen_state(get(detector, "screenState").as_string().as_deref())
    }

    fn clear_timer(handle: i32) {
        if let Some(w) = web_sys::window() {
            w.clear_timeout_with_handle(handle);
        }
    }

    /// abort・`change` 解除・`clearTimeout` を行う（冪等）。closure は drop しない。
    fn teardown_entry(e: &mut Entry) {
        if let Some(c) = e.controller.take() {
            c.abort();
        }
        if let Some(d) = e.detector.take() {
            if e.listening_change {
                if let Some(t) = d.dyn_ref::<web_sys::EventTarget>() {
                    let _ = t.remove_event_listener_with_callback(
                        "change",
                        e.on_change.as_ref().unchecked_ref(),
                    );
                }
            }
        }
        e.listening_change = false;
        if let Some(h) = e.start_timer.take() {
            clear_timer(h);
        }
        e.session = None;
        e.prev = None;
    }

    fn teardown(id: u64) {
        with(id, teardown_entry);
    }

    fn fail(applier: &Applier<State>, gen: u64, f: StartFailure) {
        applier.apply(move |s| finish_start(s, gen, Err(f)));
    }

    /// 記録と今のロック状態を画面へ渡す。借用を落としてから `apply` する。
    fn push_snapshot(applier: &Applier<State>, id: u64, gen: u64, lock: Option<LockState>) {
        let Some((recent, counts)) = with(id, |e| e.log.snapshot()) else {
            return;
        };
        let recent: Vec<_> = recent
            .into_iter()
            .map(|(n, e)| (n, kind_from_device(e)))
            .collect();
        let current = CurrentView {
            lock: lock_view(lock),
            screen: ScreenView::NotAvailable,
        };
        applier.apply(move |s| apply_snapshot(s, gen, recent, counts, Some(current)));
    }

    fn new_entry(applier: &Applier<State>, id: u64) -> Entry {
        let on_change = {
            let applier = applier.clone();
            Closure::<dyn FnMut()>::new(move || {
                let Some(Some((gen, detector, resolved))) = with(id, |e| {
                    let d = e.detector.clone()?;
                    Some((e.session?, d, e.start_timer.is_none()))
                }) else {
                    return;
                };
                let now = read_lock(&detector);
                if !resolved {
                    // 開始の解決前は基準を置くだけで、出来事にも表示にもしない。
                    with(id, |e| e.prev = now);
                    return;
                }
                with(id, |e| {
                    if let Some(ev) = now.and_then(|n| event_from_lock_transition(e.prev, n)) {
                        e.log.record(ev);
                    }
                    // None（読めない）は基準を捨てる。次に読めた値は基準になるだけ。
                    e.prev = now;
                });
                push_snapshot(&applier, id, gen, now);
            })
        };
        let on_timeout = {
            let applier = applier.clone();
            Closure::<dyn FnMut()>::new(move || {
                // 発火済みのハンドルは clear しない。未確定（タイマーが残っている）ときだけ失敗にする。
                let gen = with(id, |e| {
                    let pending = e.start_timer.take().is_some();
                    pending.then_some(e.session).flatten()
                })
                .flatten();
                let Some(gen) = gen else {
                    return;
                };
                // 状態の確定（abort・解除）を結果の通知より先に行う（契約表 2・3）。
                teardown(id);
                fail(&applier, gen, StartFailure::Failed);
            })
        };
        let on_pagehide = {
            let applier = applier.clone();
            Closure::<dyn FnMut()>::new(move || {
                let Some(Some(gen)) = with(id, |e| e.session) else {
                    return;
                };
                teardown(id);
                applier.apply(move |s| mark_left(s, gen));
            })
        };
        Entry {
            session: None,
            detector: None,
            controller: None,
            start_timer: None,
            listening_change: false,
            pagehide_registered: false,
            init_sent: false,
            log: EventLog::new(),
            prev: None,
            on_change,
            on_timeout,
            on_pagehide,
        }
    }

    /// window の pagehide を 1 マウントにつき 1 度だけ登録する。
    fn install_lifecycle(id: u64) -> bool {
        let Some(w) = web_sys::window() else {
            return false;
        };
        with(id, |e| {
            if e.pagehide_registered {
                return true;
            }
            if w.add_event_listener_with_callback(
                "pagehide",
                e.on_pagehide.as_ref().unchecked_ref(),
            )
            .is_err()
            {
                return false;
            }
            e.pagehide_registered = true;
            true
        })
        .unwrap_or(false)
    }

    fn current(id: u64, gen: u64) -> bool {
        with(id, |e| is_current(is_live(id), e.session, gen)).unwrap_or(false)
    }

    /// 購読を開始する（同期部）。`start()` の Promise を待つ非同期部は `spawn_local` に渡す。
    fn begin_start(applier: &Applier<State>, id: u64, gen: u64) {
        let Some(ctor) = detector_ctor() else {
            return fail(applier, gen, StartFailure::Unsupported);
        };
        let detector = match Reflect::construct(&ctor, &Array::new()) {
            Ok(d) => d,
            Err(e) => return fail(applier, gen, failure_from_error_name(&error_name(&e))),
        };
        let Ok(controller) = web_sys::AbortController::new() else {
            return fail(applier, gen, StartFailure::Failed);
        };
        // `change` は `start()` より前に登録する。以降の確保は Entry に格納し、失敗時は teardown で解放する。
        let registered = with(id, |e| {
            if e.session != Some(gen) {
                return false;
            }
            let Some(t) = detector.dyn_ref::<web_sys::EventTarget>() else {
                return false;
            };
            if t.add_event_listener_with_callback("change", e.on_change.as_ref().unchecked_ref())
                .is_err()
            {
                return false;
            }
            e.listening_change = true;
            e.detector = Some(detector.clone());
            e.controller = Some(controller.clone());
            true
        })
        .unwrap_or(false);
        if !registered {
            // 世代が進んでいた場合は新しい要求の状態を壊さないよう、何も確定しない。
            if current(id, gen) {
                teardown(id);
                fail(applier, gen, StartFailure::Failed);
            }
            return;
        }
        let opts = Object::new();
        let _ = Reflect::set(
            &opts,
            &JsValue::from_str("threshold"),
            &JsValue::from_f64(threshold_ms()),
        );
        let _ = Reflect::set(
            &opts,
            &JsValue::from_str("signal"),
            &controller.signal().into(),
        );
        let promise = match get(&detector, "start").dyn_into::<Function>() {
            Ok(f) => match f.call1(&detector, &opts) {
                Ok(p) => p.dyn_into::<Promise>().ok(),
                Err(e) => {
                    teardown(id);
                    return fail(applier, gen, failure_from_error_name(&error_name(&e)));
                }
            },
            Err(_) => None,
        };
        let Some(promise) = promise else {
            teardown(id);
            return fail(applier, gen, StartFailure::Failed);
        };
        let timer = web_sys::window().and_then(|w| {
            with(id, |e| {
                w.set_timeout_with_callback_and_timeout_and_arguments_0(
                    e.on_timeout.as_ref().unchecked_ref(),
                    START_TIMEOUT_MS,
                )
                .ok()
            })
            .flatten()
        });
        match timer {
            Some(h) => {
                with(id, |e| e.start_timer = Some(h));
            }
            None => {
                teardown(id);
                return fail(applier, gen, StartFailure::Failed);
            }
        }
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise).await;
            // 再開後に要求がまだ現在のものか確かめる（abort 由来の AbortError もここで捨てる）。
            if !current(id, gen) {
                return;
            }
            match result {
                Ok(_) => {
                    let (detector, timer) =
                        with(id, |e| (e.detector.clone(), e.start_timer.take()))
                            .unwrap_or((None, None));
                    if let Some(h) = timer {
                        clear_timer(h);
                    }
                    let Some(now) = detector.as_ref().and_then(read_lock) else {
                        teardown(id);
                        return fail(&applier, gen, StartFailure::Failed);
                    };
                    with(id, |e| e.prev = Some(now));
                    let current = CurrentView {
                        lock: lock_view(Some(now)),
                        screen: ScreenView::NotAvailable,
                    };
                    applier.apply(move |s| {
                        finish_start(s, gen, Ok(()));
                        apply_snapshot(s, gen, Vec::new(), [0; 6], Some(current));
                    });
                }
                Err(e) => {
                    let failure = failure_from_error_name(&error_name(&e));
                    teardown(id);
                    fail(&applier, gen, failure);
                }
            }
        });
    }

    /// `requestPermission()` を同期区間で呼び、許可後に開始する。
    fn request_permission_then_start(applier: &Applier<State>, id: u64, gen: u64) {
        let Some(ctor) = detector_ctor() else {
            return fail(applier, gen, StartFailure::Unsupported);
        };
        let Ok(f) = get(&ctor, "requestPermission").dyn_into::<Function>() else {
            return fail(applier, gen, StartFailure::Unsupported);
        };
        let promise = match f.call0(&ctor) {
            Ok(p) => p.dyn_into::<Promise>().ok(),
            Err(e) => return fail(applier, gen, failure_from_error_name(&error_name(&e))),
        };
        let Some(promise) = promise else {
            return fail(applier, gen, StartFailure::Failed);
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise).await;
            if !current(id, gen) {
                return;
            }
            let outcome = match result {
                Ok(v) => permission_outcome(v.as_string().as_deref()),
                Err(e) => Err(failure_from_error_name(&error_name(&e))),
            };
            match outcome {
                Ok(()) => begin_start(&applier, id, gen),
                Err(f) => {
                    teardown(id);
                    fail(&applier, gen, f);
                }
            }
        });
    }

    /// `mount_with_effect` へ渡す effect。実行中は `Keep` で何もしない（再描画ループ防止）。
    pub fn sleep_lock_events_show_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        let session = ENTRIES.with(|m| {
            m.borrow_mut()
                .entry(id)
                .or_insert_with(|| new_entry(applier, id))
                .session
        });
        // 1. 初回: 対応状況と開始前の状態。
        if take_init(state)
            && with(id, |e| !std::mem::replace(&mut e.init_sent, true)) == Some(true)
        {
            let available = detector_ctor().is_some();
            let (support, current) = (support_from(available), initial_current(available));
            applier.apply(move |s| apply_init(s, support, current));
        }
        // 2. 開始 / 継続 / 停止。
        let receiving = (state.phase == Phase::Receiving).then_some(state.generation);
        match decide(take_start(state), receiving, session) {
            Action::Keep => {}
            Action::Stop => teardown(id),
            Action::Start(gen) => {
                with(id, |e| {
                    teardown_entry(e);
                    e.session = Some(gen);
                    e.log = EventLog::new();
                });
                if !install_lifecycle(id) {
                    teardown(id);
                    return fail(applier, gen, StartFailure::Failed);
                }
                request_permission_then_start(applier, id, gen);
            }
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(mut entry) = ENTRIES.with(|m| m.borrow_mut().remove(&mount_id)) else {
            return;
        };
        teardown_entry(&mut entry);
        if entry.pagehide_registered {
            if let Some(w) = web_sys::window() {
                let _ = w.remove_event_listener_with_callback(
                    "pagehide",
                    entry.on_pagehide.as_ref().unchecked_ref(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_only_when_available() {
        let s = support_from(true);
        assert_eq!(s.events, [false, false, false, false, true, true]);
        assert!(s.lock && !s.screen);
        let n = support_from(false);
        assert!(n.events.iter().all(|e| !e) && !n.lock && !n.screen);
    }

    #[test]
    fn initial_current_is_unknown_not_unlocked() {
        let c = initial_current(true);
        assert_eq!(
            (c.lock, c.screen),
            (LockView::Unknown, ScreenView::NotAvailable)
        );
        let c = initial_current(false);
        assert_eq!(c.lock, LockView::NotAvailable);
    }

    #[test]
    fn unreadable_lock_is_read_failed() {
        assert_eq!(lock_view(None), LockView::ReadFailed);
        assert_eq!(lock_view(Some(LockState::Locked)), LockView::Locked);
        assert_eq!(lock_view(Some(LockState::Unlocked)), LockView::Unlocked);
    }

    #[test]
    fn permission_outcome_never_fakes_success() {
        assert_eq!(permission_outcome(Some("granted")), Ok(()));
        assert_eq!(
            permission_outcome(Some("denied")),
            Err(StartFailure::PermissionDenied)
        );
        for v in [None, Some("prompt"), Some("")] {
            assert_eq!(permission_outcome(v), Err(StartFailure::Failed));
        }
    }

    #[test]
    fn error_name_mapping() {
        assert_eq!(
            failure_from_error_name("NotAllowedError"),
            StartFailure::PermissionDenied
        );
        for n in ["TypeError", "AbortError", "SecurityError", ""] {
            assert_eq!(failure_from_error_name(n), StartFailure::Failed);
        }
    }

    #[test]
    fn decide_starts_once_per_generation() {
        assert_eq!(decide(Some(1), None, None), Action::Start(1));
        assert_eq!(decide(Some(1), None, Some(1)), Action::Keep);
        assert_eq!(decide(Some(2), None, Some(1)), Action::Start(2));
    }

    #[test]
    fn decide_stops_when_not_current() {
        assert_eq!(decide(None, Some(4), Some(4)), Action::Keep);
        assert_eq!(decide(None, Some(5), Some(4)), Action::Stop);
        assert_eq!(decide(None, Some(5), None), Action::Stop);
        assert_eq!(decide(None, None, Some(4)), Action::Stop);
        assert_eq!(decide(None, None, None), Action::Stop);
    }

    #[test]
    fn kinds_align_with_device_events() {
        for (i, e) in fandhe_device::sleep_lock_events::ALL_EVENTS
            .iter()
            .enumerate()
        {
            assert_eq!(kind_from_device(*e).index(), i);
        }
    }
}
