//! 型サンプル「方位（コンパス）」の Web 経路（親 #1154、Issue #1407）。
//!
//! `fandhe_core::screens::compass_show`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`]
//! の effect として駆動する。`fandhe-egui-proxy` の wasm canvas 経路は対象外
//! （`UnsupportedCompass` のまま）。`fandhe_device::compass` から `READ_INTERVAL`・
//! `normalize_heading`・`WaitTracker` を共用し、`Compass` trait（`Send + Sync`）は
//! JS の Closure を持てないため通さず、core の `apply_reading` / `apply_failure` へ直接流す。
//!
//! # 方式
//!
//! - Chromium / Firefox: `deviceorientationabsolute` の `alpha`（反時計回り）を `360 - alpha`
//!   で時計回りの磁北基準へ。`absolute` が偽・`alpha` が null / 範囲外は値なし
//! - Safari: `deviceorientation` の `webkitCompassHeading`。iOS は `requestPermission()` を
//!   「開始」押下の同期区間で呼ぶ（transient activation）
//! - イベントは最新値を書くだけ。画面への反映は `READ_INTERVAL` の `setInterval` だけが行う
//!   （書き手 1 本）。非表示中は値を捨て、最後の値を出し続けない。値なしが
//!   `NO_READING_TIMEOUT_MS` 続けば `NoReading`（単調時計 `performance.now()` で測る）
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 0° や古い値を出さない、1 マウントで二重に購読しない、停止・失敗・マウント破棄で
//! 購読を解除する、非対応・権限拒否・値なしの区別、古い世代の完了の破棄。
//! 守らない: 偏角（真北）とブラウザ間の基準の違い、画面回転・端末を立てた姿勢、磁気の乱れ、
//! 許可ダイアログの応答待ちの期限、Permissions Policy の区別、複数タブ・複数マウント、
//! 表示の遅れ（最大 `READ_INTERVAL` + イベント周期。背景タブでは間引きでさらに遅れる）。
//! `visibilitychange` も購読し、非表示で保持値と期限の起点を捨て、可視復帰の時刻から期限を数え直す
//! （背景で interval が止まっても非表示の時間を期限へ含めない）。
//! 受信から `STALE_MS` 途絶えた値は値なし扱いにし、そこから `NO_READING_TIMEOUT_MS` で `NoReading`。
//! 非表示中に届いたイベントは listener が捨てる（tick が走る前に凍結されても古い値が残らない）。
//!
//! 方位・精度・例外の message は DOM・ログへ出さない（security.md A09）。

use fandhe_core::screens::compass_show::{AccuracyView, CompassFailureView, HeadingView};
use fandhe_device::compass::{
    compass_point, display_degrees, normalize_heading, Accuracy, CompassFailure, CompassPoint,
    Heading, WaitTracker,
};

/// Chromium / Firefox: `alpha`（反時計回り）を時計回りの方位へ。`absolute` が偽・null・範囲外は値なし。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn heading_from_absolute(alpha: Option<f64>, absolute: bool) -> Option<f64> {
    if !absolute {
        return None;
    }
    let a = alpha.filter(|a| a.is_finite() && (0.0..=360.0).contains(a))?;
    normalize_heading(360.0 - a)
}

/// Safari: `webkitCompassHeading`（時計回り）。無い・範囲外は値なし。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn heading_from_webkit(raw: Option<f64>) -> Option<f64> {
    raw.and_then(normalize_heading)
}

/// Safari の `webkitCompassAccuracy`（負は較正不能）。それ以外は OS 間で閾値を作らず不明。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn accuracy_from_webkit(raw: Option<f64>) -> Accuracy {
    match raw {
        Some(a) if a < 0.0 => Accuracy::Unreliable,
        _ => Accuracy::Unknown,
    }
}

