//! 型サンプル「画面スリープの抑止」の Web 経路（親 #1129、Issue #1281）。
//!
//! `fandhe_core::screens::wake_lock_toggle`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。`take_request` で要求を観測し
//! Screen Wake Lock API（`navigator.wakeLock.request("screen")`）へ渡して `finish` で結果を返す。
//! `fandhe-device` の trait は通らない（他の Web モジュールと同じ。`fandhe-egui-proxy` の
//! wasm canvas 経路は対象外）。
//!
//! # Reflect を使う理由
//!
//! web-sys の `WakeLock` / `WakeLockSentinel` は `web_sys_unstable_apis` が必須の不安定 API。
//! ビルド設定を変えないため `js_sys::Reflect` で呼ぶ（`background_task` の `SyncManager` と同方針）。
//!
//! # 方式
//!
//! - 取得: `request("screen")` の resolve 後、`released` を読み戻して `held` を決める
//! - 解除: 保持中の sentinel の `release()`
//! - sentinel の `release` イベント（タブ非表示・bfcache 等でブラウザが解放）: 即 `apply_held(false)`
//! - `visibilitychange`: 可視に戻り、抑止を望んでいて、保持していなければ取り直す
//! - 非同期の再開後は世代（epoch）とマウントの生存を再確認し、古い結果で取れた sentinel は即解放する
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内で sentinel を漏らさない（二重取得しない・解除後や破棄後に取れたものは即解放・
//! マウント破棄で解放）、`released` が偽の間だけ「抑止しています」、非対応・拒否・secure context 外を
//! 成功と見せない。例外の `message` は転送しない（security.md A09）。
//! 守らない: 複数タブ・複数マウント間の表示の食い違い、背景タブで効かないこと（仕様上ブラウザが
//! 解放）、ブラウザが取り直しを拒む間の追従（`PENDING_STATUS` のまま）、ページ終了時の後片付け
//! （ブラウザが解放）。

use fandhe_core::screens::wake_lock_toggle::WakeLockFailure;

/// 拒否・例外時の固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const REJECTED: &str = "ブラウザが画面スリープの抑止を拒否しました";

/// 取得結果を `finish` へ渡す `(結果, held)` へ写す純関数。`resolved` は resolve 時の
/// `released`、reject は `None`。`released=true` は `Ok` のまま返し、`finish` が失敗へ畳む。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn acquire_outcome(
    has_api: bool,
    resolved: Option<bool>,
) -> (Result<(), WakeLockFailure>, bool) {
    if !has_api {
        return (Err(WakeLockFailure::Unsupported), false);
    }
    match resolved {
        None => (Err(WakeLockFailure::Failed(REJECTED.to_string())), false),
        Some(released) => (Ok(()), !released),
    }
}

/// 可視復帰時に取り直すか。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn should_reacquire(hidden: bool, desired: bool, holding: bool, busy: bool) -> bool {
    !hidden && desired && !holding && !busy
}

/// 処理中（busy）に届いた可視復帰を、完了後に再評価するため記録するか。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn defer_visible(hidden: bool, busy: bool) -> bool {
    !hidden && busy
}

/// 非同期の再開後に結果を採用してよいか（マウント生存・世代一致・まだ望まれている）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_current(live: bool, entry_epoch: u64, started_epoch: u64, desired: bool) -> bool {
    live && entry_epoch == started_epoch && desired
}

#[cfg(target_arch = "wasm32")]
pub use web::wake_lock_toggle_effect;

