//! 型サンプル「アプリの前面・背面」の Web 経路（親 #178、Issue #896）。
//!
//! `fandhe_core::screens::app_lifecycle_show` の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。呼び出し元は `demo.rs` のエントリポイントと、破棄時の解放を担う
//! `mount::sweep_mounts`。`fandhe-device` の `AppLifecycle` trait は経由しない
//! （`OnSignal` / `SubscriptionGuard` が `Send + Sync` を要し、`Closure` を持てないため。
//! 他の Web モジュールと同じ扱い）。結果の契約は trait doc に従い、`fandhe_device::app_lifecycle` から
//! 使い回すのは `EventLog` と `LifecycleEvent` だけ。
//!
//! # 方式
//!
//! | 出来事 | 通知 | 受け口 | 対応 |
//! |---|---|---|---|
//! | 操作の対象になった / 外れた | `focus` / `blur` | window | 常に |
//! | 見えなくなった / 見えるようになった | `visibilitychange`（届いた時点の `visibilityState` で分ける） | document | 常に |
//! | 凍結 / 凍結から戻った | `freeze` / `resume` | document | `onfreeze` があるとき（Chromium 系）だけ |
//! | キャッシュから戻った | `pageshow`（`persisted` が真のとき） | window | 常に |
//!
//! 今の状態は画面を開いた時と出来事が届くたびに `document.hasFocus()` と `visibilityState` から
//! 読み直す。読めない値を「前面」「見えている」へ畳まない。`isTrusted` が偽の出来事（ページ内
//! スクリプトの偽装）は数えない。
//!
//! bfcache 中もリスナーを保つ（pagehide で外さない）。復帰時は `pageshow(persisted)` を記録し、
//! 今の状態を読み直す。タイマー・推測による補完は使わない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の順序、読めない状態の非畳み込み、停止後・古い世代・破棄後の出来事の破棄、
//! 登録失敗・停止・マウント破棄でのリスナー解放、開始完了前に届いた出来事の保持。
//! 守らない: 画面を開く前と止めた後の出来事、discard、複数タブ・複数マウント間の差、iframe 内の
//! フォーカス移動、ブラウザごとの「見える」の定義差、Firefox・Safari の freeze / resume
//! （「取れない」表示）。
//!
//! 例外・ブラウザの値は console にも DOM にも出さない（security.md A09）。

use fandhe_core::screens::app_lifecycle_show::{
    ActiveView, CurrentView, EventKind, SupportView, VisibleView,
};
use fandhe_device::app_lifecycle::LifecycleEvent;

/// 出来事を取れる環境か（DOM の有無と freeze の有無）から対応表示を作る。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn support_from(has_dom: bool, has_freeze: bool) -> SupportView {
    let mut events = [has_dom; 7];
    events[LifecycleEvent::Frozen.index()] = has_dom && has_freeze;
    events[LifecycleEvent::Unfrozen.index()] = has_dom && has_freeze;
    SupportView {
        events,
        active: has_dom,
        visible: has_dom,
    }
}

/// DOM の通知を出来事へ写す。`trusted` が偽、未知の種類、出来事にならない値は `None`。
/// `window_target` は `focus` / `blur` の対象が window かどうか。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn event_from_dom(
    kind: &str,
    trusted: bool,
    window_target: bool,
    persisted: Option<bool>,
    visibility: Option<&str>,
) -> Option<LifecycleEvent> {
    if !trusted {
        return None;
    }
    match kind {
        "focus" if window_target => Some(LifecycleEvent::Activated),
        "blur" if window_target => Some(LifecycleEvent::Deactivated),
        "freeze" => Some(LifecycleEvent::Frozen),
        "resume" => Some(LifecycleEvent::Unfrozen),
        "pageshow" if persisted == Some(true) => Some(LifecycleEvent::RestoredFromCache),
        "visibilitychange" => match visibility {
            Some("hidden") => Some(LifecycleEvent::Hidden),
            Some("visible") => Some(LifecycleEvent::Shown),
            _ => None,
        },
        _ => None,
    }
}

/// 今の状態を表示用へ写す。`visibility` の `None` は文字列ではなかった（読み取り失敗）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn current_from_dom(
    has_focus: Result<bool, ()>,
    visibility: Option<&str>,
) -> CurrentView {
    CurrentView {
        active: match has_focus {
            Ok(true) => ActiveView::Active,
            Ok(false) => ActiveView::Inactive,
            Err(()) => ActiveView::ReadFailed,
        },
        visible: match visibility {
            Some("visible") => VisibleView::Visible,
            Some("hidden") => VisibleView::Hidden,
            Some(_) => VisibleView::Unknown,
            None => VisibleView::ReadFailed,
        },
    }
}