/// `requestPermission()` の解決値。`"granted"` だけ許可。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn permission_outcome(result: Option<&str>) -> Result<(), CompassFailure> {
    match result {
        Some("granted") => Ok(()),
        Some("denied") => Err(CompassFailure::PermissionDenied),
        _ => Err(CompassFailure::Failed),
    }
}

/// `requestPermission()` の reject の例外名。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn permission_error(name: &str) -> CompassFailure {
    if name == "NotAllowedError" {
        CompassFailure::PermissionDenied
    } else {
        CompassFailure::Failed
    }
}

/// 1 回の tick の判定結果。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, PartialEq)]
pub(crate) enum Tick {
    /// 画面へ送る値（`None` は値なし）。
    Send(Option<Heading>),
    /// 値なしが期限を超えた。
    Expired,
}

/// 受信から経過してよい上限（ms）。超えた値は「途絶」として値なし扱いにする
/// （`deviceorientation` は通常数十 ms 周期。端末が静止していても多くのブラウザは周期的に届く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const STALE_MS: f64 = 2000.0;

/// 画面に出る内容の比較キー（整数度・8 方位・精度）。どれかが変われば再描画する。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn view_key(h: Option<Heading>) -> Option<(u16, CompassPoint, Accuracy)> {
    h.map(|h| {
        (
            display_degrees(h.degrees),
            compass_point(h.degrees),
            h.accuracy,
        )
    })
}

/// 保持値: 方位と受信時刻（`performance.now()` の ms）。
pub(crate) type Stamped = std::cell::Cell<Option<(Heading, f64)>>;

/// `visibilitychange` の処理。非表示で保持値と起点を捨て、可視復帰で復帰時刻から期限を数え直す
/// （interval が背景で止まって tick(hidden) が走らなくても、非表示の時間を期限へ含めない）。
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

/// 非表示中は保持値 `latest` を捨てて期限も数えない（間引かれた tick で `NoReading` にせず、
/// 可視復帰後に古い方位を出さない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn tick(hidden: bool, latest: &Stamped, tracker: &mut WaitTracker, now_ms: f64) -> Tick {
    if hidden {
        latest.set(None);
        tracker.reset();
        return Tick::Send(None);
    }
    // 受信から STALE_MS を超えた値は捨てる（途絶した最後の値を出し続けない）。
    let fresh = latest.get().filter(|(_, at)| now_ms - at <= STALE_MS);
    if fresh.is_none() {
        latest.set(None);
    }
    let latest = fresh.map(|(h, _)| h);
    if tracker.observe(latest.is_some(), now_ms) {
        Tick::Expired
    } else {
        Tick::Send(latest)
    }
}

