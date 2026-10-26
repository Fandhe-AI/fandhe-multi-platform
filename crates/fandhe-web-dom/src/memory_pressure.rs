//! 型サンプル「メモリ逼迫・負荷」の Web 経路（親 #179、Issue #914）。
//!
//! `fandhe_core::screens::memory_pressure_show` の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。呼び出し元は `demo.rs` のエントリポイントと、破棄時の解放を担う
//! `mount::sweep_mounts`。`fandhe-device` の `MemoryPressure` trait は経由しない
//! （`OnSignal` / `SubscriptionGuard` が `Send + Sync` を要し、`Closure` を持てないため。
//! 他の Web モジュールと同じ扱い）。`fandhe_device::memory_pressure` から使い回すのは
//! `PressureEvent` と `EventLog` だけ。
//!
//! # 方式
//!
//! Web にメモリ逼迫の標準通知は無い。Compute Pressure API の `PressureObserver.observe("cpu")` で
//! CPU の負荷段階（nominal / fair / serious / critical）を受け、メモリではなく CPU の負荷として出す。
//! `PressureObserver` には web-sys の安定した feature が無いため `js_sys::Reflect` で呼ぶ。
//!
//! | 状況 | 画面 |
//! |---|---|
//! | `PressureObserver` なし、または `knownSources` に `cpu` がない | 非対応 |
//! | `observe` が `NotSupportedError` | 非対応 |
//! | `observe` が `NotAllowedError`（Permissions Policy `compute-pressure`） | 許可なし |
//! | その他の拒否 | 開始失敗 |
//! | 利用者の停止 | 「止めました」（正常） |
//! | `pagehide` | 「画面を離れたため止めました」（正常） |
//!
//! 今の状態は Web では読めないため常に「この OS では取れない」と出す（最後の合図から作らない）。
//! 未知の source・state、欠けた値の記録は捨てる（捏造しない）。bfcache 復帰後に observer が配信を
//! 続けるかは仕様上確かめられていないため、`pagehide` で止めて「受け取り中」の嘘を作らない。
//!
//! # 期限
//!
//! `observe()` に許可ダイアログは無く、タイマーは置かない。解決しないまま残っても「開始中…」から
//! 停止ボタンで止められ、停止時に `disconnect` する。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の順序、未知値の非捏造、停止後・古い世代・破棄後の記録の破棄、
//! 構築・observe 失敗・停止・pagehide・破棄での `disconnect` と closure の解放。
//! 守らない: 画面を開く前と止めた後の合図、implicit focus を失った間の記録と間引き（仕様）、
//! `thermals`（仕様で at-risk）、Firefox・Safari（非対応表示）、複数タブ・複数マウント間の差。
//!
//! 例外・記録の生値は console にも DOM にも出さない（security.md A09）。

use fandhe_core::screens::memory_pressure_show::{
    EventKind, StartFailure, SupportView, KIND_COUNT,
};
use fandhe_device::memory_pressure::PressureEvent;

/// 対応表示を作る。CPU の 4 枠だけを、observer があり `knownSources` が `cpu` を含む（または
/// 取得できない）ときに有効にする。メモリの枠と今の状態は常に「取れない」。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn support_from(has_observer: bool, known_sources: Option<&[String]>) -> SupportView {
    let cpu = has_observer && known_sources.is_none_or(|s| s.iter().any(|x| x == "cpu"));
    let mut events = [false; KIND_COUNT];
    for e in [
        PressureEvent::CpuNominal,
        PressureEvent::CpuFair,
        PressureEvent::CpuSerious,
        PressureEvent::CpuCritical,
    ] {
        events[e.index()] = cpu;
    }
    SupportView {
        events,
        current: false,
    }
}

/// `PressureRecord` を合図へ写す。`cpu` 以外の source、未知・欠けた state は `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn event_from_record(
    source: Option<&str>,
    state: Option<&str>,
) -> Option<PressureEvent> {
    if source != Some("cpu") {
        return None;
    }
    match state? {
        "nominal" => Some(PressureEvent::CpuNominal),
        "fair" => Some(PressureEvent::CpuFair),
        "serious" => Some(PressureEvent::CpuSerious),
        "critical" => Some(PressureEvent::CpuCritical),
        _ => None,
    }
}

