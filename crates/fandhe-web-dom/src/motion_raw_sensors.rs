//! 型サンプル「ジャイロ・磁気・線形加速度」の Web 経路（親 #180、Issue #902）。
//!
//! `fandhe_core::screens::motion_raw_sensors_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。呼び出し元は `demo.rs` の
//! エントリポイントと `mount::sweep_mounts`。`MotionRawSensors` trait は `Send + Sync` で JS の
//! `Closure` を持てないため通さず、core の `apply_reading` / `apply_failure` へ直接流す
//! （compass / memory_pressure と同じ扱い）。`fandhe-egui-proxy` の wasm canvas 経路は対象外
//! （`UnsupportedMotionRawSensors` のまま）。
//!
//! # 方式
//!
//! - `devicemotion` の `rotationRate`（度/秒。beta→x、gamma→y、alpha→z）と
//!   `acceleration`（m/s²。重力を除く）。成分が 1 つでも null なら値なし
//! - 磁場は Web で取れないため常に `UnsupportedOnOs`（Generic Sensor の `Magnetometer` は使わない）
//! - secure context が必須。`DeviceMotionEvent.requestPermission` があれば「開始」と同じ同期区間で
//!   呼ぶ（transient activation。専用ボタンは置かない）
//! - イベントは最新値を書くだけ。画面への反映は `READ_INTERVAL` の `setInterval` だけが行う（書き手 1 本）。
//!   送信は `apply_in_place` で 3 行の文だけを書き換え、「停止」ボタンの DOM を作り直さない（#815）
//! - **期限・古さの閾値を置かない**: 値が届かない間は待機のまま。古い値を捨てるのは非表示と null 成分のときだけ。
//!   前回値や 0 では埋めない
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 嘘の値を出さない、種類ごとに購読を二重にしない、停止・失敗・非表示・マウント破棄で解除する
//! （非表示で devicemotion リスナーと interval を外し、可視復帰で現在の世代があれば再登録する。
//! 許可要求が非表示中に完了した場合も登録を保留する）、
//! 古い世代の完了の破棄、非対応・権限拒否・失敗の区別。
//! 守らない: 値が届かないまま待ち続ける場合（Web では `NotPresent` を出さない）、ブラウザ間の座標・軸の違いと
//! 画面回転、許可ダイアログの応答待ちの期限、複数タブ・複数マウント、表示の遅れ（最大 `READ_INTERVAL` +
//! イベント周期）、bfcache 復帰後の配信（非表示 → 可視の扱いに任せる）。
//!
//! センサー値・例外の name / message は DOM・ログへ出さない（security.md A09）。

use fandhe_core::screens::motion_raw_sensors_show::{RawFailureView, RawReadingView, RawUnitView};
use fandhe_device::motion_raw_sensors::{
    validate_reading, RawKind, RawReading, RawSensorFailure, RawUnit,
};

/// `rotationRate` の 3 成分から角速度を作る。1 つでも null、または非有限なら値なし。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn angular_from(
    alpha: Option<f64>,
    beta: Option<f64>,
    gamma: Option<f64>,
) -> Option<RawReading> {
    validate_reading(
        RawKind::AngularVelocity,
        RawReading {
            x: beta?,
            y: gamma?,
            z: alpha?,
            unit: RawUnit::DegPerSec,
        },
    )
}

/// `acceleration` の 3 成分から線形加速度を作る。1 つでも null、または非有限なら値なし。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn linear_from(x: Option<f64>, y: Option<f64>, z: Option<f64>) -> Option<RawReading> {
    validate_reading(
        RawKind::LinearAcceleration,
        RawReading {
            x: x?,
            y: y?,
            z: z?,
            unit: RawUnit::MetersPerSec2,
        },
    )
}

/// `requestPermission()` の解決値。`"granted"` だけ許可。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn permission_outcome(result: Option<&str>) -> Result<(), RawSensorFailure> {
    match result {
        Some("granted") => Ok(()),
        Some("denied") => Err(RawSensorFailure::PermissionDenied),
        _ => Err(RawSensorFailure::Failed),
    }
}

