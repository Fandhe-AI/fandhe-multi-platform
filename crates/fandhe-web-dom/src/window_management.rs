//! 型サンプル「ウィンドウ管理（位置・サイズ・最前面）」の Web 経路（親 #127、Issue #638）。
//!
//! `fandhe_core::screens::window_management_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。`take_request` で要求を観測し
//! `window.moveTo` / `window.resizeTo` へ渡し、`finish` で結果を返す。`fandhe-device` の trait 実装は
//! 作らず、型と [`fandhe_device::window_management::reached`] だけを共有する（他の Web モジュールと同じ。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外）。契約の SSOT は
//! `fandhe_device::window_management`、駆動の手本は `fandhe-egui-proxy` の同名 effect ドライバ。
//!
//! # 方式
//!
//! - 依頼は `moveTo`（作業領域の左上）と `resizeTo`（外枠の寸法。目標の内寸に `outerWidth - innerWidth`
//!   の差分を足す）。完了は待たず、`screenX` / `screenY` / `innerWidth` 等を読み戻して目標と
//!   `TOLERANCE` 内で一致したときだけ成功にする
//! - 期限は依頼直前の `performance.now()`（単調時計）から `CONFIRM_TIMEOUT_SECS` 秒。期限の判定を
//!   一致判定より先に行い、期限後の一致は失敗で確定する
//! - 全画面中（`fullscreenElement` / `display-mode: fullscreen`）、または全画面かどうか読めないときは
//!   依頼を出さない
//! - ブラウザのタブ（`display-mode: browser`）では移動・サイズ変更も非対応として表示する
//! - 最前面は Web に API が無く常に非対応（`always_on_top = false`）
//! - 非表示中（最小化・背景）は位置と大きさを読めない値として扱い、成功にしない
//! - bfcache 中はタイマーが凍結する。保留中の要求は破棄せず、復帰後の最初の tick で期限を判定する
//!   （期限を過ぎていれば失敗で確定する）
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 表示は読み戻した値に従う、非対応・タブ・全画面・読めない状態・期限切れを成功と見せない、
//! 二重発送しない、結果を届ける前に状態を確定する、破棄時に `clearTimeout` してから closure を drop する。
//! 例外の `message`・座標・寸法はログにも画面にも出さない（security.md A09）。
//! 守らない: ページの拡大率が 100% 以外のときの期限切れ（失敗になるだけで成功とは見せない）、
//! Safari・Firefox、複数タブ・複数マウント間の表示の食い違い、外部変化の反映遅れ（最大 1 秒）、
//! ページ終了時の後片付け（ブラウザが戻す）。

use fandhe_core::screens::window_management_show::{
    WindowAction, WindowFailure, WindowObserved, WindowSupportView, CONFIRM_TIMEOUT_SECS,
    DEFAULT_INNER_SIZE, NOT_APPLIED_REASON, UNREADABLE_REASON,
};
use fandhe_device::window_management::{
    reached, WindowFrame, WindowOp, WindowSnapshot, WindowSupport,
};

/// 期限（ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CONFIRM_TIMEOUT_MS: f64 = CONFIRM_TIMEOUT_SECS as f64 * 1000.0;
/// 確認待ち中の読み直し間隔（ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CONFIRM_POLL_MS: i32 = 100;
/// 平常時に外部変化（ドラッグ等）を読み直す間隔（ミリ秒）。契約の「反映遅れ最大 1 秒」。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const IDLE_REFRESH_MS: i32 = 1000;
/// `moveTo` / `resizeTo` が例外を投げたときの固定文言（例外の `message` は転送しない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const REQUEST_FAILED: &str = "ブラウザがウィンドウの変更を受け付けませんでした";
/// i32 へ渡す前の値域上限（CSS px）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const MAX_COORD: f64 = 1_000_000.0;

/// `window` から読んだ生値。読めない項目は `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RawWindow {
    pub(crate) screen_x: Option<f64>,
    pub(crate) screen_y: Option<f64>,
    pub(crate) outer_w: Option<f64>,
    pub(crate) outer_h: Option<f64>,
    pub(crate) inner_w: Option<f64>,
    pub(crate) inner_h: Option<f64>,
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn rect(x: Option<f64>, y: Option<f64>, w: Option<f64>, h: Option<f64>) -> Option<WindowFrame> {
    let (x, y, w, h) = (x?, y?, w?, h?);
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(WindowFrame {
        x,
        y,
        width: w,
        height: h,
    })
}

