//! 型サンプル「全画面表示」の Web 経路（親 #1133、Issue #1315）。
//!
//! `fandhe_core::screens::fullscreen_toggle`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。`take_request` で要求を観測し
//! Fullscreen API（`documentElement.requestFullscreen()` / `document.exitFullscreen()`）へ渡し、
//! `finish` で結果を返す。`fandhe-device` の trait は通らない（他の Web モジュールと同じ。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外）。
//!
//! # 方式
//!
//! - 呼び出しは利用者のクリックから effect まで同期に連なる中で行う（`spawn_local` や `await` の
//!   後だと transient activation を失い拒否されるため）。Promise は待つだけ
//! - 実状態は `document.fullscreenElement` だけを真実とする（`null` = 通常、object = 全画面、
//!   `undefined` = 読めない）。`fullscreenchange`（Esc 解除もここに届く）と `pageshow` で読み直す
//! - `fullscreenerror` と Promise の reject は失敗。resolve でも目標に届いていなければ
//!   `fullscreenchange` か期限（`CONFIRM_TIMEOUT_SECS` 秒、呼び出し開始が起点）を待つ
//! - 非対応（API 無し・`fullscreenEnabled` が偽）は `Unsupported`。prefixed API は使わない
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 表示は `fullscreenElement` に従う、非対応・拒否・読めない状態を全画面と見せない、
//! 1 マウント内でリスナーとタイマーを漏らさない。例外の `message` は転送しない（security.md A09）。
//! 守らない: 複数タブ・複数マウント間の表示の食い違い、期限後にブラウザが遅れて切り替えた場合の
//! 失敗文言（表示自体は追従する）、iframe の `allow="fullscreen"` 無しによる `Unsupported`、
//! マウント破棄時に全画面を解くこと、ページ終了時の後片付け（ブラウザが戻す）。

/// 拒否・例外時の固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const REJECTED: &str = "ブラウザが全画面への切り替えを拒否しました";

/// 要求の扱い。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Start {
    /// 既に目標の状態。API を呼ばない（通常時の `exitFullscreen` は TypeError で reject される）。
    Already,
    /// 非対応。
    Unsupported,
    /// API を呼ぶ。
    Call,
}

/// 要求を発送するかの判定。`observed=None`（読めない）は `Already` にしない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decide_start(
    target: bool,
    observed: Option<bool>,
    has_api: bool,
    enabled: bool,
) -> Start {
    if observed == Some(target) {
        Start::Already
    } else if !has_api || (target && !enabled) {
        Start::Unsupported
    } else {
        Start::Call
    }
}

/// 読み戻しが目標に届いたか。`None` は届いたと見なさない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reached(target: bool, observed: Option<bool>) -> bool {
    observed == Some(target)
}

#[cfg(target_arch = "wasm32")]
pub use web::fullscreen_toggle_effect;

