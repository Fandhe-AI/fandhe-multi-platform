//! 型サンプル「無操作時間の検知」の Web 経路（親 #125、Issue #632）。
//!
//! `fandhe_core::screens::user_idle_show` の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。`demo.rs` のエントリポイントと `mount::sweep_mounts`（破棄時の解放）から
//! 呼ばれる。`fandhe-device` の trait と `fandhe-egui-proxy` の registry は native 専用のため
//! 通らない（Web は本モジュールが `pending_request` を観測し結果を `finish_request` で返す）。
//!
//! # 方式
//!
//! Idle Detection API を `js_sys::Reflect` で呼ぶ（web-sys に stable feature が無い。
//! `screen_wake_lock` と同方針で、Cargo.toml の feature は増やさない）。
//!
//! - 押下に続く再描画の同期区間（transient activation の内）で `IdleDetector.requestPermission()`
//! - 許可後に `new IdleDetector()`、`change` の購読、`start({ threshold, signal })`
//! - bfcache 復帰では許可済みのため `start` だけをやり直す
//!
//! 経過秒は取れないので数えて代用しない（操作中 / 離席中と画面ロックだけを出す）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 非対応・拒否・失敗・一時停止・null や未知の状態を「操作中」と見せない、1 マウントで
//! 二重に `start` しない、古い世代の完了を捨てる、pagehide とマウント破棄で abort・リスナー解除・
//! `clearTimeout` を行う、`start()` に期限（呼び出しを起点に 10 秒）を置く。
//! 守らない: 許可ダイアログの応答待ちの期限、Permissions Policy による拒否と利用者の拒否の区別
//! （どちらも `PermissionDenied`）、iframe 埋め込み、複数タブ・複数マウント、Chromium 系以外
//! （非対応表示）、ブラウザ側の判定の遅れ。
//!
//! 在席状態・画面ロック・例外の `message` は console にも DOM にも出さない（security.md A09）。

use fandhe_core::screens::user_idle_show::{
    ReadFailure, ScreenLockView, UserPresenceView, WebIdleReading, WebRequestKind, IDLE_THRESHOLD,
};

/// `start()` の呼び出しを起点にした期限（ms）。許可ダイアログの待ちは含めない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const START_TIMEOUT_MS: i32 = 10_000;
/// 開始失敗の固定文言（core の `error_text` が「失敗しました: 」を前に付ける）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_START: &str = "離席検知の開始";
/// 開始の時間切れの固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_TIMEOUT: &str = "離席検知の開始（時間切れ）";
/// 状態が読めなかったときの固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_READ: &str = "在席状態の読み取り";

/// effect が 1 回の再描画で取る動作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) enum Action {
    /// 世代と種別で新規に開始する（既存の検知は先に畳む）。
    Start(u64, WebRequestKind),
    /// 何もしない（実行中。再描画ループを防ぐ）。
    Keep,
    /// 検知を畳む。
    Stop,
}

/// `start` に渡すしきい値（ms）。Web の下限 60,000 ms 以上。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn threshold_ms() -> f64 {
    IDLE_THRESHOLD.as_millis() as f64
}

/// `requestPermission()` の結果文字列を写す。granted 以外を成功に畳まない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn permission_outcome(v: Option<&str>) -> Result<(), ReadFailure> {
    match v {
        Some("granted") => Ok(()),
        Some("denied") => Err(ReadFailure::PermissionDenied),
        _ => Err(ReadFailure::Failed(FAILED_START.to_string())),
    }
}

/// DOM 例外の `name` を契約上の失敗へ写す（全呼び出し箇所で本関数に集約する）。
/// `message` は受け取らない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: &str) -> ReadFailure {
    match name {
        "NotAllowedError" => ReadFailure::PermissionDenied,
        _ => ReadFailure::Failed(FAILED_START.to_string()),
    }
}

/// `userState` / `screenState` を表示用へ写す。null・未知値・片方欠けは `None`
/// （呼び出し側で失敗にし、「操作中」へ畳まない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from(user: Option<&str>, screen: Option<&str>) -> Option<WebIdleReading> {
    let user = match user? {
        "active" => UserPresenceView::Active,
        "idle" => UserPresenceView::Idle,
        _ => return None,
    };
    let screen = match screen? {
        "locked" => ScreenLockView::Locked,
        "unlocked" => ScreenLockView::Unlocked,
        _ => return None,
    };
    Some(WebIdleReading { user, screen })
}