/// 提供できる操作と、ブラウザのタブか。`display_mode_browser` が `None`（`matchMedia` が読めない）は
/// 非対応として扱い、タブとも断定しない。最前面は常に非対応。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn support_from(
    display_mode_browser: Option<bool>,
    has_move: bool,
    has_resize: bool,
) -> (WindowSupport, bool) {
    let in_tab = display_mode_browser == Some(true);
    let move_resize = display_mode_browser == Some(false) && has_move && has_resize;
    (
        WindowSupport {
            move_resize,
            always_on_top: false,
        },
        in_tab,
    )
}

/// 画面へ渡す写し。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn support_view(support: WindowSupport, in_tab: bool) -> WindowSupportView {
    WindowSupportView {
        move_resize: Some(support.move_resize),
        always_on_top: Some(support.always_on_top),
        in_browser_tab: in_tab,
    }
}

/// 全画面か。どちらかが真なら真、両方が偽のときだけ偽、それ以外は読めない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn fullscreen_from(
    element: Option<bool>,
    display_mode_fullscreen: Option<bool>,
) -> Option<bool> {
    match (element, display_mode_fullscreen) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// 生値から [`WindowSnapshot`] を作る。非表示中（読めない場合を含む）は外枠・内側とも `None`。
/// 最前面は読む API が無いため常に `None`（表示は `support` 側が決める）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn snapshot_from(
    raw: RawWindow,
    hidden: Option<bool>,
    fullscreen: Option<bool>,
) -> WindowSnapshot {
    let (outer, inner) = if hidden == Some(false) {
        (
            rect(raw.screen_x, raw.screen_y, raw.outer_w, raw.outer_h),
            // 判定には幅と高さだけを使う。位置は外枠と同じ値を置く。
            rect(raw.screen_x, raw.screen_y, raw.inner_w, raw.inner_h),
        )
    } else {
        (None, None)
    };
    WindowSnapshot {
        outer,
        inner,
        always_on_top: None,
        fullscreen,
    }
}

/// `screen.availLeft` 等から作業領域を作る。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn work_area_from(
    left: Option<f64>,
    top: Option<f64>,
    w: Option<f64>,
    h: Option<f64>,
) -> Option<WindowFrame> {
    rect(left, top, w, h)
}

/// 有限で `MAX_COORD` 以内のときだけ丸めて i32 にする。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_i32(v: f64) -> Option<i32> {
    (v.is_finite() && v.abs() <= MAX_COORD).then(|| v.round() as i32)
}

/// `resizeTo` に渡す外枠の寸法。目標の内寸に `outer - inner` の差分を足す。
/// 差分が負・非有限、結果が `(0, MAX_COORD]` 外なら `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outer_for_inner(
    want_w: f64,
    want_h: f64,
    outer: Option<WindowFrame>,
    inner: Option<WindowFrame>,
) -> Option<(i32, i32)> {
    let (o, i) = (outer?, inner?);
    let (dw, dh) = (o.width - i.width, o.height - i.height);
    if !(dw.is_finite() && dh.is_finite()) || dw < 0.0 || dh < 0.0 {
        return None;
    }
    let (w, h) = (want_w + dw, want_h + dh);
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    Some((to_i32(w)?, to_i32(h)?))
}

/// 読み戻した値を画面の観測値へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn observed_from(s: &WindowSnapshot) -> WindowObserved {
    WindowObserved {
        position: s.outer.map(|f| (f.x, f.y)),
        inner_size: s.inner.map(|f| (f.width, f.height)),
        always_on_top: s.always_on_top,
        fullscreen: s.fullscreen,
    }
}

/// 画面の操作を依頼する [`WindowOp`] へ変換する。失敗は依頼を出さない理由。
/// 判定順は 非対応 → 全画面 → 作業領域（`fandhe-egui-proxy` と同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn plan(
    action: WindowAction,
    support: WindowSupport,
    snap: &WindowSnapshot,
    work_area: Option<WindowFrame>,
) -> Result<WindowOp, WindowFailure> {
    let supported = match action {
        WindowAction::SetAlwaysOnTop(_) => support.always_on_top,
        _ => support.move_resize,
    };
    if !supported {
        return Err(WindowFailure::Unsupported);
    }
    match snap.fullscreen {
        Some(true) => return Err(WindowFailure::FullscreenRefused),
        Some(false) => {}
        None => return Err(WindowFailure::Failed(UNREADABLE_REASON.to_string())),
    }
    match action {
        WindowAction::MoveTopLeft => match work_area {
            Some(a) => Ok(WindowOp::MoveTo { x: a.x, y: a.y }),
            None => Err(WindowFailure::Failed(UNREADABLE_REASON.to_string())),
        },
        WindowAction::ResetSize => Ok(WindowOp::ResizeInner {
            width: f64::from(DEFAULT_INNER_SIZE[0]),
            height: f64::from(DEFAULT_INNER_SIZE[1]),
        }),
        WindowAction::SetAlwaysOnTop(on) => Ok(WindowOp::SetAlwaysOnTop(on)),
    }
}