/// マウント破棄時にリスナーとタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::fullscreen_toggle::{
        apply_observed, finish, mark_applying, take_request, FullscreenFailure,
        FullscreenToggleState, CONFIRM_TIMEOUT_SECS,
    };
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{decide_start, reached, Start, REJECTED};
    use crate::mount::{is_live, Applier};

    type State = FullscreenToggleState;

    struct Pending {
        generation: u64,
        target: bool,
        timer: (i32, Closure<dyn FnMut()>),
    }

    #[derive(Default)]
    struct Entry {
        /// 発送済みの世代（同じ世代を二度渡さない）。
        started: Option<u64>,
        pending: Option<Pending>,
        /// `fullscreenchange`（document）と `pageshow`（window）で共有する読み直し。
        on_change: Option<Closure<dyn FnMut()>>,
        on_error: Option<Closure<dyn FnMut()>>,
    }

    thread_local! {
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// 既存の Entry にだけ作用する。破棄済みマウントでは `None`（再生成して漏らさない）。
    fn with<R>(id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        ENTRIES.with(|e| e.borrow_mut().get_mut(&id).map(f))
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    /// `null` = 通常、object = 全画面、`undefined`・取得失敗 = 読めない（`Off` と偽らない）。
    fn observed() -> Option<bool> {
        let doc = web_sys::window()?.document()?;
        let v = get(&doc, "fullscreenElement");
        if v.is_null() {
            Some(false)
        } else if v.is_object() {
            Some(true)
        } else {
            None
        }
    }

    fn failed() -> Result<(), FullscreenFailure> {
        Err(FullscreenFailure::Failed(REJECTED.to_string()))
    }

    /// 保留中の要求を確定する。タイマーを止めてから結果を届ける（状態確定 → 通知の順、契約表 2・9）。
    fn complete(applier: &Applier<State>, id: u64, result: Result<(), FullscreenFailure>) {
        let Some(Some(p)) = with(id, |e| e.pending.take()) else {
            return;
        };
        if let Some(w) = web_sys::window() {
            w.clear_timeout_with_handle(p.timer.0);
        }
        let gen = p.generation;
        applier.apply(move |s| finish(s, gen, result, observed()));
    }

    /// マウントごとに 1 度だけ購読を登録し、初期値を反映する。
    fn install(applier: &Applier<State>, id: u64) {
        let Some(win) = web_sys::window() else {
            return;
        };
        let Some(doc) = win.document() else {
            return;
        };
        let fresh = ENTRIES.with(|e| match e.borrow_mut().entry(id) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Entry::default());
                true
            }
            _ => false,
        });
        if !fresh {
            return;
        }
        let a = applier.clone();
        let on_change = Closure::<dyn FnMut()>::new(move || {
            let obs = observed();
            a.apply(move |s| apply_observed(s, obs));
            let done = with(id, |e| {
                e.pending.as_ref().is_some_and(|p| reached(p.target, obs))
            })
            .unwrap_or(false);
            if done {
                complete(&a, id, Ok(()));
            }
        });
        let a = applier.clone();
        let on_error = Closure::<dyn FnMut()>::new(move || complete(&a, id, failed()));
        let _ = doc.add_event_listener_with_callback(
            "fullscreenchange",
            on_change.as_ref().unchecked_ref(),
        );
        let _ =
            win.add_event_listener_with_callback("pageshow", on_change.as_ref().unchecked_ref());
        let _ = doc
            .add_event_listener_with_callback("fullscreenerror", on_error.as_ref().unchecked_ref());
        with(id, |e| {
            e.on_change = Some(on_change);
            e.on_error = Some(on_error);
        });
        let obs = observed();
        applier.apply(move |s| apply_observed(s, obs));
    }

    /// `obj.method()` を呼んで戻り値を得る。例外は `Err`。
    fn call(obj: &JsValue, method: &str) -> Result<JsValue, JsValue> {
        let f: Function = get(obj, method).dyn_into()?;
        f.call0(obj)
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない（再描画ループ防止）。
    pub fn fullscreen_toggle_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        install(applier, id);
        let Some(req) = take_request(state) else {
            return;
        };
        let gen = req.generation;
        let fresh = with(id, |e| {
            let fresh = e.started != Some(gen);
            e.started = Some(gen);
            fresh
        })
        .unwrap_or(false);
        if !fresh {
            return;
        }
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let (target_obj, method) = if req.on {
            let root = doc.document_element().map(JsValue::from);
            (root.unwrap_or(JsValue::UNDEFINED), "requestFullscreen")
        } else {
            (JsValue::from(doc.clone()), "exitFullscreen")
        };
        let has_api = get(&target_obj, method).is_function();
        let enabled = get(&doc, "fullscreenEnabled").as_bool() == Some(true);
        match decide_start(req.on, observed(), has_api, enabled) {
            Start::Call => {}
            other => {
                let res = if other == Start::Already {
                    Ok(())
                } else {
                    Err(FullscreenFailure::Unsupported)
                };
                applier.apply(move |s| {
                    mark_applying(s);
                    finish(s, gen, res, observed());
                });
                return;
            }
        }
        // 期限は呼び出し開始が起点。先に予約して、以降の全経路を complete で一度だけ確定する。
        let a = applier.clone();
        let cb = Closure::<dyn FnMut()>::new(move || complete(&a, id, Ok(())));
        let handle = web_sys::window().and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                (CONFIRM_TIMEOUT_SECS * 1000) as i32,
            )
            .ok()
        });
        let Some(handle) = handle else {
            applier.apply(move |s| {
                mark_applying(s);
                finish(s, gen, failed(), observed());
            });
            return;
        };
        with(id, |e| {
            e.pending = Some(Pending {
                generation: gen,
                target: req.on,
                timer: (handle, cb),
            })
        });
        // transient activation を保つため、ここ（同期）で呼ぶ。
        applier.apply(mark_applying);
        let ret = match call(&target_obj, method) {
            Ok(r) => r,
            Err(_) => {
                complete(applier, id, failed());
                return;
            }
        };
        if let Ok(p) = ret.dyn_into::<Promise>() {
            let a = applier.clone();
            let target = req.on;
            wasm_bindgen_futures::spawn_local(async move {
                let ok = JsFuture::from(p).await.is_ok();
                // 再開後も同じ要求かを確かめる（契約表 2）。
                let current = is_live(id)
                    && with(id, |e| {
                        e.pending.as_ref().is_some_and(|p| p.generation == gen)
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
                if !ok {
                    complete(&a, id, failed());
                } else if reached(target, observed()) {
                    complete(&a, id, Ok(()));
                }
                // 未到達なら fullscreenchange か期限を待つ。
            });
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(mut entry) = ENTRIES.with(|e| e.borrow_mut().remove(&mount_id)) else {
            return;
        };
        let win = web_sys::window();
        if let (Some(p), Some(w)) = (entry.pending.take(), &win) {
            w.clear_timeout_with_handle(p.timer.0);
        }
        let doc = win.as_ref().and_then(|w| w.document());
        if let (Some(w), Some(d)) = (&win, &doc) {
            if let Some(cb) = entry.on_change.take() {
                let f = cb.as_ref().unchecked_ref();
                let _ = d.remove_event_listener_with_callback("fullscreenchange", f);
                let _ = w.remove_event_listener_with_callback("pageshow", f);
            }
            if let Some(cb) = entry.on_error.take() {
                let f = cb.as_ref().unchecked_ref();
                let _ = d.remove_event_listener_with_callback("fullscreenerror", f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_decision() {
        assert_eq!(decide_start(true, Some(true), true, true), Start::Already);
        assert_eq!(
            decide_start(false, Some(false), false, false),
            Start::Already
        );
        assert_eq!(
            decide_start(true, Some(false), false, true),
            Start::Unsupported
        );
        assert_eq!(
            decide_start(true, Some(false), true, false),
            Start::Unsupported
        );
        assert_eq!(decide_start(true, None, true, true), Start::Call);
        assert_eq!(decide_start(false, Some(true), true, false), Start::Call);
    }

    #[test]
    fn unknown_never_reached() {
        assert!(!reached(true, None));
        assert!(!reached(false, None));
        assert!(reached(true, Some(true)));
    }
}