/// effect の判断。`watching_gen` は `Watching` のときだけ現在の世代。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decide(
    pending: Option<(u64, WebRequestKind)>,
    watching_gen: Option<u64>,
    entry_session: Option<u64>,
) -> Action {
    if let Some((gen, kind)) = pending {
        return if entry_session == Some(gen) {
            Action::Keep
        } else {
            Action::Start(gen, kind)
        };
    }
    match watching_gen {
        Some(g) if entry_session == Some(g) => Action::Keep,
        _ => Action::Stop,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::user_idle_show_effect;

/// マウント破棄時に検知・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

/// Idle Detection の呼び出し部品を `sleep_lock_events` と共有する（同じ API・同じ許可を使うため）。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::{detector_ctor, error_name, get};

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::user_idle_show::{
        apply_web_change, finish_request, pause_web, pending_request, resume_web, IdleView,
        ReadFailure, UserIdleShowState, WebIdleReading, WebRequestKind,
    };
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{
        decide, failure_from_error_name, permission_outcome, reading_from, threshold_ms, Action,
        FAILED_READ, FAILED_START, FAILED_TIMEOUT, START_TIMEOUT_MS,
    };
    use crate::compass::is_current;
    use crate::mount::{is_live, Applier};

    type State = UserIdleShowState;

    struct Entry {
        /// 動作中の要求の世代。
        session: Option<u64>,
        detector: Option<JsValue>,
        controller: Option<web_sys::AbortController>,
        start_timer: Option<i32>,
        listening_change: bool,
        lifecycle_registered: bool,
        on_change: Closure<dyn FnMut()>,
        on_timeout: Closure<dyn FnMut()>,
        on_pagehide: Closure<dyn FnMut()>,
        on_pageshow: Closure<dyn FnMut(JsValue)>,
    }

    thread_local! {
        /// マウントごとの状態（wasm32 は単一スレッド）。
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// 既存の Entry にだけ作用する。破棄済みマウントでは `None`（再生成して漏らさない）。
    fn with<R>(id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        ENTRIES.with(|m| m.borrow_mut().get_mut(&id).map(f))
    }

    pub(crate) fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    pub(crate) fn error_name(e: &JsValue) -> String {
        get(e, "name").as_string().unwrap_or_default()
    }

    /// `IdleDetector` コンストラクタ。非 secure context・非対応では `None`。
    pub(crate) fn detector_ctor() -> Option<Function> {
        let w = web_sys::window()?;
        if !w.is_secure_context() {
            return None;
        }
        get(&w, "IdleDetector").dyn_into::<Function>().ok()
    }

    fn read_states(detector: &JsValue) -> Option<WebIdleReading> {
        reading_from(
            get(detector, "userState").as_string().as_deref(),
            get(detector, "screenState").as_string().as_deref(),
        )
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
    }

    fn teardown(id: u64) {
        with(id, teardown_entry);
    }

    fn finish(applier: &Applier<State>, gen: u64, result: Result<WebIdleReading, ReadFailure>) {
        applier.apply(move |s| finish_request(s, gen, result));
    }

    fn failed(text: &str) -> ReadFailure {
        ReadFailure::Failed(text.to_string())
    }

    fn new_entry(applier: &Applier<State>, id: u64) -> Entry {
        let on_change = {
            let applier = applier.clone();
            Closure::<dyn FnMut()>::new(move || {
                let Some((Some(gen), Some(detector), timer)) = with(id, |e| {
                    (e.session, e.detector.clone(), e.start_timer.take())
                }) else {
                    return;
                };
                if let Some(h) = timer {
                    clear_timer(h);
                }
                match read_states(&detector) {
                    Some(r) => applier.apply(move |s| apply_web_change(s, gen, r)),
                    None => {
                        teardown(id);
                        finish(&applier, gen, Err(failed(FAILED_READ)));
                    }
                }
            })
        };
        let on_timeout = {
            let applier = applier.clone();
            Closure::<dyn FnMut()>::new(move || {
                // 発火済みのハンドルは clear しない。未確定（タイマーが残っている）ときだけ失敗にする。
                let gen = with(id, |e| {
                    let pending = e.start_timer.take().is_some();
                    let gen = e.session;
                    pending.then_some(gen).flatten()
                })
                .flatten();
                let Some(gen) = gen else {
                    return;
                };
                // 状態の確定（abort・解除）を結果の通知より先に行う（契約表 2・3）。
                teardown(id);
                finish(&applier, gen, Err(failed(FAILED_TIMEOUT)));
            })
        };
        let on_pagehide = {
            let applier = applier.clone();
            Closure::<dyn FnMut()>::new(move || {
                applier.apply(pause_web);
                teardown(id);
            })
        };
        let on_pageshow = {
            let applier = applier.clone();
            Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
                let persisted = ev
                    .dyn_ref::<web_sys::PageTransitionEvent>()
                    .is_some_and(|e| e.persisted());
                if persisted {
                    applier.apply(resume_web);
                }
            })
        };
        Entry {
            session: None,
            detector: None,
            controller: None,
            start_timer: None,
            listening_change: false,
            lifecycle_registered: false,
            on_change,
            on_timeout,
            on_pagehide,
            on_pageshow,
        }
    }

    /// window の pagehide / pageshow を 1 マウントにつき 1 度だけ登録する。失敗時は付けた分を外す。
    fn install_lifecycle(id: u64) -> bool {
        let Some(w) = web_sys::window() else {
            return false;
        };
        with(id, |e| {
            if e.lifecycle_registered {
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
            if w.add_event_listener_with_callback(
                "pageshow",
                e.on_pageshow.as_ref().unchecked_ref(),
            )
            .is_err()
            {
                let _ = w.remove_event_listener_with_callback(
                    "pagehide",
                    e.on_pagehide.as_ref().unchecked_ref(),
                );
                return false;
            }
            e.lifecycle_registered = true;
            true
        })
        .unwrap_or(false)
    }

    fn current(id: u64, gen: u64) -> bool {
        with(id, |e| is_current(is_live(id), e.session, gen)).unwrap_or(false)
    }

    /// 検知を開始する（同期部）。`start()` の Promise を待つ非同期部は `spawn_local` に渡す。
    fn begin_start(applier: &Applier<State>, id: u64, gen: u64) {
        let Some(ctor) = detector_ctor() else {
            finish(applier, gen, Err(ReadFailure::Unsupported));
            return;
        };
        let detector = match Reflect::construct(&ctor, &Array::new()) {
            Ok(d) => d,
            Err(e) => {
                finish(applier, gen, Err(failure_from_error_name(&error_name(&e))));
                return;
            }
        };
        let Ok(controller) = web_sys::AbortController::new() else {
            finish(applier, gen, Err(failed(FAILED_START)));
            return;
        };
        // 以降の確保はすべて Entry に格納し、失敗時は teardown で解放する（契約表 9）。
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
                finish(applier, gen, Err(failed(FAILED_START)));
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
                    finish(applier, gen, Err(failure_from_error_name(&error_name(&e))));
                    return;
                }
            },
            Err(_) => None,
        };
        let Some(promise) = promise else {
            teardown(id);
            finish(applier, gen, Err(failed(FAILED_START)));
            return;
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
                finish(applier, gen, Err(failed(FAILED_START)));
                return;
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
                    match detector.as_ref().and_then(read_states) {
                        Some(r) => finish(&applier, gen, Ok(r)),
                        None => {
                            teardown(id);
                            finish(&applier, gen, Err(failed(FAILED_READ)));
                        }
                    }
                }
                Err(e) => {
                    let failure = failure_from_error_name(&error_name(&e));
                    teardown(id);
                    finish(&applier, gen, Err(failure));
                }
            }
        });
    }

    /// `requestPermission()` を同期区間で呼び、許可後に開始する。
    fn request_permission_then_start(applier: &Applier<State>, id: u64, gen: u64) {
        let Some(ctor) = detector_ctor() else {
            finish(applier, gen, Err(ReadFailure::Unsupported));
            return;
        };
        let Ok(f) = get(&ctor, "requestPermission").dyn_into::<Function>() else {
            finish(applier, gen, Err(ReadFailure::Unsupported));
            return;
        };
        let promise = match f.call0(&ctor) {
            Ok(p) => p.dyn_into::<Promise>().ok(),
            Err(e) => {
                finish(applier, gen, Err(failure_from_error_name(&error_name(&e))));
                return;
            }
        };
        let Some(promise) = promise else {
            finish(applier, gen, Err(failed(FAILED_START)));
            return;
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
                Err(failure) => {
                    teardown(id);
                    finish(&applier, gen, Err(failure));
                }
            }
        });
    }

    /// `mount_with_effect` へ渡す effect。実行中は `Keep` で何もしない（再描画ループ防止）。
    pub fn user_idle_show_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        let pending = pending_request(state);
        let watching_gen = matches!(state.view, IdleView::Watching(_)).then_some(state.generation);
        let session = ENTRIES.with(|m| {
            m.borrow_mut()
                .entry(id)
                .or_insert_with(|| new_entry(applier, id))
                .session
        });
        match decide(pending, watching_gen, session) {
            Action::Keep => {}
            Action::Stop => teardown(id),
            Action::Start(gen, kind) => {
                with(id, |e| {
                    teardown_entry(e);
                    e.session = Some(gen);
                });
                if !install_lifecycle(id) {
                    teardown(id);
                    finish(applier, gen, Err(failed(FAILED_START)));
                    return;
                }
                match kind {
                    WebRequestKind::Permission => request_permission_then_start(applier, id, gen),
                    WebRequestKind::Resume => begin_start(applier, id, gen),
                }
            }
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(mut entry) = ENTRIES.with(|m| m.borrow_mut().remove(&mount_id)) else {
            return;
        };
        teardown_entry(&mut entry);
        if entry.lifecycle_registered {
            if let Some(w) = web_sys::window() {
                let _ = w.remove_event_listener_with_callback(
                    "pagehide",
                    entry.on_pagehide.as_ref().unchecked_ref(),
                );
                let _ = w.remove_event_listener_with_callback(
                    "pageshow",
                    entry.on_pageshow.as_ref().unchecked_ref(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_web_minimum_or_more() {
        assert_eq!(threshold_ms(), 60_000.0);
        assert!(threshold_ms() >= 60_000.0);
    }

    #[test]
    fn permission_outcome_never_fakes_success() {
        assert_eq!(permission_outcome(Some("granted")), Ok(()));
        assert_eq!(
            permission_outcome(Some("denied")),
            Err(ReadFailure::PermissionDenied)
        );
        for v in [None, Some("prompt"), Some("")] {
            assert_eq!(
                permission_outcome(v),
                Err(ReadFailure::Failed(FAILED_START.to_string()))
            );
        }
    }

    #[test]
    fn error_name_mapping_is_fixed_text() {
        assert_eq!(
            failure_from_error_name("NotAllowedError"),
            ReadFailure::PermissionDenied
        );
        for n in ["TypeError", "AbortError", "SecurityError", ""] {
            assert_eq!(
                failure_from_error_name(n),
                ReadFailure::Failed(FAILED_START.to_string())
            );
        }
    }

    #[test]
    fn reading_maps_known_values_only() {
        use ScreenLockView::*;
        use UserPresenceView::*;
        let r = |u, s| reading_from(Some(u), Some(s));
        assert_eq!(
            r("active", "unlocked"),
            Some(WebIdleReading {
                user: Active,
                screen: Unlocked
            })
        );
        assert_eq!(
            r("idle", "locked"),
            Some(WebIdleReading {
                user: Idle,
                screen: Locked
            })
        );
        assert_eq!(reading_from(None, Some("locked")), None);
        assert_eq!(reading_from(Some("active"), None), None);
        assert_eq!(r("away", "locked"), None);
        assert_eq!(r("active", "dim"), None);
    }

    #[test]
    fn decide_starts_once_per_generation() {
        let p = WebRequestKind::Permission;
        assert_eq!(decide(Some((1, p)), None, None), Action::Start(1, p));
        assert_eq!(decide(Some((1, p)), None, Some(1)), Action::Keep);
        assert_eq!(decide(Some((2, p)), None, Some(1)), Action::Start(2, p));
        assert_eq!(
            decide(Some((3, WebRequestKind::Resume)), None, Some(2)),
            Action::Start(3, WebRequestKind::Resume)
        );
    }

    #[test]
    fn decide_watching_and_idle_states() {
        assert_eq!(decide(None, Some(4), Some(4)), Action::Keep);
        assert_eq!(decide(None, Some(5), Some(4)), Action::Stop);
        assert_eq!(decide(None, Some(5), None), Action::Stop);
        // Paused・PermissionDenied・Unsupported・Failed・AwaitingRequest は pending も watching も無い。
        assert_eq!(decide(None, None, Some(4)), Action::Stop);
        assert_eq!(decide(None, None, None), Action::Stop);
    }
}
