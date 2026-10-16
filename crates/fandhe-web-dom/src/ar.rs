//! 型サンプル「AR 表示」の Web 経路（親 #90、Issue #309）。
//!
//! `fandhe_core::screens::ar_show`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`]
//! の effect として駆動する。`fandhe-egui-proxy` の wasm canvas 経路は対象外
//! （`UnsupportedAr` のまま）。`fandhe_device::ar` から `READ_INTERVAL`・`checked_reading` を、
//! `fandhe_device::compass` から `WaitTracker`・`NO_READING_TIMEOUT_MS` を共用する。
//! `ArTracking` trait（`Send + Sync`）は JS の Closure を持てないため通さず、core の
//! `apply_reading` / `apply_failure` / `apply_ended` へ直接流す。
//!
//! # 方式（WebXR `immersive-ar`）
//!
//! - egui の canvas とは結ばない。マウント先の DOM を `dom-overlay` の root としてカメラ映像の
//!   上に重ねる。XR 用の WebGL は DOM に挿さない専用の `<canvas>` に作り、描画はしない
//!   （UA が透明な層として合成する）。要求する機能は `dom-overlay`（optional）だけ
//! - `requestSession` は「開始」押下の同期区間（transient activation）で呼ぶ
//! - XR の rAF は最新の姿勢を書くだけ。画面への反映は `READ_INTERVAL` の `setInterval` だけが
//!   行う（書き手 1 本）。非表示中は値を捨て、値なしが `NO_READING_TIMEOUT_MS` 続けば
//!   `NoReading`（単調時計 `performance.now()`）
//! - 水平面は数えない（`horizontal_planes` は常に `None`。0 枚と出さない）
//! - `NotSupportedError` は利用者の拒否でも起きるため `isSessionSupported` で分ける
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: `NotAvailable` の位置・0 枚という嘘の値を出さない、1 マウントで二重に開始しない、
//! 停止・失敗・期限切れ・マウント破棄の全経路でセッション（カメラ）を終える、
//! 待ち中に起きた停止・破棄で届いたセッションを即終える、古い世代の完了の破棄、
//! ブラウザ側の終了（戻る操作）は失敗でなく待機へ戻す。
//! 守らない: 許可ダイアログの応答待ちの期限、`NotSupportedError` が拒否か非対応かの
//! 判別の限界、`dom-overlay` が許されない環境（画面が映像の下に隠れる。戻る操作で待機へ戻る）、
//! 姿勢（回転）・平面検出・hit-test、複数タブ・複数マウント、表示の遅れ
//! （最大 `READ_INTERVAL` + フレーム周期）。
//!
//! 位置・例外の message は DOM・ログへ出さない（security.md A09）。

use fandhe_core::screens::ar_show::{
    ArFailureView, ArReadingView, LimitedReasonView, TrackingView,
};
use fandhe_device::ar::{checked_reading, ArFailure, ArReading, LimitedReason, Tracking};
use fandhe_device::compass::WaitTracker;

use super::compass::STALE_MS;

/// `getViewerPose` の結果から状態を作る。`None`（姿勢なし）は追跡不能、`emulatedPosition` は制限付き。
/// 位置の検証は `checked_reading` に任せ、不正なら値なし。水平面は数えない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from_pose(pose: Option<([f64; 3], bool)>) -> Option<ArReading> {
    match pose {
        None => checked_reading(Tracking::NotAvailable, [0.0; 3], None),
        Some((p, true)) => checked_reading(Tracking::Limited(LimitedReason::Other), p, None),
        Some((p, false)) => checked_reading(Tracking::Normal, p, None),
    }
}

/// `requestSession` の reject を失敗種別へ。`supported` は `isSessionSupported('immersive-ar')`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn classify_request_error(name: &str, supported: Option<bool>) -> ArFailure {
    match (name, supported) {
        ("SecurityError", _) => ArFailure::PermissionDenied,
        // WebXR では利用者の拒否も NotSupportedError になる。判定不能は非対応側（偽の拒否にしない）。
        ("NotSupportedError", Some(true)) => ArFailure::PermissionDenied,
        ("NotSupportedError", _) => ArFailure::Unsupported,
        _ => ArFailure::Failed,
    }
}