/// `requestPermission()` の reject の例外名。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn permission_error(name: &str) -> RawSensorFailure {
    if name == "NotAllowedError" {
        RawSensorFailure::PermissionDenied
    } else {
        RawSensorFailure::Failed
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_view(r: RawReading) -> RawReadingView {
    RawReadingView {
        x: r.x,
        y: r.y,
        z: r.z,
        unit: match r.unit {
            RawUnit::RadPerSec => RawUnitView::RadPerSec,
            RawUnit::DegPerSec => RawUnitView::DegPerSec,
            RawUnit::MicroTesla => RawUnitView::MicroTesla,
            RawUnit::StandardGravity => RawUnitView::StandardGravity,
            RawUnit::MetersPerSec2 => RawUnitView::MetersPerSec2,
        },
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_view(f: RawSensorFailure) -> RawFailureView {
    match f {
        RawSensorFailure::UnsupportedOnOs => RawFailureView::UnsupportedOnOs,
        RawSensorFailure::NotPresent => RawFailureView::NotPresent,
        RawSensorFailure::PermissionDenied => RawFailureView::PermissionDenied,
        RawSensorFailure::Failed => RawFailureView::Failed,
    }
}

/// 購読の計画。添字 0 が角速度、1 が線形加速度。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// 全部下りた。解除する。
    Stop,
    /// 落ちた種類があるだけ。購読は維持する（失敗の反映で再開始・再要求のループにしない）。
    Keep,
    /// 新しい世代が立ち上がった。解除して許可要求と登録をやり直す。
    Restart,
}

/// `active`（動かしている世代）と `want`（`wanted` な世代）から計画を決める。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decide(active: [Option<u64>; 2], want: [Option<u64>; 2]) -> Plan {
    if want.iter().all(Option::is_none) {
        return Plan::Stop;
    }
    if (0..2).all(|i| want[i].is_none() || want[i] == active[i]) {
        Plan::Keep
    } else {
        Plan::Restart
    }
}

/// 表示上同じ値か（`None` 同士は一致）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn view_same(a: &Option<RawReadingView>, b: &Option<RawReadingView>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => fandhe_core::screens::motion_raw_sensors_show::display_equal(a, b),
        _ => false,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::motion_raw_sensors_show_effect;

/// マウント破棄時にリスナーとタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::motion_raw_sensors_show::{
        apply_failure, apply_reading, row_text, MotionRawSensorsShowState, RawKindView,
        RawReadingView,
    };
    use fandhe_device::motion_raw_sensors::{RawReading, RawSensorFailure, READ_INTERVAL};
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{
        angular_from, decide, failure_view, linear_from, permission_error, permission_outcome,
        to_view, view_same, Plan,
    };
    use crate::mount::{is_live, Applier};

    type State = MotionRawSensorsShowState;
    type Latest = Rc<Cell<[Option<RawReading>; 2]>>;

    /// 添字 0・1 に対応する種類。
    const KINDS: [RawKindView; 2] = [
        RawKindView::AngularVelocity,
        RawKindView::LinearAcceleration,
    ];

    /// マウント単位の状態。closure は 1 度だけ作り、世代の切り替えでは add/remove・set/clear だけを行う
    /// （drop は `release_mount` だけ）。
    struct Entry {
        /// 動かしている世代（種類ごと）。`None` は停止。
        active: [Option<u64>; 2],
        /// 磁場の失敗を確定済みの世代。
        mag_failed: Option<u64>,
        listening: bool,
        watching_visibility: bool,
        interval: Option<i32>,
        latest: Latest,
        /// 最後に画面へ送った値（外側 `None` は未送信）。
        last_sent: [Option<Option<RawReadingView>>; 2],
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

    fn get_obj(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::NULL)
    }

    fn is_hidden() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .map(|d| d.hidden())
            .unwrap_or(false)
    }

    /// devicemotion リスナーと interval だけを外し、保持値を捨てる（冪等）。
    /// 非表示中の購読資源の保持を避けるため、visibilitychange の監視は残す。
    fn release_sensing(e: &mut Entry) {
        if let Some(w) = web_sys::window() {
            if std::mem::take(&mut e.listening) {
                let _ = w.remove_event_listener_with_callback(
                    "devicemotion",
                    e.listener.as_ref().unchecked_ref(),
                );
            }
            // clearInterval でハンドルを止めてから closure は Entry が保持する（drop は release_mount のみ）。
            if let Some(h) = e.interval.take() {
                w.clear_interval_with_handle(h);
            }
        }
        e.latest.set([None; 2]);
        e.last_sent = [None; 2];
    }

    /// リスナー・interval・visibilitychange を外し、保持値を捨てる（冪等）。
    fn teardown(e: &mut Entry) {
        release_sensing(e);
        if std::mem::take(&mut e.watching_visibility) {
            if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                let _ = d.remove_event_listener_with_callback(
                    "visibilitychange",
                    e.visibility.as_ref().unchecked_ref(),
                );
            }
        }
    }

    /// devicemotion リスナーと interval を登録する。失敗時は確保済みの分を解放して `Err`。
    fn add_sensing(e: &mut Entry, w: &web_sys::Window) -> Result<(), ()> {
        if e.listening {
            return Ok(());
        }
        if w.add_event_listener_with_callback("devicemotion", e.listener.as_ref().unchecked_ref())
            .is_err()
        {
            return Err(());
        }
        e.listening = true;
        match w.set_interval_with_callback_and_timeout_and_arguments_0(
            e.ticker.as_ref().unchecked_ref(),
            READ_INTERVAL.as_millis() as i32,
        ) {
            Ok(h) => e.interval = Some(h),
            Err(_) => {
                release_sensing(e);
                return Err(());
            }
        }
        Ok(())
    }

    /// `want` の全種類を同じ失敗で 1 度に確定する（反映途中の effect で片方だけ再開始させない）。
    fn fail_all(applier: &Applier<State>, want: [Option<u64>; 2], f: RawSensorFailure) {
        let v = failure_view(f);
        applier.apply(move |s| {
            for (i, w) in want.iter().enumerate() {
                if let Some(session) = w {
                    apply_failure(s, KINDS[i], *session, v);
                }
            }
        });
    }

    /// 3 行の文だけを既存の DOM へ直接書く。`p` がちょうど 3 件でなければ `false`（全再構築へ倒す）。
    fn patch_rows(root: &web_sys::Element, state: &State) -> bool {
        let Ok(ps) = root.query_selector_all("p") else {
            return false;
        };
        if ps.length() != 3 {
            return false;
        }
        for (i, kind) in [
            RawKindView::AngularVelocity,
            RawKindView::MagneticField,
            RawKindView::LinearAcceleration,
        ]
        .into_iter()
        .enumerate()
        {
            let Some(n) = ps.item(i as u32) else {
                return false;
            };
            n.set_text_content(Some(&row_text(state, kind)));
        }
        true
    }

    /// 許可要求の関数を呼ぶ（無ければ `None`）。同期区間で呼ぶこと。
    fn request_permission(w: &web_sys::Window) -> Option<Result<Promise, RawSensorFailure>> {
        let ctor = Reflect::get(w, &JsValue::from_str("DeviceMotionEvent")).ok()?;
        let f = Reflect::get(&ctor, &JsValue::from_str("requestPermission")).ok()?;
        let f: Function = f.dyn_into().ok()?;
        Some(match f.call0(&ctor) {
            Ok(p) => Ok(Promise::resolve(&p)),
            Err(_) => Err(RawSensorFailure::Failed),
        })
    }

    /// visibilitychange を監視し、表示中ならリスナー・interval も登録する。失敗時は全て解放して `Err`。
    /// 非表示中の完了なら購読は保留し、可視へ戻った時点で `visibility` が登録する。
    fn register(mount_id: u64) -> Result<(), ()> {
        let w = web_sys::window().ok_or(())?;
        let hidden = is_hidden();
        ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.get_mut(&mount_id).ok_or(())?;
            if !e.watching_visibility {
                let added = w.document().is_some_and(|d| {
                    d.add_event_listener_with_callback(
                        "visibilitychange",
                        e.visibility.as_ref().unchecked_ref(),
                    )
                    .is_ok()
                });
                if !added {
                    return Err(());
                }
                e.watching_visibility = true;
            }
            if !hidden && add_sensing(e, &w).is_err() {
                teardown(e);
                return Err(());
            }
            Ok(())
        })
    }

    fn new_entry(mount_id: u64, applier: &Applier<State>) -> Entry {
        let latest: Latest = Rc::new(Cell::new([None; 2]));
        let l = latest.clone();
        let listener = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
            // 非表示中は保持しない（復帰後に古い値を出さない）。
            if is_hidden() {
                l.set([None; 2]);
                return;
            }
            let rr = get_obj(&ev, "rotationRate");
            let acc = get_obj(&ev, "acceleration");
            l.set([
                angular_from(
                    get_num(&rr, "alpha"),
                    get_num(&rr, "beta"),
                    get_num(&rr, "gamma"),
                ),
                linear_from(get_num(&acc, "x"), get_num(&acc, "y"), get_num(&acc, "z")),
            ]);
        });
        let tick_applier = applier.clone();
        let ticker = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let hidden = is_hidden();
            // ENTRIES の borrow は apply の前に落とす。
            let changes = ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let Some(e) = m.get_mut(&mount_id) else {
                    return Vec::new();
                };
                if !e.listening {
                    return Vec::new();
                }
                let latest = e.latest.get();
                let mut out = Vec::new();
                for i in 0..2 {
                    let Some(session) = e.active[i] else { continue };
                    let v = if hidden { None } else { latest[i].map(to_view) };
                    if e.last_sent[i].as_ref().is_some_and(|p| view_same(p, &v)) {
                        continue;
                    }
                    e.last_sent[i] = Some(v);
                    out.push((KINDS[i], session, v));
                }
                out
            });
            if changes.is_empty() {
                return;
            }
            // 全再構築すると「停止」ボタンの click が成立しないため、行の文だけを書き換える。
            tick_applier.apply_in_place(
                move |s| {
                    for (kind, session, v) in changes {
                        apply_reading(s, kind, session, v);
                    }
                },
                patch_rows,
            );
        });
        let vis_applier = applier.clone();
        let visibility = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let hidden = is_hidden();
            // 実際の状態（listening / active）を見て判断する。ENTRIES の borrow は apply の前に落とす。
            enum Next {
                None,
                Cleared(Vec<(RawKindView, u64)>),
                Failed([Option<u64>; 2]),
            }
            let next = ENTRIES.with(|m| {
                let mut m = m.borrow_mut();
                let Some(e) = m.get_mut(&mount_id) else {
                    return Next::None;
                };
                if hidden {
                    if !e.listening {
                        return Next::None;
                    }
                    release_sensing(e);
                    // 古い値を出し続けない。
                    let cleared = (0..2)
                        .filter_map(|i| e.active[i].map(|s| (KINDS[i], s)))
                        .collect();
                    return Next::Cleared(cleared);
                }
                if e.listening || e.active.iter().all(Option::is_none) {
                    return Next::None;
                }
                match web_sys::window().map(|w| add_sensing(e, &w)) {
                    Some(Ok(())) => Next::None,
                    _ => {
                        let want = e.active;
                        teardown(e);
                        Next::Failed(want)
                    }
                }
            });
            match next {
                Next::None => {}
                Next::Cleared(kinds) => vis_applier.apply_in_place(
                    move |s| {
                        for (kind, session) in kinds {
                            apply_reading(s, kind, session, None);
                        }
                    },
                    patch_rows,
                ),
                Next::Failed(want) => fail_all(&vis_applier, want, RawSensorFailure::Failed),
            }
        });
        Entry {
            active: [None; 2],
            mag_failed: None,
            listening: false,
            watching_visibility: false,
            interval: None,
            latest,
            last_sent: [None; 2],
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

    /// `mount_with_effect` へ渡す effect。再描画のたびに呼ばれ、新しい世代が立ち上がったときだけ開始する。
    pub fn motion_raw_sensors_show_effect(state: &State, applier: &Applier<State>) {
        let mount_id = applier.mount_id();
        let want = [
            state
                .angular_velocity
                .wanted
                .then_some(state.angular_velocity.session),
            state
                .linear_acceleration
                .wanted
                .then_some(state.linear_acceleration.session),
        ];
        let mag = state
            .magnetic_field
            .wanted
            .then_some(state.magnetic_field.session);
        let (plan, mag_fail) = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            let e = m
                .entry(mount_id)
                .or_insert_with(|| new_entry(mount_id, applier));
            let mag_fail = mag.filter(|s| e.mag_failed != Some(*s));
            if mag_fail.is_some() {
                e.mag_failed = mag;
            }
            let plan = decide(e.active, want);
            match plan {
                Plan::Stop => {
                    teardown(e);
                    e.active = [None; 2];
                }
                Plan::Keep => e.active = want,
                Plan::Restart => {
                    teardown(e);
                    e.active = want;
                }
            }
            (plan, mag_fail)
        });
        // 磁場は Web で取れない。
        if let Some(session) = mag_fail {
            let v = failure_view(RawSensorFailure::UnsupportedOnOs);
            applier.apply(move |s| apply_failure(s, RawKindView::MagneticField, session, v));
        }
        if plan != Plan::Restart {
            return;
        }
        let Some(w) = web_sys::window() else {
            return fail_all(applier, want, RawSensorFailure::UnsupportedOnOs);
        };
        let has_api = Reflect::has(&w, &JsValue::from_str("DeviceMotionEvent")).unwrap_or(false);
        if !w.is_secure_context() || !has_api {
            return fail_all(applier, want, RawSensorFailure::UnsupportedOnOs);
        }
        match request_permission(&w) {
            None => {
                if register(mount_id).is_err() {
                    fail_all(applier, want, RawSensorFailure::Failed);
                }
            }
            Some(Err(f)) => fail_all(applier, want, f),
            Some(Ok(promise)) => {
                let applier = applier.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let result = JsFuture::from(promise).await;
                    // 再開後: まだ現在の要求か（登録前なので解放物は無い）。
                    let current = is_live(mount_id)
                        && ENTRIES
                            .with(|m| m.borrow().get(&mount_id).is_some_and(|e| e.active == want));
                    if !current {
                        return;
                    }
                    let outcome = match result {
                        Ok(v) => permission_outcome(v.as_string().as_deref()),
                        Err(e) => Err(permission_error(&get_name(&e).unwrap_or_default())),
                    };
                    match outcome {
                        Ok(()) => {
                            if register(mount_id).is_err() {
                                fail_all(&applier, want, RawSensorFailure::Failed);
                            }
                        }
                        Err(f) => fail_all(&applier, want, f),
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

    #[test]
    fn angular_maps_axes() {
        let r = angular_from(Some(3.0), Some(1.0), Some(2.0)).unwrap();
        assert_eq!((r.x, r.y, r.z, r.unit), (1.0, 2.0, 3.0, RawUnit::DegPerSec));
    }

    #[test]
    fn linear_maps_axes() {
        let r = linear_from(Some(1.0), Some(2.0), Some(3.0)).unwrap();
        assert_eq!(
            (r.x, r.y, r.z, r.unit),
            (1.0, 2.0, 3.0, RawUnit::MetersPerSec2)
        );
    }

    #[test]
    fn null_or_non_finite_component_is_no_value() {
        assert_eq!(angular_from(None, Some(1.0), Some(1.0)), None);
        assert_eq!(angular_from(Some(1.0), Some(f64::NAN), Some(1.0)), None);
        assert_eq!(linear_from(Some(1.0), None, Some(1.0)), None);
        assert_eq!(linear_from(Some(1.0), Some(1.0), Some(f64::INFINITY)), None);
    }

    #[test]
    fn permission_mapping() {
        assert_eq!(permission_outcome(Some("granted")), Ok(()));
        assert_eq!(
            permission_outcome(Some("denied")),
            Err(RawSensorFailure::PermissionDenied)
        );
        assert_eq!(permission_outcome(None), Err(RawSensorFailure::Failed));
        assert_eq!(
            permission_outcome(Some("prompt")),
            Err(RawSensorFailure::Failed)
        );
        assert_eq!(
            permission_error("NotAllowedError"),
            RawSensorFailure::PermissionDenied
        );
        assert_eq!(permission_error("TypeError"), RawSensorFailure::Failed);
    }

    #[test]
    fn decide_plans() {
        assert_eq!(decide([Some(1), Some(1)], [None, None]), Plan::Stop);
        assert_eq!(decide([None, None], [None, None]), Plan::Stop);
        // 失敗で片方が落ちるだけなら再開始しない。
        assert_eq!(decide([Some(1), Some(1)], [None, Some(1)]), Plan::Keep);
        assert_eq!(decide([Some(1), Some(1)], [Some(1), Some(1)]), Plan::Keep);
        // 停止からの開始・新しい世代は再開始。
        assert_eq!(decide([None, None], [Some(1), Some(1)]), Plan::Restart);
        assert_eq!(
            decide([Some(1), Some(1)], [Some(2), Some(2)]),
            Plan::Restart
        );
        assert_eq!(decide([None, Some(1)], [Some(1), Some(1)]), Plan::Restart);
    }

    #[test]
    fn view_same_compares_display() {
        let v = |x| {
            Some(to_view(RawReading {
                x,
                y: 0.0,
                z: 0.0,
                unit: RawUnit::DegPerSec,
            }))
        };
        assert!(view_same(&None, &None));
        assert!(view_same(&v(1.0), &v(1.0001)));
        assert!(!view_same(&v(1.0), &v(1.5)));
        assert!(!view_same(&None, &v(1.0)));
    }

    #[test]
    fn failure_mapping_is_one_to_one() {
        assert_eq!(
            failure_view(RawSensorFailure::UnsupportedOnOs),
            RawFailureView::UnsupportedOnOs
        );
        assert_eq!(
            failure_view(RawSensorFailure::PermissionDenied),
            RawFailureView::PermissionDenied
        );
    }
}