/// 確認待ちの 1 ステップ。期限を先に判定し、次に目標への到達を見る。`None` は続行。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn confirm_step(
    elapsed_ms: f64,
    op: &WindowOp,
    snap: &WindowSnapshot,
) -> Option<Result<(), WindowFailure>> {
    if elapsed_ms >= CONFIRM_TIMEOUT_MS {
        return Some(Err(WindowFailure::Failed(NOT_APPLIED_REASON.to_string())));
    }
    reached(op, snap).then_some(Ok(()))
}

#[cfg(target_arch = "wasm32")]
pub use web::{web_initial_state, window_management_show_effect};

/// マウント破棄時にタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::window_management_show::{
        apply_observed, apply_support, finish, mark_applying, take_request, WindowManagementState,
    };
    use js_sys::{Function, Reflect};
    use wasm_bindgen::prelude::*;

    use super::*;
    use crate::mount::{is_live, Applier};

    type State = WindowManagementState;

    struct Pending {
        generation: u64,
        op: WindowOp,
        began_ms: f64,
    }

    #[derive(Default)]
    struct Entry {
        /// 発送済みの世代（同じ世代を二度渡さない）。
        started: Option<u64>,
        pending: Option<Pending>,
        /// 次回の tick を呼ぶ closure（使い回す）。
        tick_cb: Option<Closure<dyn FnMut()>>,
        /// 予約中の `setTimeout` のハンドル。
        timer: Option<i32>,
        last_observed: Option<WindowObserved>,
        last_support: Option<WindowSupportView>,
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

    fn num(o: &JsValue, key: &str) -> Option<f64> {
        get(o, key).as_f64()
    }

    fn now_ms() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0)
    }

    fn media_matches(query: &str) -> Option<bool> {
        let list = web_sys::window()?.match_media(query).ok()??;
        Some(list.matches())
    }

    fn read_fullscreen() -> Option<bool> {
        let win = web_sys::window()?;
        let doc = win.document()?;
        let v = get(&doc, "fullscreenElement");
        let element = if v.is_null() {
            Some(false)
        } else if v.is_object() {
            Some(true)
        } else {
            None
        };
        fullscreen_from(element, media_matches("(display-mode: fullscreen)"))
    }

    fn read_snapshot() -> WindowSnapshot {
        let Some(win) = web_sys::window() else {
            return WindowSnapshot::default();
        };
        let w = JsValue::from(win.clone());
        let raw = RawWindow {
            screen_x: num(&w, "screenX"),
            screen_y: num(&w, "screenY"),
            outer_w: num(&w, "outerWidth"),
            outer_h: num(&w, "outerHeight"),
            inner_w: num(&w, "innerWidth"),
            inner_h: num(&w, "innerHeight"),
        };
        let hidden = win
            .document()
            .and_then(|d| get(&JsValue::from(d), "hidden").as_bool());
        snapshot_from(raw, hidden, read_fullscreen())
    }

    fn read_support() -> (WindowSupport, bool) {
        let Some(win) = web_sys::window() else {
            return support_from(None, false, false);
        };
        let w = JsValue::from(win);
        support_from(
            media_matches("(display-mode: browser)"),
            get(&w, "moveTo").is_function(),
            get(&w, "resizeTo").is_function(),
        )
    }

    fn read_work_area() -> Option<WindowFrame> {
        let win = web_sys::window()?;
        let s = JsValue::from(win.screen().ok()?);
        work_area_from(
            num(&s, "availLeft"),
            num(&s, "availTop"),
            num(&s, "availWidth"),
            num(&s, "availHeight"),
        )
    }

    fn read_support_view() -> WindowSupportView {
        let (support, in_tab) = read_support();
        support_view(support, in_tab)
    }

    /// `demo.rs` 用。マウント時の support と observed を入れた初期状態
    /// （`mount` はボタン構成を組み直さないため、読んだ値で画面を作る）。
    pub fn web_initial_state() -> State {
        State {
            support: read_support_view(),
            observed: observed_from(&read_snapshot()),
            ..State::default()
        }
    }

    /// 既存の予約を消して `ms` 後に tick を予約する。予約に失敗したら予約なしで続ける
    /// （表示が追従しないだけで、嘘の成功にはならない）。
    fn schedule(id: u64, ms: i32) {
        let Some(win) = web_sys::window() else {
            return;
        };
        with(id, |e| {
            if let Some(h) = e.timer.take() {
                win.clear_timeout_with_handle(h);
            }
            if let Some(cb) = e.tick_cb.as_ref() {
                let f: &Function = cb.as_ref().unchecked_ref();
                e.timer = win
                    .set_timeout_with_callback_and_timeout_and_arguments_0(f, ms)
                    .ok();
            }
        });
    }

    /// 保留中の要求を確定する。先に Entry を確定し、そのあと結果を届ける（契約表 2）。
    fn complete(
        applier: &Applier<State>,
        id: u64,
        result: Result<(), WindowFailure>,
        observed: WindowObserved,
    ) {
        let Some(Some(p)) = with(id, |e| {
            e.last_observed = Some(observed);
            e.pending.take()
        }) else {
            return;
        };
        let gen = p.generation;
        applier.apply(move |s| {
            mark_applying(s);
            finish(s, gen, result, observed);
        });
    }

    fn tick(applier: &Applier<State>, id: u64) {
        if !is_live(id) {
            return;
        }
        let snap = read_snapshot();
        let observed = observed_from(&snap);
        let pending = with(id, |e| {
            e.pending.as_ref().map(|p| (p.op, now_ms() - p.began_ms))
        })
        .flatten();
        if let Some((op, elapsed)) = pending {
            if let Some(result) = confirm_step(elapsed, &op, &snap) {
                complete(applier, id, result, observed);
            }
        } else {
            let support = read_support_view();
            let (obs_changed, sup_changed) = with(id, |e| {
                let o = e.last_observed != Some(observed);
                let s = e.last_support != Some(support);
                e.last_observed = Some(observed);
                e.last_support = Some(support);
                (o, s)
            })
            .unwrap_or((false, false));
            if obs_changed {
                applier.apply(move |s| apply_observed(s, observed));
            }
            if sup_changed {
                applier.apply(move |s| apply_support(s, support));
            }
        }
        // 保留中が残っていれば短い間隔、なければ平常間隔で次回を予約する。
        let still_pending = with(id, |e| e.pending.is_some()).unwrap_or(false);
        schedule(
            id,
            if still_pending {
                CONFIRM_POLL_MS
            } else {
                IDLE_REFRESH_MS
            },
        );
    }

    /// マウントごとに 1 度だけ Entry と tick を用意し、初期値を反映する。
    fn install(applier: &Applier<State>, id: u64) {
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
        let cb = Closure::<dyn FnMut()>::new(move || tick(&a, id));
        let support = read_support_view();
        let observed = observed_from(&read_snapshot());
        with(id, |e| {
            e.tick_cb = Some(cb);
            e.last_support = Some(support);
            e.last_observed = Some(observed);
        });
        schedule(id, IDLE_REFRESH_MS);
        applier.apply(move |s| {
            apply_support(s, support);
            apply_observed(s, observed);
        });
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない（再描画ループ防止）。
    pub fn window_management_show_effect(state: &State, applier: &Applier<State>) {
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
        let snap = read_snapshot();
        let observed = observed_from(&snap);
        let (support, _) = read_support();
        let reject = |failure: WindowFailure| {
            applier.apply(move |s| {
                mark_applying(s);
                finish(s, gen, Err(failure), observed);
            });
        };
        let op = match plan(req.action, support, &snap, read_work_area()) {
            Ok(op) => op,
            Err(f) => return reject(f),
        };
        // 引数を検証してから丸める。検証に落ちたら依頼を出さない。
        enum Call {
            Move(i32, i32),
            Resize(i32, i32),
        }
        let call = match op {
            WindowOp::MoveTo { x, y } => to_i32(x).zip(to_i32(y)).map(|(x, y)| Call::Move(x, y)),
            WindowOp::ResizeInner { width, height } => {
                outer_for_inner(width, height, snap.outer, snap.inner)
                    .map(|(w, h)| Call::Resize(w, h))
            }
            // plan が最前面を Unsupported にするため到達しない。
            WindowOp::SetAlwaysOnTop(_) => None,
        };
        let Some(call) = call else {
            return reject(WindowFailure::Failed(UNREADABLE_REASON.to_string()));
        };
        let Some(win) = web_sys::window() else {
            return reject(WindowFailure::Unsupported);
        };
        // 期限の起点は依頼直前。先に保留を登録し、以降の全経路を complete で一度だけ確定する。
        with(id, |e| {
            e.pending = Some(Pending {
                generation: gen,
                op,
                began_ms: now_ms(),
            })
        });
        let res = match call {
            Call::Move(x, y) => win.move_to(x, y),
            Call::Resize(w, h) => win.resize_to(w, h),
        };
        if res.is_err() {
            complete(
                applier,
                id,
                Err(WindowFailure::Failed(REQUEST_FAILED.to_string())),
                observed,
            );
            return;
        }
        applier.apply(mark_applying);
        schedule(id, CONFIRM_POLL_MS);
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(mut entry) = ENTRIES.with(|e| e.borrow_mut().remove(&mount_id)) else {
            return;
        };
        // 保留中の結果は送らない（画面ごと破棄される）。clearTimeout してから closure を drop する。
        if let (Some(h), Some(w)) = (entry.timer.take(), web_sys::window()) {
            w.clear_timeout_with_handle(h);
        }
        drop(entry.tick_cb.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, y: f64, w: f64, h: f64) -> WindowFrame {
        WindowFrame {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn sup(move_resize: bool) -> WindowSupport {
        WindowSupport {
            move_resize,
            always_on_top: false,
        }
    }

    fn raw() -> RawWindow {
        RawWindow {
            screen_x: Some(10.0),
            screen_y: Some(20.0),
            outer_w: Some(500.0),
            outer_h: Some(420.0),
            inner_w: Some(480.0),
            inner_h: Some(360.0),
        }
    }

    #[test]
    fn support_three_branches() {
        assert_eq!(support_from(Some(true), true, true), (sup(false), true));
        assert_eq!(support_from(Some(false), true, true), (sup(true), false));
        assert_eq!(support_from(Some(false), true, false), (sup(false), false));
        assert_eq!(support_from(None, true, true), (sup(false), false));
    }

    #[test]
    fn fullscreen_truth_table_keeps_unknown() {
        assert_eq!(fullscreen_from(Some(true), None), Some(true));
        assert_eq!(fullscreen_from(None, Some(true)), Some(true));
        assert_eq!(fullscreen_from(Some(false), Some(false)), Some(false));
        assert_eq!(fullscreen_from(Some(false), None), None);
        assert_eq!(fullscreen_from(None, Some(false)), None);
        assert_eq!(fullscreen_from(None, None), None);
    }

    #[test]
    fn snapshot_reads_frames_when_visible() {
        let s = snapshot_from(raw(), Some(false), Some(false));
        assert_eq!(s.outer, Some(frame(10.0, 20.0, 500.0, 420.0)));
        assert_eq!(s.inner.map(|f| (f.width, f.height)), Some((480.0, 360.0)));
        assert_eq!(s.always_on_top, None);
        assert_eq!(s.fullscreen, Some(false));
    }

    #[test]
    fn snapshot_is_unreadable_when_hidden_or_invalid() {
        for hidden in [Some(true), None] {
            let s = snapshot_from(raw(), hidden, Some(false));
            assert!(s.outer.is_none() && s.inner.is_none());
        }
        let mut r = raw();
        r.outer_w = Some(0.0);
        r.inner_h = Some(f64::NAN);
        let s = snapshot_from(r, Some(false), Some(false));
        assert!(s.outer.is_none() && s.inner.is_none());
        let mut r = raw();
        r.screen_x = None;
        assert!(snapshot_from(r, Some(false), None).outer.is_none());
    }

    #[test]
    fn work_area_rejects_invalid() {
        assert_eq!(
            work_area_from(Some(0.0), Some(25.0), Some(1440.0), Some(875.0)),
            Some(frame(0.0, 25.0, 1440.0, 875.0))
        );
        assert!(work_area_from(Some(0.0), Some(0.0), Some(0.0), Some(10.0)).is_none());
        assert!(work_area_from(Some(f64::INFINITY), Some(0.0), Some(1.0), Some(1.0)).is_none());
        assert!(work_area_from(None, Some(0.0), Some(1.0), Some(1.0)).is_none());
    }

    #[test]
    fn to_i32_range() {
        assert_eq!(to_i32(12.6), Some(13));
        assert_eq!(to_i32(-3.2), Some(-3));
        assert_eq!(to_i32(f64::NAN), None);
        assert_eq!(to_i32(MAX_COORD + 1.0), None);
    }

    #[test]
    fn outer_for_inner_adds_chrome_difference() {
        let outer = Some(frame(0.0, 0.0, 500.0, 420.0));
        let inner = Some(frame(0.0, 0.0, 480.0, 360.0));
        assert_eq!(
            outer_for_inner(480.0, 360.0, outer, inner),
            Some((500, 420))
        );
        assert_eq!(
            outer_for_inner(300.0, 200.0, outer, inner),
            Some((320, 260))
        );
        // 差分が負（outer < inner）は依頼しない。
        let bad = Some(frame(0.0, 0.0, 400.0, 300.0));
        assert_eq!(outer_for_inner(480.0, 360.0, bad, inner), None);
        assert_eq!(outer_for_inner(480.0, 360.0, None, inner), None);
        assert_eq!(outer_for_inner(f64::NAN, 360.0, outer, inner), None);
        assert_eq!(outer_for_inner(1e9, 360.0, outer, inner), None);
    }

    #[test]
    fn plan_branches() {
        let snap = |fs| WindowSnapshot {
            fullscreen: fs,
            ..Default::default()
        };
        let area = Some(frame(0.0, 25.0, 1440.0, 875.0));
        assert_eq!(
            plan(
                WindowAction::MoveTopLeft,
                sup(true),
                &snap(Some(false)),
                area
            ),
            Ok(WindowOp::MoveTo { x: 0.0, y: 25.0 })
        );
        assert_eq!(
            plan(WindowAction::ResetSize, sup(true), &snap(Some(false)), None),
            Ok(WindowOp::ResizeInner {
                width: 480.0,
                height: 360.0
            })
        );
        // 最前面は常に非対応。
        assert_eq!(
            plan(
                WindowAction::SetAlwaysOnTop(true),
                sup(true),
                &snap(Some(false)),
                area
            ),
            Err(WindowFailure::Unsupported)
        );
        assert_eq!(
            plan(
                WindowAction::MoveTopLeft,
                sup(false),
                &snap(Some(false)),
                area
            ),
            Err(WindowFailure::Unsupported)
        );
        // 全画面中・読めないときは依頼を出さない。
        assert_eq!(
            plan(WindowAction::ResetSize, sup(true), &snap(Some(true)), area),
            Err(WindowFailure::FullscreenRefused)
        );
        assert_eq!(
            plan(WindowAction::ResetSize, sup(true), &snap(None), area),
            Err(WindowFailure::Failed(UNREADABLE_REASON.to_string()))
        );
        // 作業領域が読めない移動。
        assert_eq!(
            plan(
                WindowAction::MoveTopLeft,
                sup(true),
                &snap(Some(false)),
                None
            ),
            Err(WindowFailure::Failed(UNREADABLE_REASON.to_string()))
        );
    }

    #[test]
    fn confirm_step_checks_deadline_before_match() {
        let op = WindowOp::MoveTo { x: 10.0, y: 20.0 };
        let snap = snapshot_from(raw(), Some(false), Some(false));
        assert_eq!(confirm_step(100.0, &op, &snap), Some(Ok(())));
        assert_eq!(
            confirm_step(100.0, &WindowOp::MoveTo { x: 99.0, y: 0.0 }, &snap),
            None
        );
        // 期限後に一致しても失敗。
        assert!(matches!(
            confirm_step(CONFIRM_TIMEOUT_MS, &op, &snap),
            Some(Err(WindowFailure::Failed(_)))
        ));
        // 読めない値は一致しない。
        let hidden = snapshot_from(raw(), Some(true), Some(false));
        assert_eq!(confirm_step(100.0, &op, &hidden), None);
    }

    #[test]
    fn observed_maps_snapshot() {
        let snap = snapshot_from(raw(), Some(false), Some(false));
        let o = observed_from(&snap);
        assert_eq!(o.position, Some((10.0, 20.0)));
        assert_eq!(o.inner_size, Some((480.0, 360.0)));
        assert_eq!(o.always_on_top, None);
        assert_eq!(o.fullscreen, Some(false));
        assert_eq!(
            observed_from(&WindowSnapshot::default()),
            WindowObserved::default()
        );
    }
}