/// 1 回の tick の判定結果。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, PartialEq)]
pub(crate) enum Tick {
    /// 画面へ送る値（`None` は値なし）。
    Send(Option<ArReading>),
    /// 値なしが期限を超えた。
    Expired,
}

/// 保持値: 状態と受信時刻（`performance.now()` の ms）。
pub(crate) type Stamped = std::cell::Cell<Option<(ArReading, f64)>>;

/// 非表示中は保持値と期限の起点を捨てる。受信から `STALE_MS` を超えた値も捨てる。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn tick(hidden: bool, latest: &Stamped, tracker: &mut WaitTracker, now_ms: f64) -> Tick {
    if hidden {
        latest.set(None);
        tracker.reset();
        return Tick::Send(None);
    }
    let fresh = latest.get().filter(|(_, at)| now_ms - at <= STALE_MS);
    if fresh.is_none() {
        latest.set(None);
    }
    let reading = fresh.map(|(r, _)| r);
    if tracker.observe(reading.is_some(), now_ms) {
        Tick::Expired
    } else {
        Tick::Send(reading)
    }
}

/// セッションの `visibilitychange`。非表示で保持値と起点を捨て、可視復帰の時刻から数え直す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn on_visibility(
    hidden: bool,
    latest: &Stamped,
    tracker: &mut WaitTracker,
    now_ms: f64,
) {
    if hidden {
        latest.set(None);
        tracker.reset();
    } else {
        tracker.begin(now_ms);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn reading_view(r: ArReading) -> ArReadingView {
    ArReadingView {
        tracking: match r.tracking {
            Tracking::NotAvailable => TrackingView::NotAvailable,
            Tracking::Normal => TrackingView::Normal,
            Tracking::Limited(why) => TrackingView::Limited(match why {
                LimitedReason::Initializing => LimitedReasonView::Initializing,
                LimitedReason::ExcessiveMotion => LimitedReasonView::ExcessiveMotion,
                LimitedReason::InsufficientFeatures => LimitedReasonView::InsufficientFeatures,
                LimitedReason::Relocalizing => LimitedReasonView::Relocalizing,
                LimitedReason::Other => LimitedReasonView::Other,
            }),
        },
        position: r.position,
        horizontal_planes: r.horizontal_planes,
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn failure_view(f: ArFailure) -> ArFailureView {
    match f {
        ArFailure::Unsupported => ArFailureView::Unsupported,
        ArFailure::PermissionDenied => ArFailureView::PermissionDenied,
        ArFailure::Failed => ArFailureView::Failed,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::ar_show_effect;

/// マウント破棄時にセッションとタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::ar_show::{
        apply_ended, apply_failure, apply_reading, reading_lines, ArFailureView, ArShowState,
    };
    use fandhe_device::ar::{ArFailure, READ_INTERVAL};
    use fandhe_device::compass::WaitTracker;
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{
        classify_request_error, failure_view, on_visibility, reading_from_pose, reading_view, tick,
        Tick,
    };
    use crate::compass::is_current;
    use crate::mount::{is_live, root_of, Applier};

    type Latest = Rc<super::Stamped>;

    /// マウント単位の状態。Closure は 1 度だけ作り、drop は `release_mount` だけ
    /// （tick の中で失敗を確定すると apply → effect が同期で進むため、実行中の Closure を落とさない）。
    struct Entry {
        /// 動かしている世代。`None` は停止。
        session: Option<u64>,
        /// `XRSession`。登録済みのときだけ `Some`。
        xr: Option<JsValue>,
        ref_space: Option<JsValue>,
        raf: Option<f64>,
        interval: Option<i32>,
        latest: Latest,
        tracker: WaitTracker,
        /// 最後に画面へ送った表示行（外側 `None` は未送信、内側 `None` は値なし）。
        last_sent: Option<Option<[String; 3]>>,
        frame_cb: Closure<dyn FnMut(f64, JsValue)>,
        ticker: Closure<dyn FnMut()>,
        on_end: Closure<dyn FnMut()>,
        on_visibility: Closure<dyn FnMut()>,
    }

    thread_local! {
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    fn key(s: &str) -> JsValue {
        JsValue::from_str(s)
    }

    fn get(o: &JsValue, k: &str) -> JsValue {
        Reflect::get(o, &key(k)).unwrap_or(JsValue::UNDEFINED)
    }

    fn call(o: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(o, method).dyn_into()?;
        f.apply(o, &args.iter().collect::<Array>())
    }

    fn name_of(e: &JsValue) -> String {
        get(e, "name").as_string().unwrap_or_default()
    }

    fn now() -> Option<f64> {
        web_sys::window()?.performance().map(|p| p.now())
    }

    fn doc_hidden() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .map(|d| d.hidden())
            .unwrap_or(false)
    }

    fn session_hidden(xr: &JsValue) -> bool {
        get(xr, "visibilityState").as_string().as_deref() != Some("visible")
    }

    /// 終了の Promise の reject（終了済みの `InvalidStateError` 等）は握りつぶす。
    fn end_session(xr: &JsValue) {
        if let Ok(p) = call(xr, "end", &[]) {
            if let Ok(p) = p.dyn_into::<Promise>() {
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = JsFuture::from(p).await;
                });
            }
        }
    }

    /// 通知・rAF・interval を外し、保持値を捨てる（冪等）。`end` が真ならセッションも終える。
    /// リスナーを先に外すので、自分の `end()` が `apply_ended` を起こさない。
    fn teardown(e: &mut Entry, end: bool) {
        if let Some(xr) = e.xr.take() {
            let t: &web_sys::EventTarget = xr.unchecked_ref();
            let _ = t.remove_event_listener_with_callback("end", e.on_end.as_ref().unchecked_ref());
            let _ = t.remove_event_listener_with_callback(
                "visibilitychange",
                e.on_visibility.as_ref().unchecked_ref(),
            );
            if let Some(h) = e.raf.take() {
                let _ = call(&xr, "cancelAnimationFrame", &[JsValue::from_f64(h)]);
            }
            if end {
                end_session(&xr);
            }
        }
        e.raf = None;
        if let Some(h) = e.interval.take() {
            if let Some(w) = web_sys::window() {
                w.clear_interval_with_handle(h);
            }
        }
        e.ref_space = None;
        e.latest.set(None);
        e.tracker.reset();
        e.last_sent = None;
    }

    fn fail(applier: &Applier<ArShowState>, session: u64, f: ArFailure) {
        let v = failure_view(f);
        applier.apply(move |s| apply_failure(s, session, v));
    }

    fn current(mount_id: u64, session: u64) -> bool {
        ENTRIES.with(|m| {
            m.borrow()
                .get(&mount_id)
                .is_some_and(|e| is_current(is_live(mount_id), e.session, session))
        })
    }

    fn new_entry(mount_id: u64, applier: &Applier<ArShowState>) -> Entry {
        let latest: Latest = Rc::new(Cell::new(None));
        let l = latest.clone();
        let frame_cb = Closure::<dyn FnMut(f64, JsValue)>::new(move |_t: f64, frame: JsValue| {
            ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let Some(e) = m.get_mut(&mount_id) else {
                    return;
                };
                let (Some(xr), Some(rs)) = (e.xr.clone(), e.ref_space.clone()) else {
                    return;
                };
                let pose = call(&frame, "getViewerPose", &[rs]).unwrap_or(JsValue::NULL);
                let parsed = if pose.is_null() || pose.is_undefined() {
                    None
                } else {
                    let pos = get(&get(&pose, "transform"), "position");
                    let n = |k: &str| get(&pos, k).as_f64().unwrap_or(f64::NAN);
                    let emulated = get(&pose, "emulatedPosition").as_bool().unwrap_or(false);
                    Some(([n("x"), n("y"), n("z")], emulated))
                };
                l.set(match (reading_from_pose(parsed), now()) {
                    (Some(r), Some(at)) => Some((r, at)),
                    _ => None,
                });
                e.raf = call(&xr, "requestAnimationFrame", &[e.frame_cb.as_ref().clone()])
                    .ok()
                    .and_then(|h| h.as_f64());
            });
        });
        let tick_applier = applier.clone();
        let ticker = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let now = now();
            // ENTRIES の borrow は apply の前に落とす（apply → effect が再び借りるため）。
            let action = ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let e = m.get_mut(&mount_id)?;
                let session = e.session?;
                let xr = e.xr.clone()?;
                // 単調時計を失ったら期限を数えられない。待ち続けず失敗で確定する。
                let Some(now) = now else {
                    teardown(e, true);
                    return Some((session, Err(ArFailureView::Failed)));
                };
                let hidden = doc_hidden() || session_hidden(&xr);
                match tick(hidden, &e.latest, &mut e.tracker, now) {
                    Tick::Expired => {
                        // 失敗の確定より前にセッションを終える。
                        teardown(e, true);
                        Some((session, Err(ArFailureView::NoReading)))
                    }
                    Tick::Send(r) => {
                        let v = r.map(reading_view);
                        let k = v.as_ref().map(reading_lines);
                        if e.last_sent.as_ref() == Some(&k) {
                            return None;
                        }
                        e.last_sent = Some(k);
                        Some((session, Ok(v)))
                    }
                }
            });
            match action {
                Some((session, Err(v))) => {
                    tick_applier.apply(move |s| apply_failure(s, session, v));
                }
                Some((session, Ok(v))) => {
                    tick_applier.apply(move |s| apply_reading(s, session, v));
                }
                None => {}
            }
        });
        let end_applier = applier.clone();
        let on_end = Closure::<dyn FnMut()>::new(move || {
            // 状態を先に確定し（セッションは既に終了済みなので end() は呼ばない）、そのあと待機へ戻す。
            let session = ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let e = m.get_mut(&mount_id)?;
                let s = e.session.take();
                teardown(e, false);
                s
            });
            if let Some(session) = session {
                end_applier.apply(move |s| apply_ended(s, session));
            }
        });
        let on_vis = Closure::<dyn FnMut()>::new(move || {
            let Some(now) = now() else { return };
            ENTRIES.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&mount_id) {
                    if let Some(xr) = e.xr.clone() {
                        on_visibility(
                            doc_hidden() || session_hidden(&xr),
                            &e.latest,
                            &mut e.tracker,
                            now,
                        );
                    }
                }
            });
        });
        Entry {
            session: None,
            xr: None,
            ref_space: None,
            raf: None,
            interval: None,
            latest,
            tracker: WaitTracker::default(),
            last_sent: None,
            frame_cb,
            ticker,
            on_end,
            on_visibility: on_vis,
        }
    }

    /// `requestSession` を呼ぶ。同期区間で呼ぶこと。前提（secure context・`navigator.xr`）が
    /// 無ければ `Err(Unsupported)`。例外の送出は `Failed`。
    fn request_session(mount_id: u64) -> Result<(Promise, JsValue), ArFailure> {
        let w = web_sys::window().ok_or(ArFailure::Unsupported)?;
        if !w.is_secure_context() {
            return Err(ArFailure::Unsupported);
        }
        let xr = get(&get(&w, "navigator"), "xr");
        if xr.is_undefined() || xr.is_null() {
            return Err(ArFailure::Unsupported);
        }
        let opts = Object::new();
        let _ = Reflect::set(
            &opts,
            &key("optionalFeatures"),
            &Array::of1(&key("dom-overlay")),
        );
        // root が取れなければ domOverlay を付けない（optional なので開始はできる）。
        if let Some(root) = root_of(mount_id) {
            let o = Object::new();
            let _ = Reflect::set(&o, &key("root"), &root);
            let _ = Reflect::set(&opts, &key("domOverlay"), &o);
        }
        let p = call(&xr, "requestSession", &[key("immersive-ar"), opts.into()])
            .map_err(|_| ArFailure::Failed)?;
        let p = p.dyn_into::<Promise>().map_err(|_| ArFailure::Failed)?;
        Ok((p, xr))
    }

    async fn is_supported(xr: &JsValue) -> Option<bool> {
        let p: Promise = call(xr, "isSessionSupported", &[key("immersive-ar")])
            .ok()?
            .dyn_into()
            .ok()?;
        JsFuture::from(p).await.ok()?.as_bool()
    }

    /// 解決したセッションを登録し、描画先・基準空間・rAF・interval を用意する。
    /// 待ちの再開後に世代が外れていたら、後片付けは外した側が済ませているので何もせず `Ok`。
    async fn setup(mount_id: u64, session: u64, xr_session: JsValue) -> Result<(), ArFailure> {
        let w = web_sys::window().ok_or(ArFailure::Failed)?;
        // 登録から保存までの間に届く通知を落とさないため、同じ borrow の中で登録と保存を行う。
        ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.get_mut(&mount_id).ok_or(ArFailure::Failed)?;
            let t: &web_sys::EventTarget = xr_session.unchecked_ref();
            let _ = t.add_event_listener_with_callback("end", e.on_end.as_ref().unchecked_ref());
            let _ = t.add_event_listener_with_callback(
                "visibilitychange",
                e.on_visibility.as_ref().unchecked_ref(),
            );
            e.xr = Some(xr_session.clone());
            Ok::<(), ArFailure>(())
        })?;
        // 描画はしない。XR の要件を満たすためだけの WebGL（DOM には挿さない）。
        let canvas = w
            .document()
            .and_then(|d| d.create_element("canvas").ok())
            .ok_or(ArFailure::Failed)?;
        let attrs = Object::new();
        let _ = Reflect::set(&attrs, &key("xrCompatible"), &JsValue::TRUE);
        let gl = call(&canvas, "getContext", &[key("webgl2"), attrs.into()])
            .map_err(|_| ArFailure::Failed)?;
        if gl.is_null() || gl.is_undefined() {
            return Err(ArFailure::Failed);
        }
        let ctor: Function = get(&w, "XRWebGLLayer")
            .dyn_into()
            .map_err(|_| ArFailure::Failed)?;
        let layer = Reflect::construct(&ctor, &Array::of2(&xr_session, &gl))
            .map_err(|_| ArFailure::Failed)?;
        let state = Object::new();
        let _ = Reflect::set(&state, &key("baseLayer"), &layer);
        call(&xr_session, "updateRenderState", &[state.into()]).map_err(|_| ArFailure::Failed)?;
        let rs_promise: Promise = call(&xr_session, "requestReferenceSpace", &[key("local")])
            .map_err(|_| ArFailure::Failed)?
            .dyn_into()
            .map_err(|_| ArFailure::Failed)?;
        let rs = JsFuture::from(rs_promise)
            .await
            .map_err(|_| ArFailure::Failed)?;
        // 再開後: 停止・破棄・ブラウザ側の終了が先に来ていないか。
        if !current(mount_id, session) {
            return Ok(());
        }
        // 単調時計が無ければ期限を数えられない（0.0 固定で待ち続けない）。
        let t0 = now().ok_or(ArFailure::Failed)?;
        ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.get_mut(&mount_id).ok_or(ArFailure::Failed)?;
            if e.xr.is_none() {
                return Ok(());
            }
            e.ref_space = Some(rs);
            e.raf = call(
                &xr_session,
                "requestAnimationFrame",
                &[e.frame_cb.as_ref().clone()],
            )
            .map_err(|_| ArFailure::Failed)?
            .as_f64();
            e.interval = Some(
                w.set_interval_with_callback_and_timeout_and_arguments_0(
                    e.ticker.as_ref().unchecked_ref(),
                    READ_INTERVAL.as_millis() as i32,
                )
                .map_err(|_| ArFailure::Failed)?,
            );
            // 同意ダイアログの待ち時間は期限に含めない（ここから数える）。
            e.tracker.begin(t0);
            Ok(())
        })
    }

    /// `mount_with_effect` へ渡す effect。再描画のたびに呼ばれ、世代が変わったときだけ動く。
    pub fn ar_show_effect(state: &ArShowState, applier: &Applier<ArShowState>) {
        let mount_id = applier.mount_id();
        let session = state.session;
        let wanted = state.wanted;
        let start = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m
                .entry(mount_id)
                .or_insert_with(|| new_entry(mount_id, applier));
            if !wanted {
                teardown(e, true);
                e.session = None;
                return false;
            }
            if e.session == Some(session) {
                return false;
            }
            teardown(e, true);
            e.session = Some(session);
            true
        });
        if !start {
            return;
        }
        let (promise, xr) = match request_session(mount_id) {
            Ok(v) => v,
            Err(f) => return fail(applier, session, f),
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise).await;
            match result {
                Err(err) => {
                    let name = name_of(&err);
                    let supported = if name == "NotSupportedError" {
                        is_supported(&xr).await
                    } else {
                        None
                    };
                    if current(mount_id, session) {
                        fail(&applier, session, classify_request_error(&name, supported));
                    }
                }
                Ok(xr_session) => {
                    if !current(mount_id, session) {
                        // 待ち中に停止・破棄が起きた。受け取ったセッション（カメラ）を残さない。
                        end_session(&xr_session);
                        return;
                    }
                    if let Err(f) = setup(mount_id, session, xr_session.clone()).await {
                        if current(mount_id, session) {
                            ENTRIES.with(|m| {
                                if let Some(e) = m.borrow_mut().get_mut(&mount_id) {
                                    teardown(e, true);
                                }
                            });
                            // 登録前の失敗では e.xr が空なので、直接終える（冪等）。
                            end_session(&xr_session);
                            fail(&applier, session, f);
                        }
                    }
                }
            }
        });
    }

    pub(crate) fn release_mount(mount_id: u64) {
        if let Some(mut e) = ENTRIES.with(|m| m.borrow_mut().remove(&mount_id)) {
            teardown(&mut e, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compass::is_current;

    fn r() -> ArReading {
        reading_from_pose(Some(([0.0; 3], false))).unwrap()
    }

    #[test]
    fn pose_mapping() {
        let none = reading_from_pose(None).unwrap();
        assert_eq!(none.tracking, Tracking::NotAvailable);
        assert_eq!(none.position, None);
        let lim = reading_from_pose(Some(([1.0, 2.0, 3.0], true))).unwrap();
        assert_eq!(lim.tracking, Tracking::Limited(LimitedReason::Other));
        assert_eq!(lim.position, Some([1.0, 2.0, 3.0]));
        for p in [[f64::NAN, 0.0, 0.0], [0.0, 1001.0, 0.0]] {
            assert_eq!(reading_from_pose(Some((p, false))), None);
        }
        assert_eq!(r().horizontal_planes, None);
    }

    #[test]
    fn request_error_mapping() {
        use ArFailure::*;
        assert_eq!(
            classify_request_error("SecurityError", None),
            PermissionDenied
        );
        assert_eq!(
            classify_request_error("NotSupportedError", Some(false)),
            Unsupported
        );
        assert_eq!(
            classify_request_error("NotSupportedError", Some(true)),
            PermissionDenied
        );
        assert_eq!(
            classify_request_error("NotSupportedError", None),
            Unsupported
        );
        assert_eq!(classify_request_error("AbortError", Some(true)), Failed);
    }

    #[test]
    fn tick_hidden_stale_and_expiry() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        let latest = Stamped::new(Some((r(), 9_900.0)));
        assert_eq!(tick(true, &latest, &mut t, 10_000.0), Tick::Send(None));
        assert_eq!(latest.get(), None);
        assert_eq!(tick(false, &latest, &mut t, 10_000.0), Tick::Send(None));
        assert_eq!(tick(false, &latest, &mut t, 13_001.0), Tick::Expired);
        latest.set(Some((r(), 13_100.0)));
        assert_eq!(
            tick(false, &latest, &mut t, 13_100.0),
            Tick::Send(Some(r()))
        );
        // 途絶した値は捨てる。
        assert_eq!(tick(false, &latest, &mut t, 20_000.0), Tick::Send(None));
        assert_eq!(latest.get(), None);
    }

    #[test]
    fn visibility_return_restarts_wait() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        let latest = Stamped::new(Some((r(), 0.0)));
        on_visibility(true, &latest, &mut t, 500.0);
        assert_eq!(latest.get(), None);
        on_visibility(false, &latest, &mut t, 10_000.0);
        assert_eq!(tick(false, &latest, &mut t, 10_000.0), Tick::Send(None));
        assert_eq!(tick(false, &latest, &mut t, 13_001.0), Tick::Expired);
    }

    #[test]
    fn stale_session_dropped() {
        assert!(is_current(true, Some(2), 2));
        assert!(!is_current(true, Some(3), 2));
        assert!(!is_current(false, Some(2), 2));
    }
}
