//! 型サンプル「電源状態」の Web 経路（Issue #1277、親 #1128）。
//!
//! `fandhe_core::screens::power_state_show`（端末依存ゼロ）を
//! [`super::mount::mount_with_effect`] の effect として観測し、Battery Status API
//! （`navigator.getBattery()`）の値を `finish_read` へ書き戻す。`fandhe-device` の
//! `PowerState` トレイトは経由しない（`fandhe-device/src/power_state.rs` の module doc どおり、
//! Web は本クレートの担当）。`fandhe-egui-proxy` の wasm canvas 経路は対象外。
//!
//! # 読み取り方
//!
//! `getBattery()` はマウントごとに 1 回だけ呼ぶ。解決後は返ったオブジェクトの
//! `level` / `charging` / `chargingTime` / `dischargingTime` を 1 秒ごとに `Reflect` で読む
//! （画面の状態行「1 秒ごとに読み直しています」と一致させる。変化イベントは購読しない）。
//! 各値は境界で型を検証し、想定外の型の項目は「不明」へ倒す（security.md A03）。
//! タイマーは 1 回ごとの `setTimeout` を自己再予約する。止めるときは次を予約しないだけで、
//! 解除すべきハンドルが残らない。
//!
//! # 嘘の成功を避ける写像
//!
//! 電池状態を報告できない環境は仕様上 `charging=true`・`chargingTime=0`・`level=1`・
//! `dischargingTime=Infinity` を返し、「満充電で AC 接続中」と区別できない。この組は
//! 電池なしとも満充電とも断定せず `BatteryView::Unknown` にする。満充電を明示する手段が
//! 無いため `Full` は出さない。省電力・熱に API は無く、常に不明。
//!
//! # 停止条件
//!
//! マウント破棄（`crate::mount::is_live`）と、恒久的な非対応・失敗では次回を予約しない
//! （失敗表示のまま回し続けない）。失敗で止まったマウントは起動記録を残し、
//! 再描画で effect が再起動しない。`setTimeout` の予約失敗も `Failed` として画面へ出す。bfcache 中はタイマーが凍結され、復帰後に再開して
//! 最新値を読み直すため `pagehide` / `pageshow` は購読しない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の取得、電池なしと満充電の組を偽らないこと、失敗を成功値へ畳まないこと。
//! 守らない: 複数タブ間の一致、変化の反映遅れ（最大 1 秒）、ブラウザによる `level` の丸め。
//!
//! 例外の詳細（`message`）は画面にも `console` にも出さない（security.md A09）。

use fandhe_core::screens::power_state_show::{
    BatteryView, ChargingView, PowerReadError, PowerReading, ThermalView,
};
use fandhe_device::power_state::percent_from_raw;

/// 読み取りに失敗したときの固定文言（例外の `message` は出さない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_FAILED: &str = "電源状態を読み取れませんでした";

/// Battery Status API の 4 項目（生値）を表示用の読み取り結果へ写す純関数。
/// `None` は「型が想定外で読めなかった」項目。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from_raw(
    level: Option<f64>,
    charging: Option<bool>,
    charging_time: Option<f64>,
    discharging_time: Option<f64>,
) -> PowerReading {
    let no_battery_report = charging == Some(true)
        && charging_time == Some(0.0)
        && level == Some(1.0)
        && discharging_time == Some(f64::INFINITY);
    let battery = if no_battery_report {
        BatteryView::Unknown
    } else {
        BatteryView::Present {
            level: level
                .filter(|l| l.is_finite())
                .filter(|l| (0.0..=1.0).contains(l))
                .and_then(|l| percent_from_raw((l * 100.0).round() as i64)),
            charging: match charging {
                Some(true) => ChargingView::Charging,
                Some(false) => ChargingView::Discharging,
                None => ChargingView::Unknown,
            },
        }
    };
    PowerReading {
        battery,
        low_power_mode: None,
        thermal: ThermalView::Unknown,
    }
}

/// `getBattery()` が reject した例外の `name` を失敗種別へ写す純関数。
/// permissions policy などによる恒久的な不許可は非対応、それ以外は固定文言の失敗。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn error_from_name(name: &str) -> PowerReadError {
    match name {
        "NotAllowedError" | "SecurityError" => PowerReadError::Unavailable,
        _ => PowerReadError::Failed(READ_FAILED.to_string()),
    }
}