/// device の出来事を画面側の鏡像へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn kind_from_device(e: LifecycleEvent) -> EventKind {
    match e {
        LifecycleEvent::Activated => EventKind::Activated,
        LifecycleEvent::Deactivated => EventKind::Deactivated,
        LifecycleEvent::Hidden => EventKind::Hidden,
        LifecycleEvent::Shown => EventKind::Shown,
        LifecycleEvent::Frozen => EventKind::Frozen,
        LifecycleEvent::Unfrozen => EventKind::Unfrozen,
        LifecycleEvent::RestoredFromCache => EventKind::RestoredFromCache,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::app_lifecycle_show_effect;

/// マウント破棄時にリスナーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::app_lifecycle_show::{
        apply_init, apply_snapshot, finish_start, status_text, take_init, take_start,
        AppLifecycleShowState, CurrentView, EventKind, Phase, StartFailure,
    };
    use fandhe_device::app_lifecycle::EventLog;
    use js_sys::Reflect;
    use wasm_bindgen::prelude::*;

    use super::{current_from_dom, event_from_dom, kind_from_device, support_from};
    use crate::mount::{is_live, Applier};

    type State = AppLifecycleShowState;

    struct Sub {
        generation: u64,
        log: EventLog,
        closure: Closure<dyn FnMut(web_sys::Event)>,
        registered: Vec<(web_sys::EventTarget, &'static str)>,
    }

    #[derive(Default)]
    struct Entry {
        init_sent: bool,
        sub: Option<Sub>,
    }

    thread_local! {
        /// マウントごとの状態（wasm32 は単一スレッド）。
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    fn visibility_state(doc: &web_sys::Document) -> Option<String> {
        Reflect::get(doc, &JsValue::from_str("visibilityState"))
            .ok()
            .and_then(|v| v.as_string())
    }

    fn read_current() -> CurrentView {
        match web_sys::window().and_then(|w| w.document()) {
            Some(d) => current_from_dom(
                d.has_focus().map_err(|_| ()),
                visibility_state(&d).as_deref(),
            ),
            None => current_from_dom(Err(()), None),
        }
    }

    fn has_freeze(doc: &web_sys::Document) -> bool {
        Reflect::has(doc, &JsValue::from_str("onfreeze")).unwrap_or(false)
    }

    /// 購読のリスナーを全部外す（冪等）。クロージャは Sub と一緒に drop される。
    fn release_sub(sub: Sub) {
        for (t, ty) in &sub.registered {
            let _ = t.remove_event_listener_with_callback(ty, sub.closure.as_ref().unchecked_ref());
        }
    }

    fn snapshot_of(id: u64) -> (Vec<(u64, EventKind)>, [u64; 7]) {
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

    fn make_closure(
        applier: &Applier<State>,
        id: u64,
        gen: u64,
    ) -> Closure<dyn FnMut(web_sys::Event)> {
        let applier = applier.clone();
        Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
            if !is_live(id) {
                return;
            }
            let doc = web_sys::window().and_then(|w| w.document());
            let visibility = doc.as_ref().and_then(visibility_state);
            let mapped = event_from_dom(
                &ev.type_(),
                ev.is_trusted(),
                ev.target()
                    .is_some_and(|t| t.dyn_ref::<web_sys::Window>().is_some()),
                ev.dyn_ref::<web_sys::PageTransitionEvent>()
                    .map(|e| e.persisted()),
                visibility.as_deref(),
            );
            let Some(mapped) = mapped else {
                return;
            };
            // 記録は借用の内側、apply（同期で再描画と effect が走る）は借用を落としてから。
            let recorded = ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let sub = m.get_mut(&id)?.sub.as_mut()?;
                if sub.generation != gen {
                    return None;
                }
                sub.log.record(mapped);
                Some(())
            });
            if recorded.is_none() {
                return;
            }
            let (recent, counts) = snapshot_of(id);
            let current = read_current();
            // 全再構築すると停止ボタンの DOM が mousedown と mouseup の間に除去され、click が
            // 成立しない（focus で戻った直後のクリック）。状態文だけを書き換えてボタンを残す。
            applier.apply_in_place(
                move |s| apply_snapshot(s, gen, recent, counts, Some(current)),
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

    fn do_start(applier: &Applier<State>, id: u64, gen: u64, freeze: bool) {
        let fail = |f: StartFailure| applier.apply(move |s| finish_start(s, gen, Err(f)));
        let (Some(win), Some(doc)) = (
            web_sys::window(),
            web_sys::window().and_then(|w| w.document()),
        ) else {
            return fail(StartFailure::Unsupported);
        };
        let closure = make_closure(applier, id, gen);
        let mut targets: Vec<(web_sys::EventTarget, &'static str)> = vec![
            (win.clone().into(), "focus"),
            (win.clone().into(), "blur"),
            (win.into(), "pageshow"),
            (doc.clone().into(), "visibilitychange"),
        ];
        if freeze {
            targets.push((doc.clone().into(), "freeze"));
            targets.push((doc.into(), "resume"));
        }
        let mut registered = Vec::new();
        for (t, ty) in targets {
            if t.add_event_listener_with_callback(ty, closure.as_ref().unchecked_ref())
                .is_err()
            {
                // 登録に失敗したら付けた分を外す（契約表 9）。closure は drop される。
                release_sub(Sub {
                    generation: gen,
                    log: EventLog::new(),
                    closure,
                    registered,
                });
                return fail(StartFailure::Failed);
            }
            registered.push((t, ty));
        }
        ENTRIES.with(|m| {
            m.borrow_mut().entry(id).or_default().sub = Some(Sub {
                generation: gen,
                log: EventLog::new(),
                closure,
                registered,
            });
        });
        // 開始完了前に届いた出来事を落とさないよう、記録と今の状態は反映時点で読む。
        applier.apply(move |s| {
            finish_start(s, gen, Ok(()));
            let (recent, counts) = snapshot_of(id);
            apply_snapshot(s, gen, recent, counts, Some(read_current()));
        });
    }

    pub fn app_lifecycle_show_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        // 1. 照合: 停止・世代違いの購読を畳む（契約表 2・10）。
        let stale = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.entry(id).or_default();
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
        let doc = web_sys::window().and_then(|w| w.document());
        // 2. 初回: 対応状況と今の状態を読む。
        let init_sent = ENTRIES.with(|m| m.borrow().get(&id).is_some_and(|e| e.init_sent));
        if take_init(state) && !init_sent {
            ENTRIES.with(|m| m.borrow_mut().entry(id).or_default().init_sent = true);
            let support = support_from(doc.is_some(), doc.as_ref().is_some_and(has_freeze));
            let current = read_current();
            applier.apply(move |s| apply_init(s, support, current));
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
                do_start(applier, id, gen, doc.as_ref().is_some_and(has_freeze));
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LifecycleEvent::*;

    #[test]
    fn maps_basic_events() {
        assert_eq!(
            event_from_dom("focus", true, true, None, None),
            Some(Activated)
        );
        assert_eq!(
            event_from_dom("blur", true, true, None, None),
            Some(Deactivated)
        );
        assert_eq!(
            event_from_dom("freeze", true, false, None, None),
            Some(Frozen)
        );
        assert_eq!(
            event_from_dom("resume", true, false, None, None),
            Some(Unfrozen)
        );
    }

    #[test]
    fn pageshow_only_when_persisted() {
        assert_eq!(
            event_from_dom("pageshow", true, true, Some(true), None),
            Some(RestoredFromCache)
        );
        assert_eq!(
            event_from_dom("pageshow", true, true, Some(false), None),
            None
        );
        assert_eq!(event_from_dom("pageshow", true, true, None, None), None);
    }

    #[test]
    fn visibility_split_and_unknown() {
        let f = |v| event_from_dom("visibilitychange", true, false, None, v);
        assert_eq!(f(Some("hidden")), Some(Hidden));
        assert_eq!(f(Some("visible")), Some(Shown));
        assert_eq!(f(Some("prerender")), None);
        assert_eq!(f(None), None);
    }

    #[test]
    fn untrusted_and_non_window_dropped() {
        assert_eq!(event_from_dom("focus", false, true, None, None), None);
        assert_eq!(event_from_dom("focus", true, false, None, None), None);
        assert_eq!(event_from_dom("unknown", true, true, None, None), None);
    }

    #[test]
    fn support_cases() {
        assert!(support_from(false, true).events.iter().all(|e| !e));
        let s = support_from(true, false);
        assert!(s.active && s.visible);
        assert_eq!(s.events, [true, true, true, true, false, false, true]);
        assert!(support_from(true, true).events.iter().all(|e| *e));
    }

    #[test]
    fn current_does_not_fold_unreadable() {
        let c = current_from_dom(Err(()), None);
        assert_eq!(c.active, ActiveView::ReadFailed);
        assert_eq!(c.visible, VisibleView::ReadFailed);
        let c = current_from_dom(Ok(false), Some("prerender"));
        assert_eq!(c.active, ActiveView::Inactive);
        assert_eq!(c.visible, VisibleView::Unknown);
        let c = current_from_dom(Ok(true), Some("visible"));
        assert_eq!(
            (c.active, c.visible),
            (ActiveView::Active, VisibleView::Visible)
        );
    }

    #[test]
    fn kind_covers_all_events() {
        for (i, e) in fandhe_device::app_lifecycle::ALL_EVENTS.iter().enumerate() {
            assert_eq!(kind_from_device(*e).index(), i);
        }
    }
}