/// マウント破棄時に sentinel とリスナーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::wake_lock_toggle::{
        apply_held, finish, mark_applying, take_request, WakeLockFailure, WakeLockToggleState,
    };
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{acquire_outcome, defer_visible, is_current, should_reacquire, REJECTED};
    use crate::mount::Applier;

    type Outcome = (Result<(), WakeLockFailure>, bool);
    type State = WakeLockToggleState;

    #[derive(Default)]
    struct Entry {
        /// 保持中の `WakeLockSentinel`。
        sentinel: Option<JsValue>,
        /// 利用者が抑止を望んでいるか（可視復帰で取り直すかの判断に使う）。
        desired: bool,
        /// 取得・解除のたびに進める世代。
        epoch: u64,
        /// effect または可視復帰の処理中（二重起動防止）。
        busy: bool,
        /// busy 中に可視復帰が届いた。完了時に一度だけ再評価する。
        revisit: bool,
        /// `release` イベント購読（購読先の sentinel とクロージャ）。
        listener: Option<(JsValue, Closure<dyn FnMut()>)>,
        /// `document` の `visibilitychange` 購読。
        on_visible: Option<Closure<dyn FnMut()>>,
    }

    thread_local! {
        /// マウントごとの状態（wasm32 は単一スレッド）。
        static LOCKS: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// 既存の Entry にだけ作用する。破棄済みマウントでは `None`（再生成して漏らさない）。
    fn with<R>(id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        LOCKS.with(|l| l.borrow_mut().get_mut(&id).map(f))
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    /// `obj.method(arg)` を呼んで Promise を得る。例外・非 Promise は `Err`。
    fn call_promise(obj: &JsValue, method: &str, arg: Option<&str>) -> Result<Promise, JsValue> {
        let f: Function = get(obj, method).dyn_into()?;
        let ret = match arg {
            Some(a) => f.call1(obj, &JsValue::from_str(a))?,
            None => f.call0(obj)?,
        };
        ret.dyn_into()
    }

    /// `navigator.wakeLock`。secure context 外・非対応では `None`（型に現れない実態を検証する、A03）。
    fn wake_lock_api() -> Option<JsValue> {
        let nav = web_sys::window()?.navigator();
        let wl = get(&nav, "wakeLock");
        (wl.is_object() && get(&wl, "request").is_function()).then_some(wl)
    }

    fn released(sentinel: &JsValue) -> bool {
        get(sentinel, "released").as_bool().unwrap_or(true)
    }

    /// 旧 `release` 購読を外して破棄する（破棄済みクロージャが呼ばれないよう先に remove）。
    fn detach_listener(entry: &mut Entry) {
        if let Some((target, cb)) = entry.listener.take() {
            if let Some(t) = target.dyn_ref::<web_sys::EventTarget>() {
                let _ =
                    t.remove_event_listener_with_callback("release", cb.as_ref().unchecked_ref());
            }
        }
    }

    /// 取得して `finish` へ渡す `(結果, held)` を返す。保持済みなら新規要求しない（冪等）。
    ///
    /// `retry` は可視復帰の自動取り直し。拒否されても利用者の抑止要求（`desired`）は保持し、
    /// 次の可視復帰で再試行する（初回要求の拒否だけが `desired` を落とす）。
    async fn acquire(applier: &Applier<State>, id: u64, retry: bool) -> Outcome {
        let Some(wl) = wake_lock_api() else {
            return acquire_outcome(false, None);
        };
        let Some(start) = with(id, |e| {
            e.desired = true;
            if e.sentinel.as_ref().is_some_and(|s| !released(s)) {
                return None;
            }
            e.epoch += 1;
            Some(e.epoch)
        }) else {
            return (Ok(()), false);
        };
        let Some(epoch) = start else {
            return (Ok(()), true);
        };
        let result = match call_promise(&wl, "request", Some("screen")) {
            Ok(p) => JsFuture::from(p).await.ok(),
            Err(_) => None,
        };
        let Some(sentinel) = result else {
            if !retry {
                with(id, |e| e.desired = false);
            }
            return acquire_outcome(true, None);
        };
        // await 再開後の再確認。古い結果で取れた sentinel は即解放する（契約表 2・9）。
        let adopt = with(id, |e| is_current(true, e.epoch, epoch, e.desired)).unwrap_or(false);
        if !adopt {
            let _ = call_promise(&sentinel, "release", None);
            return (Ok(()), false);
        }
        let on_release = {
            let applier = applier.clone();
            let target = sentinel.clone();
            Closure::<dyn FnMut()>::new(move || {
                // 自分で外した sentinel は Entry から取り除き済みなので無視される。
                let ours = with(id, |e| {
                    let same = e
                        .sentinel
                        .as_ref()
                        .is_some_and(|s| js_sys::Object::is(s, &target));
                    if same {
                        e.sentinel = None;
                    }
                    same
                })
                .unwrap_or(false);
                if ours {
                    applier.apply(|s| apply_held(s, false));
                    // 可視復帰が release 通知より先に処理された場合の取り直し。
                    try_reacquire(&applier, id);
                }
            })
        };
        if let Some(t) = sentinel.dyn_ref::<web_sys::EventTarget>() {
            let _ =
                t.add_event_listener_with_callback("release", on_release.as_ref().unchecked_ref());
        }
        // 取得時点で解放済みの sentinel は保持しない（保持中扱いで取り直しが止まるため）。
        let is_released = released(&sentinel);
        with(id, |e| {
            detach_listener(e);
            if is_released {
                // 失敗として確定する初回・利用者操作では要求も取り消す（画面側 desired と揃える）。
                if !retry {
                    e.desired = false;
                }
                if let Some(t) = sentinel.dyn_ref::<web_sys::EventTarget>() {
                    let _ = t.remove_event_listener_with_callback(
                        "release",
                        on_release.as_ref().unchecked_ref(),
                    );
                }
            } else {
                e.listener = Some((sentinel.clone(), on_release));
                e.sentinel = Some(sentinel);
            }
        });
        acquire_outcome(true, Some(is_released))
    }

    /// 解除して `(結果, held)` を返す。失敗時は sentinel を戻し、効いているものを隠さない。
    async fn release(id: u64) -> Outcome {
        let taken = with(id, |e| {
            e.desired = false;
            e.epoch += 1;
            e.sentinel.take()
        })
        .flatten();
        let Some(sentinel) = taken else {
            return (Ok(()), false);
        };
        let ok = match call_promise(&sentinel, "release", None) {
            Ok(p) => JsFuture::from(p).await.is_ok(),
            Err(_) => false,
        };
        if ok {
            return (Ok(()), false);
        }
        let held = !released(&sentinel);
        if held {
            // 画面側 `finish` が失敗時に desired=true を復元するのと揃える。
            with(id, |e| {
                e.sentinel = Some(sentinel);
                e.desired = true;
            });
        }
        (Err(WakeLockFailure::Failed(REJECTED.to_string())), held)
    }

    /// 可視復帰時の取り直し。busy 中なら記録だけし、完了側が `try_reacquire` を呼び直す。
    fn try_reacquire(applier: &Applier<State>, id: u64) {
        let hidden = web_sys::window()
            .and_then(|w| w.document())
            .is_none_or(|d| d.hidden());
        let go = with(id, |e| {
            if defer_visible(hidden, e.busy) {
                e.revisit = true;
            }
            let go = should_reacquire(
                hidden,
                e.desired,
                // 解放済み（release 通知が未着）の sentinel は保持中と数えない。
                e.sentinel.as_ref().is_some_and(|s| !released(s)),
                e.busy,
            );
            if go {
                e.busy = true;
            }
            go
        })
        .unwrap_or(false);
        if !go {
            return;
        }
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let (res, held) = acquire(&applier, id, true).await;
            let again = with(id, |e| {
                e.busy = false;
                std::mem::take(&mut e.revisit)
            })
            .unwrap_or(false);
            // 失敗は PENDING 表示のまま（嘘の成功にしない）。再描画のため常に反映する。
            applier.apply(move |s| apply_held(s, res.is_ok() && held));
            if again {
                try_reacquire(&applier, id);
            }
        });
    }

    /// `document` の `visibilitychange` 購読を 1 マウントにつき 1 度だけ登録する。
    fn install_visibility(applier: &Applier<State>, id: u64) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if with(id, |e| e.on_visible.is_some()).unwrap_or(true) {
            return;
        }
        let tick = applier.clone();
        let cb = Closure::<dyn FnMut()>::new(move || try_reacquire(&tick, id));
        let _ =
            doc.add_event_listener_with_callback("visibilitychange", cb.as_ref().unchecked_ref());
        with(id, |e| e.on_visible = Some(cb));
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない（再描画ループ防止）。
    pub fn wake_lock_toggle_effect(state: &State, applier: &Applier<State>) {
        let Some(req) = take_request(state) else {
            return;
        };
        let id = applier.mount_id();
        LOCKS.with(|l| {
            l.borrow_mut().entry(id).or_default();
        });
        install_visibility(applier, id);
        if !with(id, |e| !std::mem::replace(&mut e.busy, true)).unwrap_or(false) {
            return;
        }
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            applier.apply(mark_applying);
            let (res, held) = if req.keep {
                acquire(&applier, id, false).await
            } else {
                release(id).await
            };
            // 状態を確定させてから結果を届ける（契約表 2）。
            let again = with(id, |e| {
                e.busy = false;
                std::mem::take(&mut e.revisit)
            })
            .unwrap_or(false);
            applier.apply(move |s| finish(s, req.generation, res, held));
            if again {
                try_reacquire(&applier, id);
            }
        });
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(mut entry) = LOCKS.with(|l| l.borrow_mut().remove(&mount_id)) else {
            return;
        };
        detach_listener(&mut entry);
        if let (Some(cb), Some(doc)) = (
            entry.on_visible.take(),
            web_sys::window().and_then(|w| w.document()),
        ) {
            let _ = doc.remove_event_listener_with_callback(
                "visibilitychange",
                cb.as_ref().unchecked_ref(),
            );
        }
        if let Some(s) = entry.sentinel.take() {
            let _ = call_promise(&s, "release", None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_never_fakes_success() {
        assert_eq!(
            acquire_outcome(false, Some(false)),
            (Err(WakeLockFailure::Unsupported), false)
        );
        assert_eq!(
            acquire_outcome(true, None),
            (Err(WakeLockFailure::Failed(REJECTED.to_string())), false)
        );
        assert_eq!(acquire_outcome(true, Some(true)), (Ok(()), false));
        assert_eq!(acquire_outcome(true, Some(false)), (Ok(()), true));
    }

    #[test]
    fn reacquire_only_when_visible_wanted_and_idle() {
        assert!(should_reacquire(false, true, false, false));
        assert!(!should_reacquire(true, true, false, false));
        assert!(!should_reacquire(false, false, false, false));
        assert!(!should_reacquire(false, true, true, false));
        assert!(!should_reacquire(false, true, false, true));
    }

    #[test]
    fn released_sentinel_does_not_block_reacquire() {
        // visibilitychange が release 通知より先に届いた順序: 解放済みなら holding=false で取り直す。
        let holding = |released: bool| !released;
        assert!(should_reacquire(false, true, holding(true), false));
        assert!(!should_reacquire(false, true, holding(false), false));
    }

    #[test]
    fn visible_during_busy_is_deferred() {
        assert!(defer_visible(false, true));
        assert!(!defer_visible(true, true));
        assert!(!defer_visible(false, false));
    }

    #[test]
    fn stale_results_are_dropped() {
        assert!(is_current(true, 3, 3, true));
        assert!(!is_current(false, 3, 3, true));
        assert!(!is_current(true, 4, 3, true));
        assert!(!is_current(true, 3, 3, false));
    }
}