/// `observe` の拒否（例外名）を開始失敗の種類へ写す。写像はここ 1 か所に集める。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: Option<&str>) -> StartFailure {
    match name {
        Some("NotSupportedError") => StartFailure::Unsupported,
        Some("NotAllowedError") => StartFailure::NotAllowed,
        _ => StartFailure::Failed,
    }
}

/// device の合図を画面側の鏡像へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn kind_from_device(e: PressureEvent) -> EventKind {
    match e {
        PressureEvent::MemoryWarning => EventKind::MemoryWarning,
        PressureEvent::Normal => EventKind::Normal,
        PressureEvent::Warning => EventKind::Warning,
        PressureEvent::Critical => EventKind::Critical,
        PressureEvent::Trim(n) => EventKind::Trim(n),
        PressureEvent::Low => EventKind::Low,
        PressureEvent::High => EventKind::High,
        PressureEvent::CpuNominal => EventKind::CpuNominal,
        PressureEvent::CpuFair => EventKind::CpuFair,
        PressureEvent::CpuSerious => EventKind::CpuSerious,
        PressureEvent::CpuCritical => EventKind::CpuCritical,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::memory_pressure_show_effect;

/// マウント破棄時に observer とリスナーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::memory_pressure_show::{
        apply_init, apply_snapshot, finish_start, mark_left, status_text, take_init, take_start,
        CurrentView, EventKind, MemoryPressureShowState, Phase, StartFailure, KIND_COUNT,
    };
    use fandhe_device::memory_pressure::EventLog;
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{event_from_record, failure_from_error_name, kind_from_device, support_from};
    use crate::mount::{is_live, Applier};

    type State = MemoryPressureShowState;

    struct Sub {
        generation: u64,
        log: EventLog,
        observer: JsValue,
        _callback: Closure<dyn FnMut(JsValue, JsValue)>,
    }

    #[derive(Default)]
    struct Entry {
        init_sent: bool,
        sub: Option<Sub>,
        /// pagehide は Sub ではなく Entry が持つ（実行中の closure 自身を drop させないため）。
        pagehide: Option<Closure<dyn FnMut(web_sys::Event)>>,
    }

    thread_local! {
        /// マウントごとの状態（wasm32 は単一スレッド）。
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn observer_ctor() -> Option<Function> {
        let win = web_sys::window()?;
        get(win.as_ref(), "PressureObserver")
            .dyn_into::<Function>()
            .ok()
    }

    fn known_sources(ctor: &Function) -> Option<Vec<String>> {
        let v = get(ctor.as_ref(), "knownSources");
        if v.is_undefined() || v.is_null() {
            return None;
        }
        Some(
            Array::from(&v)
                .iter()
                .filter_map(|x| x.as_string())
                .collect(),
        )
    }

    fn disconnect(observer: &JsValue) {
        if let Ok(f) = get(observer, "disconnect").dyn_into::<Function>() {
            let _ = f.call0(observer);
        }
    }

    /// observer を止めてから closure を drop する（冪等）。
    fn release_sub(sub: Sub) {
        disconnect(&sub.observer);
    }

    fn take_sub(id: u64) -> Option<Sub> {
        ENTRIES.with(|m| m.borrow_mut().get_mut(&id).and_then(|e| e.sub.take()))
    }

    fn snapshot_of(id: u64) -> (Vec<(u64, EventKind)>, [u64; KIND_COUNT]) {
        ENTRIES.with(|m| {
            m.borrow()
                .get(&id)
                .and_then(|e| e.sub.as_ref())
                .map(|s| {
                    let (recent, counts) = s.log.snapshot();
                    (
                        recent
                            .into_iter()
                            .map(|(n, e)| (n, kind_from_device(e)))
                            .collect(),
                        counts,
                    )
                })
                .unwrap_or_default()
        })
    }

    fn make_callback(
        applier: &Applier<State>,
        id: u64,
        gen: u64,
    ) -> Closure<dyn FnMut(JsValue, JsValue)> {
        let applier = applier.clone();
        Closure::<dyn FnMut(JsValue, JsValue)>::new(move |records: JsValue, _observer: JsValue| {
            if !is_live(id) {
                return;
            }
            let events: Vec<_> = Array::from(&records)
                .iter()
                .filter_map(|r| {
                    event_from_record(
                        get(&r, "source").as_string().as_deref(),
                        get(&r, "state").as_string().as_deref(),
                    )
                })
                .collect();
            let recorded = ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let sub = m.get_mut(&id)?.sub.as_mut()?;
                if sub.generation != gen || events.is_empty() {
                    return None;
                }
                for e in events {
                    sub.log.record(e);
                }
                Some(())
            });
            if recorded.is_none() {
                return;
            }
            let (recent, counts) = snapshot_of(id);
            // 全再構築すると停止ボタンの click が成立しないため、状態文だけを書き換える。
            applier.apply_in_place(
                move |s| apply_snapshot(s, gen, recent, counts, None),
                patch_status,
            );
        })
    }

    /// 状態文（先頭の `p`）だけを既存の DOM へ直接書く。要素が無ければ `false`（全再構築へ倒す）。
    fn patch_status(root: &web_sys::Element, state: &State) -> bool {
        match root.query_selector("p") {
            Ok(Some(p)) => {
                p.set_text_content(Some(&status_text(state)));
                true
            }
            _ => false,
        }
    }

    fn name_of(e: &JsValue) -> Option<String> {
        get(e, "name").as_string()
    }

    fn do_start(applier: &Applier<State>, id: u64, gen: u64) {
        let fail = |f: StartFailure| applier.apply(move |s| finish_start(s, gen, Err(f)));
        let Some(ctor) = observer_ctor() else {
            return fail(StartFailure::Unsupported);
        };
        let callback = make_callback(applier, id, gen);
        let Ok(observer) = Reflect::construct(&ctor, &Array::of1(callback.as_ref())) else {
            return fail(StartFailure::Failed);
        };
        let Ok(observe) = get(&observer, "observe").dyn_into::<Function>() else {
            return fail(StartFailure::Failed);
        };
        let promise = match observe.call1(&observer, &JsValue::from_str("cpu")) {
            Ok(p) => p,
            Err(e) => {
                disconnect(&observer);
                return fail(failure_from_error_name(name_of(&e).as_deref()));
            }
        };
        // await の前に登録する。待っている間に停止されても、照合段で disconnect できる。
        let registered = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let Some(e) = m.get_mut(&id) else {
                return false;
            };
            e.sub = Some(Sub {
                generation: gen,
                log: EventLog::new(),
                observer: observer.clone(),
                _callback: callback,
            });
            true
        });
        if !registered {
            disconnect(&observer);
            return;
        }
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(js_sys::Promise::from(promise)).await;
            let current = is_live(id)
                && ENTRIES.with(|m| {
                    m.borrow()
                        .get(&id)
                        .and_then(|e| e.sub.as_ref())
                        .is_some_and(|s| s.generation == gen)
                });
            match result {
                Ok(_) if current => applier.apply(move |s| {
                    finish_start(s, gen, Ok(()));
                    // 開始完了前に届いた記録は反映時点で読む。
                    let (recent, counts) = snapshot_of(id);
                    apply_snapshot(s, gen, recent, counts, None);
                }),
                Ok(_) => disconnect(&observer),
                Err(_) if !current => disconnect(&observer),
                Err(e) => {
                    if let Some(sub) = take_sub(id) {
                        release_sub(sub);
                    }
                    let f = failure_from_error_name(name_of(&e).as_deref());
                    applier.apply(move |s| finish_start(s, gen, Err(f)));
                }
            }
        });
    }

    /// マウントごとに 1 回、pagehide を登録する。bfcache 入りで止めて Left にする（契約表 1）。
    fn ensure_pagehide(applier: &Applier<State>, id: u64) {
        let has = ENTRIES.with(|m| m.borrow().get(&id).is_some_and(|e| e.pagehide.is_some()));
        if has {
            return;
        }
        let Some(win) = web_sys::window() else {
            return;
        };
        let ap = applier.clone();
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
            if !ev.is_trusted() || !is_live(id) {
                return;
            }
            let Some(sub) = take_sub(id) else {
                return;
            };
            let gen = sub.generation;
            release_sub(sub);
            ap.apply(move |s| mark_left(s, gen));
        });
        if win
            .add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref())
            .is_ok()
        {
            ENTRIES.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&id) {
                    e.pagehide = Some(closure);
                }
            });
        }
    }

    pub fn memory_pressure_show_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        ENTRIES.with(|m| {
            m.borrow_mut().entry(id).or_default();
        });
        ensure_pagehide(applier, id);
        // 1. 照合: 停止・世代違いの購読を畳む（契約表 2・10）。
        let stale = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.get_mut(&id)?;
            let drop_it = e
                .sub
                .as_ref()
                .is_some_and(|s| state.phase == Phase::Idle || s.generation != state.generation);
            if drop_it {
                e.sub.take()
            } else {
                None
            }
        });
        if let Some(sub) = stale {
            release_sub(sub);
        }
        // 2. 初回: 対応状況を読む。今の状態は Web では読めない。
        let init_sent = ENTRIES.with(|m| m.borrow().get(&id).is_some_and(|e| e.init_sent));
        if take_init(state) && !init_sent {
            ENTRIES.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&id) {
                    e.init_sent = true;
                }
            });
            let ctor = observer_ctor();
            let sources = ctor.as_ref().and_then(known_sources);
            let support = support_from(ctor.is_some(), sources.as_deref());
            applier.apply(move |s| apply_init(s, support, CurrentView::NotAvailable));
        }
        // 3. 開始: 同じ世代で二重に登録しない。
        if let Some(gen) = take_start(state) {
            let has_sub = ENTRIES.with(|m| {
                m.borrow()
                    .get(&id)
                    .and_then(|e| e.sub.as_ref())
                    .is_some_and(|s| s.generation == gen)
            });
            if !has_sub {
                do_start(applier, id, gen);
            }
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(entry) = ENTRIES.with(|m| m.borrow_mut().remove(&mount_id)) else {
            return;
        };
        if let Some(sub) = entry.sub {
            release_sub(sub);
        }
        if let (Some(c), Some(win)) = (entry.pagehide, web_sys::window()) {
            let _ = win.remove_event_listener_with_callback("pagehide", c.as_ref().unchecked_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_slots(s: &SupportView) -> Vec<bool> {
        (7..=10).map(|i| s.events[i]).collect()
    }

    #[test]
    fn support_cases() {
        let none = support_from(false, None);
        assert!(none.events.iter().all(|e| !e) && !none.current);
        let no_cpu = support_from(true, Some(&["thermals".to_string()]));
        assert!(no_cpu.events.iter().all(|e| !e));
        assert_eq!(cpu_slots(&support_from(true, None)), [true; 4]);
        let ok = support_from(true, Some(&["cpu".to_string()]));
        assert_eq!(cpu_slots(&ok), [true; 4]);
        assert!(ok.events[..7].iter().all(|e| !e) && !ok.current);
    }

    #[test]
    fn record_mapping() {
        use PressureEvent::*;
        let f = |s| event_from_record(Some("cpu"), Some(s));
        assert_eq!(f("nominal"), Some(CpuNominal));
        assert_eq!(f("fair"), Some(CpuFair));
        assert_eq!(f("serious"), Some(CpuSerious));
        assert_eq!(f("critical"), Some(CpuCritical));
        assert_eq!(f("bogus"), None);
        assert_eq!(event_from_record(Some("thermals"), Some("nominal")), None);
        assert_eq!(event_from_record(None, Some("fair")), None);
        assert_eq!(event_from_record(Some("cpu"), None), None);
    }

    #[test]
    fn failure_mapping() {
        assert_eq!(
            failure_from_error_name(Some("NotSupportedError")),
            StartFailure::Unsupported
        );
        assert_eq!(
            failure_from_error_name(Some("NotAllowedError")),
            StartFailure::NotAllowed
        );
        assert_eq!(
            failure_from_error_name(Some("InvalidStateError")),
            StartFailure::Failed
        );
        assert_eq!(failure_from_error_name(None), StartFailure::Failed);
    }

    #[test]
    fn kind_covers_all_events() {
        for (i, e) in fandhe_device::memory_pressure::ALL_EVENTS
            .iter()
            .enumerate()
        {
            assert_eq!(kind_from_device(*e).index(), i);
        }
    }
}