/// 起動済みマウントの記録。失敗で止まったマウントも残し、再描画での再起動を防ぐ。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Default)]
pub(crate) struct Started(std::collections::HashSet<u64>);

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
impl Started {
    /// 初回のみ true。
    pub(crate) fn try_start(&mut self, id: u64) -> bool {
        self.0.insert(id)
    }
    /// マウント破棄時のみ呼ぶ（失敗では呼ばない）。
    pub(crate) fn forget(&mut self, id: u64) {
        self.0.remove(&id);
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::power_state_show::{
        finish_read, PowerReadError, PowerReading, PowerStateShowState,
    };
    use js_sys::{Function, Reflect};
    use wasm_bindgen::prelude::*;

    use super::{error_from_name, reading_from_raw, Started, READ_FAILED};
    use crate::mount::{is_live, Applier};

    const INTERVAL_MS: i32 = 1000;

    /// `setTimeout` のハンドルと、その発火で呼ばれるクロージャ。
    type PendingTimer = (i32, Closure<dyn FnMut()>);

    thread_local! {
        /// 起動済みのマウント（二重起動の防止。止まったら外す）。
        static STARTED: RefCell<Started> = RefCell::new(Started::default());
        /// 予約中タイマーのクロージャ（マウントごとに 1 本）。次回予約時・停止時・
        /// 予約失敗時に drop して解放する（forget しない）。
        static TIMERS: RefCell<HashMap<u64, PendingTimer>> = RefCell::new(HashMap::new());
    }

    /// マウント破棄時だけ起動記録を外す。失敗で止まった場合は外さない
    /// （外すと延期された再描画で effect が再起動し、失敗を再報告し続ける）。
    fn stop(mount_id: u64) {
        STARTED.with(|s| s.borrow_mut().forget(mount_id));
        drop_timer(mount_id);
    }

    /// 保持中のタイマークロージャを解放する（無ければ何もしない）。
    fn drop_timer(mount_id: u64) {
        // 発火中のクロージャ自身を落としてよい（wasm-bindgen の自己解放の定石）。
        // 予約中ならブラウザ側の予約も解除してから drop する（解除しないと
        // 破棄済みクロージャが約 1 秒後に呼ばれて例外になる）。発火済みなら clear は無害。
        let old = TIMERS.with(|t| t.borrow_mut().remove(&mount_id));
        if let Some((handle, closure)) = old {
            if let Some(w) = web_sys::window() {
                w.clear_timeout_with_handle(handle);
            }
            drop(closure);
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に起動記録を解放する。
    /// 失敗で止まったマウントの記録もここで消え、同じ root への再マウントで蓄積しない。
    pub(crate) fn release_mount(mount_id: u64) {
        stop(mount_id);
    }

    fn get(target: &JsValue, name: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    fn read(battery: &JsValue) -> PowerReading {
        reading_from_raw(
            get(battery, "level").as_f64(),
            get(battery, "charging").as_bool(),
            get(battery, "chargingTime").as_f64(),
            get(battery, "dischargingTime").as_f64(),
        )
    }

    fn report(
        applier: &Applier<PowerStateShowState>,
        result: Result<PowerReading, PowerReadError>,
    ) {
        applier.apply(move |s| finish_read(s, result));
    }

    /// 1 秒ごとの読み取り。前回と同じ結果なら反映せず、次回を予約する。
    fn tick(
        mount_id: u64,
        applier: Applier<PowerStateShowState>,
        battery: JsValue,
        last: Option<PowerReading>,
    ) {
        if !is_live(mount_id) {
            stop(mount_id);
            return;
        }
        let reading = read(&battery);
        if last != Some(reading) {
            report(&applier, Ok(reading));
        }
        let reporter = applier.clone();
        // 予約が成功するまで Drop 可能な Closure を持ち、失敗時は drop して解放する。
        // 成功後は TIMERS に保持し、次回予約・停止で置き換え/解放する（毎 tick の forget はしない）。
        let next = Closure::once(move || tick(mount_id, applier, battery, Some(reading)));
        let handle = web_sys::window().and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                next.as_ref().unchecked_ref(),
                INTERVAL_MS,
            )
            .ok()
        });
        let scheduled = handle.is_some();
        if let Some(handle) = handle {
            // 発火中の自分自身（旧ハンドル）は発火済みなので、置き換えて drop するだけでよい。
            let old = TIMERS.with(|t| t.borrow_mut().insert(mount_id, (handle, next)));
            drop(old);
        } else {
            drop(next);
            drop_timer(mount_id);
        }
        if !scheduled {
            report(
                &reporter,
                Err(PowerReadError::Failed(READ_FAILED.to_string())),
            );
        }
    }

    fn start(mount_id: u64, applier: Applier<PowerStateShowState>) {
        // 失敗時は STARTED を残す（終了済みの印。再描画で再起動させない）。
        let give_up = |applier: &Applier<PowerStateShowState>, error: PowerReadError| {
            report(applier, Err(error));
        };
        let Some(window) = web_sys::window() else {
            return give_up(&applier, PowerReadError::Unavailable);
        };
        let navigator: JsValue = window.navigator().into();
        let get_battery = get(&navigator, "getBattery");
        if !window.is_secure_context() || !get_battery.is_function() {
            return give_up(&applier, PowerReadError::Unavailable);
        }
        let promise = get_battery
            .unchecked_into::<Function>()
            .call0(&navigator)
            .ok()
            .and_then(|p| p.dyn_into::<js_sys::Promise>().ok());
        let Some(promise) = promise else {
            return give_up(&applier, PowerReadError::Failed(READ_FAILED.to_string()));
        };
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(battery) if battery.is_object() => tick(mount_id, applier, battery, None),
                other => {
                    let error = match other {
                        Err(e) => error_from_name(&get(&e, "name").as_string().unwrap_or_default()),
                        Ok(_) => PowerReadError::Failed(READ_FAILED.to_string()),
                    };
                    // ponytail: reject 後は getBattery を再取得しない。一時的な失敗でも
                    // 表示のまま止める（再試行が要るなら再マウント）。
                    if is_live(mount_id) {
                        give_up(&applier, error);
                    } else {
                        stop(mount_id);
                    }
                }
            }
        });
    }

    /// `mount_with_effect` へ渡す effect。マウントごとに 1 度だけ起動する。
    pub fn power_state_show_effect(
        _state: &PowerStateShowState,
        applier: &Applier<PowerStateShowState>,
    ) {
        let mount_id = applier.mount_id();
        if !STARTED.with(|s| s.borrow_mut().try_start(mount_id)) {
            return;
        }
        start(mount_id, applier.clone());
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::power_state_show_effect;

/// マウント破棄時に起動記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    fn present(level: Option<u8>, charging: ChargingView) -> BatteryView {
        BatteryView::Present { level, charging }
    }

    #[test]
    fn discharging_maps_level_to_percent() {
        let r = reading_from_raw(Some(0.42), Some(false), Some(f64::INFINITY), Some(3600.0));
        assert_eq!(r.battery, present(Some(42), ChargingView::Discharging));
        assert_eq!(r.low_power_mode, None);
        assert_eq!(r.thermal, ThermalView::Unknown);
    }

    #[test]
    fn charging_below_full_is_present() {
        let r = reading_from_raw(Some(0.8), Some(true), Some(600.0), Some(f64::INFINITY));
        assert_eq!(r.battery, present(Some(80), ChargingView::Charging));
    }

    #[test]
    fn no_battery_report_is_unknown_not_full() {
        let r = reading_from_raw(Some(1.0), Some(true), Some(0.0), Some(f64::INFINITY));
        assert_eq!(r.battery, BatteryView::Unknown);
    }

    #[test]
    fn invalid_level_becomes_none_only_for_level() {
        for bad in [
            Some(f64::NAN),
            Some(1.5),
            Some(-0.1),
            Some(-0.001),
            Some(1.001),
            None,
        ] {
            let r = reading_from_raw(bad, Some(false), None, None);
            assert_eq!(r.battery, present(None, ChargingView::Discharging));
        }
    }

    #[test]
    fn missing_charging_is_unknown() {
        let r = reading_from_raw(Some(0.5), None, None, None);
        assert_eq!(r.battery, present(Some(50), ChargingView::Unknown));
    }

    #[test]
    fn never_reports_full() {
        let r = reading_from_raw(Some(1.0), Some(true), Some(0.0), Some(10.0));
        assert_eq!(r.battery, present(Some(100), ChargingView::Charging));
    }

    #[test]
    fn failed_mount_is_not_restarted_by_rerender() {
        let mut s = Started::default();
        assert!(s.try_start(1));
        // 失敗しても forget しない: 再描画での再起動は拒否される。
        assert!(!s.try_start(1));
        s.forget(1);
        assert!(s.try_start(1));
    }

    #[test]
    fn error_names_are_classified() {
        assert_eq!(
            error_from_name("NotAllowedError"),
            PowerReadError::Unavailable
        );
        assert_eq!(
            error_from_name("SecurityError"),
            PowerReadError::Unavailable
        );
        let failed = PowerReadError::Failed(READ_FAILED.to_string());
        assert_eq!(error_from_name("UnknownError"), failed);
        assert_eq!(error_from_name(""), failed);
    }
}