/// 許可待ちの再開後に結果を採用してよいか（マウント生存・同じ世代がまだ動いている）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_current(live: bool, entry_session: Option<u64>, started: u64) -> bool {
    live && entry_session == Some(started)
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn heading_view(h: Heading) -> HeadingView {
    HeadingView {
        degrees: h.degrees,
        accuracy: match h.accuracy {
            Accuracy::Unknown => AccuracyView::Unknown,
            Accuracy::Unreliable => AccuracyView::Unreliable,
            Accuracy::Approximate => AccuracyView::Approximate,
            Accuracy::High => AccuracyView::High,
        },
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn failure_view(f: CompassFailure) -> CompassFailureView {
    match f {
        CompassFailure::Unsupported => CompassFailureView::Unsupported,
        CompassFailure::PermissionDenied => CompassFailureView::PermissionDenied,
        CompassFailure::Failed => CompassFailureView::Failed,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::compass_show_effect;

/// マウント破棄時にリスナーとタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::compass_show::{
        apply_failure, apply_reading, CompassFailureView, CompassShowState,
    };
    use fandhe_device::compass::{
        Accuracy, CompassFailure, CompassPoint, Heading, WaitTracker, READ_INTERVAL,
    };
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{
        accuracy_from_webkit, failure_view, heading_from_absolute, heading_from_webkit,
        heading_view, is_current, on_visibility, permission_error, permission_outcome, tick,
        view_key, Tick,
    };
    use crate::mount::{is_live, Applier};

    type Latest = Rc<super::Stamped>;

    /// マウント単位の状態。closure は 1 度だけ作り、世代の切り替えでは add/remove・
    /// set/clear だけを行う（tick の中で失敗を確定すると apply → effect が同期で進むため、
    /// 実行中の closure を drop しない。drop は `release_mount` だけ）。
    struct Entry {
        /// 動かしている世代。`None` は停止。
        session: Option<u64>,
        listening: Option<&'static str>,
        /// `document` の `visibilitychange` を購読中か。
        watching_visibility: bool,
        interval: Option<i32>,
        latest: Latest,
        tracker: WaitTracker,
        /// 最後に画面へ送った表示上の値（外側 `None` は未送信）。
        last_sent: Option<Option<(u16, CompassPoint, Accuracy)>>,
        listener: Closure<dyn FnMut(JsValue)>,
        ticker: Closure<dyn FnMut()>,
        visibility: Closure<dyn FnMut()>,
    }

    thread_local! {
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    fn get_num(o: &JsValue, key: &str) -> Option<f64> {
        Reflect::get(o, &JsValue::from_str(key)).ok()?.as_f64()
    }

    /// イベントから方位を作る。`webkitCompassHeading` を持つなら Safari の経路。
    fn heading_of(ev: &JsValue) -> Option<Heading> {
        if Reflect::has(ev, &JsValue::from_str("webkitCompassHeading")).unwrap_or(false) {
            let degrees = heading_from_webkit(get_num(ev, "webkitCompassHeading"))?;
            return Some(Heading {
                degrees,
                accuracy: accuracy_from_webkit(get_num(ev, "webkitCompassAccuracy")),
            });
        }
        let absolute = Reflect::get(ev, &JsValue::from_str("absolute"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let degrees = heading_from_absolute(get_num(ev, "alpha"), absolute)?;
        Some(Heading {
            degrees,
            accuracy: Accuracy::Unknown,
        })
    }

    /// リスナーと interval を外し、保持値を捨てる（冪等）。
    fn teardown(e: &mut Entry) {
        if let Some(w) = web_sys::window() {
            if let Some(name) = e.listening.take() {
                let _ = w
                    .remove_event_listener_with_callback(name, e.listener.as_ref().unchecked_ref());
            }
            if let Some(h) = e.interval.take() {
                w.clear_interval_with_handle(h);
            }
            if std::mem::take(&mut e.watching_visibility) {
                if let Some(d) = w.document() {
                    let _ = d.remove_event_listener_with_callback(
                        "visibilitychange",
                        e.visibility.as_ref().unchecked_ref(),
                    );
                }
            }
        }
        e.latest.set(None);
        e.tracker.reset();
        e.last_sent = None;
    }

    fn fail(applier: &Applier<CompassShowState>, session: u64, f: CompassFailure) {
        let v = failure_view(f);
        applier.apply(move |s| apply_failure(s, session, v));
    }

    /// 使うイベント名。非 secure context・センサー API 無しは `None`。
    fn event_name(w: &web_sys::Window) -> Option<&'static str> {
        if !w.is_secure_context() {
            return None;
        }
        let has = |k: &str| Reflect::has(w, &JsValue::from_str(k)).unwrap_or(false);
        if has("ondeviceorientationabsolute") {
            Some("deviceorientationabsolute")
        } else if has("DeviceOrientationEvent") {
            Some("deviceorientation")
        } else {
            None
        }
    }

    /// iOS Safari の `DeviceOrientationEvent.requestPermission` を呼ぶ（無ければ `None`）。
    /// 同期区間で呼ぶこと。呼び出しの例外は `Some(Err)`。
    fn request_permission(w: &web_sys::Window) -> Option<Result<Promise, CompassFailure>> {
        let ctor = Reflect::get(w, &JsValue::from_str("DeviceOrientationEvent")).ok()?;
        let f = Reflect::get(&ctor, &JsValue::from_str("requestPermission")).ok()?;
        let f: Function = f.dyn_into().ok()?;
        Some(match f.call0(&ctor) {
            Ok(p) => Ok(Promise::resolve(&p)),
            Err(_) => Err(CompassFailure::Failed),
        })
    }

    /// リスナーと interval を登録する。失敗時は確保済みの分を解放して `Err`。
    fn register(mount_id: u64, name: &'static str) -> Result<(), ()> {
        let w = web_sys::window().ok_or(())?;
        // 単調時計が無ければ期限を数えられない（0.0 固定で待ち続けない）。
        let now = w.performance().ok_or(())?.now();
        ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.get_mut(&mount_id).ok_or(())?;
            w.add_event_listener_with_callback(name, e.listener.as_ref().unchecked_ref())
                .map_err(|_| ())?;
            e.listening = Some(name);
            match w.set_interval_with_callback_and_timeout_and_arguments_0(
                e.ticker.as_ref().unchecked_ref(),
                READ_INTERVAL.as_millis() as i32,
            ) {
                Ok(h) => e.interval = Some(h),
                Err(_) => {
                    teardown(e);
                    return Err(());
                }
            }
            let added = w.document().is_some_and(|d| {
                d.add_event_listener_with_callback(
                    "visibilitychange",
                    e.visibility.as_ref().unchecked_ref(),
                )
                .is_ok()
            });
            if !added {
                teardown(e);
                return Err(());
            }
            e.watching_visibility = true;
            e.tracker.begin(now);
            Ok(())
        })
    }

    fn new_entry(mount_id: u64, applier: &Applier<CompassShowState>) -> Entry {
        let latest: Latest = Rc::new(Cell::new(None));
        let l = latest.clone();
        let listener = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            let hidden = web_sys::window()
                .and_then(|w| w.document())
                .map(|d| d.hidden())
                .unwrap_or(false);
            // 非表示中は保持しない（tick より先に凍結されても復帰後に古い値を出さない）。
            let now = web_sys::window()
                .and_then(|w| w.performance())
                .map(|p| p.now());
            l.set(match (hidden, now) {
                (false, Some(at)) => heading_of(&ev).map(|h| (h, at)),
                _ => None,
            });
        });
        let tick_applier = applier.clone();
        let ticker = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let hidden = web_sys::window()
                .and_then(|w| w.document())
                .map(|d| d.hidden())
                .unwrap_or(false);
            let now = web_sys::window()
                .and_then(|w| w.performance())
                .map(|p| p.now());
            // ENTRIES の borrow は apply の前に落とす（apply → effect が再び借りるため）。
            let action = ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let e = m.get_mut(&mount_id)?;
                let session = e.session?;
                e.listening?;
                // 単調時計を失ったら期限を数えられない。待ち続けず失敗で確定する。
                let Some(now) = now else {
                    teardown(e);
                    return Some((session, Err(CompassFailureView::Failed)));
                };
                match tick(hidden, &e.latest, &mut e.tracker, now) {
                    Tick::Expired => {
                        // 失敗の確定より前に購読を止める。
                        teardown(e);
                        Some((session, Err(CompassFailureView::NoReading)))
                    }
                    Tick::Send(h) => {
                        let key = view_key(h);
                        if e.last_sent == Some(key) {
                            return None;
                        }
                        e.last_sent = Some(key);
                        Some((session, Ok(h)))
                    }
                }
            });
            match action {
                Some((session, Err(v))) => {
                    tick_applier.apply(move |s| apply_failure(s, session, v));
                }
                Some((session, Ok(h))) => {
                    let v = h.map(heading_view);
                    tick_applier.apply(move |s| apply_reading(s, session, v));
                }
                None => {}
            }
        });
        let visibility = Closure::<dyn FnMut()>::new(move || {
            let Some(w) = web_sys::window() else { return };
            let hidden = w.document().map(|d| d.hidden()).unwrap_or(false);
            let Some(now) = w.performance().map(|p| p.now()) else {
                return;
            };
            ENTRIES.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&mount_id) {
                    on_visibility(hidden, &e.latest, &mut e.tracker, now);
                }
            });
        });
        Entry {
            session: None,
            listening: None,
            watching_visibility: false,
            interval: None,
            latest,
            tracker: WaitTracker::default(),
            last_sent: None,
            listener,
            ticker,
            visibility,
        }
    }

    fn get_name(e: &JsValue) -> Option<String> {
        Reflect::get(e, &JsValue::from_str("name"))
            .ok()?
            .as_string()
    }

    /// `mount_with_effect` へ渡す effect。再描画のたびに呼ばれ、世代が変わったときだけ動く。
    pub fn compass_show_effect(state: &CompassShowState, applier: &Applier<CompassShowState>) {
        let mount_id = applier.mount_id();
        let session = state.session;
        let wanted = state.wanted;
        let start = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m
                .entry(mount_id)
                .or_insert_with(|| new_entry(mount_id, applier));
            if !wanted {
                teardown(e);
                e.session = None;
                return false;
            }
            if e.session == Some(session) {
                return false;
            }
            teardown(e);
            e.session = Some(session);
            true
        });
        if !start {
            return;
        }
        let Some(w) = web_sys::window() else {
            return fail(applier, session, CompassFailure::Unsupported);
        };
        let Some(name) = event_name(&w) else {
            return fail(applier, session, CompassFailure::Unsupported);
        };
        match request_permission(&w) {
            None => {
                if register(mount_id, name).is_err() {
                    fail(applier, session, CompassFailure::Failed);
                }
            }
            Some(Err(f)) => fail(applier, session, f),
            Some(Ok(promise)) => {
                let applier = applier.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let result = JsFuture::from(promise).await;
                    // 再開後: まだ現在の世代か（登録前なので解放物は無い）。
                    let current = ENTRIES.with(|m| {
                        m.borrow()
                            .get(&mount_id)
                            .map(|e| is_current(is_live(mount_id), e.session, session))
                            .unwrap_or(false)
                    });
                    if !current {
                        return;
                    }
                    let outcome = match result {
                        Ok(v) => permission_outcome(v.as_string().as_deref()),
                        Err(e) => Err(permission_error(&get_name(&e).unwrap_or_default())),
                    };
                    match outcome {
                        Ok(()) => {
                            if register(mount_id, name).is_err() {
                                fail(&applier, session, CompassFailure::Failed);
                            }
                        }
                        Err(f) => fail(&applier, session, f),
                    }
                });
            }
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        if let Some(mut e) = ENTRIES.with(|m| m.borrow_mut().remove(&mount_id)) {
            teardown(&mut e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(d: f64) -> Heading {
        Heading {
            degrees: d,
            accuracy: Accuracy::Unknown,
        }
    }

    #[test]
    fn absolute_maps_counterclockwise_alpha() {
        assert_eq!(heading_from_absolute(Some(270.0), true), Some(90.0));
        assert_eq!(heading_from_absolute(Some(0.0), true), Some(0.0));
        assert_eq!(heading_from_absolute(Some(360.0), true), Some(0.0));
        assert_eq!(heading_from_absolute(Some(90.0), false), None);
        for bad in [None, Some(f64::NAN), Some(-1.0), Some(361.0)] {
            assert_eq!(heading_from_absolute(bad, true), None);
        }
    }

    #[test]
    fn webkit_and_accuracy() {
        assert_eq!(heading_from_webkit(Some(123.0)), Some(123.0));
        assert_eq!(heading_from_webkit(Some(400.0)), None);
        assert_eq!(heading_from_webkit(None), None);
        assert_eq!(accuracy_from_webkit(Some(-1.0)), Accuracy::Unreliable);
        assert_eq!(accuracy_from_webkit(Some(15.0)), Accuracy::Unknown);
        assert_eq!(accuracy_from_webkit(None), Accuracy::Unknown);
    }

    #[test]
    fn permission_mapping() {
        assert_eq!(permission_outcome(Some("granted")), Ok(()));
        assert_eq!(
            permission_outcome(Some("denied")),
            Err(CompassFailure::PermissionDenied)
        );
        assert_eq!(permission_outcome(None), Err(CompassFailure::Failed));
        assert_eq!(
            permission_error("NotAllowedError"),
            CompassFailure::PermissionDenied
        );
        assert_eq!(permission_error("TypeError"), CompassFailure::Failed);
    }

    #[test]
    fn tick_hidden_resets_and_expires_only_when_visible() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        let latest = Stamped::new(Some((h(1.0), 9_900.0)));
        assert_eq!(tick(true, &latest, &mut t, 10_000.0), Tick::Send(None));
        // hidden で保持値も起点も捨てたので、可視復帰後は古い値を出さず、最初の tick は期限切れにならない。
        assert_eq!(latest.get(), None);
        assert_eq!(tick(false, &latest, &mut t, 10_000.0), Tick::Send(None));
        assert_eq!(tick(false, &latest, &mut t, 13_001.0), Tick::Expired);
        latest.set(Some((h(5.0), 13_100.0)));
        assert_eq!(
            tick(false, &latest, &mut t, 13_100.0),
            Tick::Send(Some(h(5.0)))
        );
    }

    #[test]
    fn visibility_return_restarts_wait_from_return_time() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        let latest = Stamped::new(None);
        // 非表示 tick が走らず凍結 -> 10 秒後に復帰。復帰時刻を起点に数え直すので即 Expired にならない。
        on_visibility(true, &latest, &mut t, 500.0);
        on_visibility(false, &latest, &mut t, 10_000.0);
        assert_eq!(tick(false, &latest, &mut t, 10_000.0), Tick::Send(None));
        assert_eq!(tick(false, &latest, &mut t, 13_001.0), Tick::Expired);
        // 非表示で保持値も捨てる。
        let l2 = Stamped::new(Some((h(1.0), 0.0)));
        on_visibility(true, &l2, &mut t, 1.0);
        assert_eq!(l2.get(), None);
    }

    #[test]
    fn tick_silent_after_one_reading_goes_stale_then_expires() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        let latest = Stamped::new(Some((h(30.0), 0.0)));
        assert_eq!(
            tick(false, &latest, &mut t, 100.0),
            Tick::Send(Some(h(30.0)))
        );
        assert_eq!(
            tick(false, &latest, &mut t, STALE_MS),
            Tick::Send(Some(h(30.0)))
        );
        // 途絶: 古い値は出さず、値なしの期限を数え始める。
        assert_eq!(tick(false, &latest, &mut t, 4_000.0), Tick::Send(None));
        assert_eq!(latest.get(), None);
        assert_eq!(tick(false, &latest, &mut t, 7_001.0), Tick::Expired);
    }

    #[test]
    fn view_key_includes_eight_point_label() {
        let k = |d| view_key(Some(h(d))).unwrap();
        assert_eq!((k(22.4).0, k(22.4).1), (22, CompassPoint::North));
        assert_eq!((k(22.5).0, k(22.5).1), (23, CompassPoint::NorthEast));
        assert_ne!(view_key(Some(h(22.4))), view_key(Some(h(22.5))));
        assert_eq!(view_key(None), None);
    }

    #[test]
    fn stale_session_dropped() {
        assert!(is_current(true, Some(2), 2));
        assert!(!is_current(false, Some(2), 2));
        assert!(!is_current(true, Some(3), 2));
        assert!(!is_current(true, None, 2));
    }
}
