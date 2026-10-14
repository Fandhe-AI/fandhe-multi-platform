//! Phase 11 の拡充ループ（親 #1128）型サンプル「電源状態」（Issue #1274）の effect ドライバ。
//! `fandhe_device::power_state::PowerState` を 1 秒間隔で UI スレッドから同期 pull し、
//! `fandhe_core::screens::power_state_show`（端末依存ゼロ）へ結果を `CompletionSender` 経由で
//! 渡す（`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::power_state`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_power_state`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を
//! 持つモジュールだけを埋める（macOS: `macos_power_state`、Windows: `windows_power_state`、
//! iOS: `ios_power_state`（#1275）、Android: `android_power_state`（#1276））。それ以外は
//! `UnsupportedPowerState` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 再描画の予約（契約表 4）
//!
//! - 間隔未満・結果が前回と同じ・読み取り成功・一時失敗: どれも次回読み取り時刻に再描画を予約する
//!   （入力が無いと egui が描画せず、pull が止まるため）
//! - 非対応（`Unsupported`）だけは予約しない。恒久的で回復しないため、失敗状態のまま再描画を
//!   回し続けない。一時失敗は 1Hz なので再描画嵐にならず、回復を待つために予約を続ける

use std::sync::Mutex;
use std::time::{Duration, Instant};

use fandhe_core::screens::power_state_show::{
    finish_read, view, BatteryView, ChargingView, PowerReadError, PowerReading,
    PowerStateShowState, PowerView, ThermalView,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::power_state::UnsupportedPowerState;
use fandhe_device::power_state::{Battery, Charging, PowerSnapshot, PowerState, Thermal};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// OS を読み直す間隔。
const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 直近の読み取り時刻。
/// ponytail: プロセス内 1 画面前提のグローバル。複数 Context が要るなら Context 単位へ移す。
static LAST_READ: Mutex<Option<Instant>> = Mutex::new(None);

/// 現在のプラットフォームに応じた `PowerState` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_power_state() -> Box<dyn PowerState> {
    #[cfg(target_os = "windows")]
    {
        super::windows_power_state::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_power_state::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_power_state::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_power_state::implementation()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Box::new(UnsupportedPowerState)
    }
}

/// device の値を画面用の値へ写す。
fn to_reading(s: PowerSnapshot) -> PowerReading {
    let battery = match s.battery {
        Battery::Absent => BatteryView::Absent,
        Battery::Unknown => BatteryView::Unknown,
        Battery::Present { level, charging } => BatteryView::Present {
            level,
            charging: match charging {
                Charging::Charging => ChargingView::Charging,
                Charging::Discharging => ChargingView::Discharging,
                Charging::Full => ChargingView::Full,
                Charging::Unknown => ChargingView::Unknown,
            },
        },
    };
    let thermal = match s.thermal {
        Thermal::Nominal => ThermalView::Nominal,
        Thermal::Fair => ThermalView::Fair,
        Thermal::Serious => ThermalView::Serious,
        Thermal::Critical => ThermalView::Critical,
        Thermal::Unknown => ThermalView::Unknown,
    };
    PowerReading {
        battery,
        low_power_mode: s.low_power_mode,
        thermal,
    }
}

/// 1 回分の処理。戻り値は予約した再描画の遅延（テスト用。予約しなければ `None`）。
fn drive(
    state: &PowerStateShowState,
    sender: &CompletionSender<PowerStateShowState>,
    ps: &dyn PowerState,
    now: Instant,
    last: &mut Option<Instant>,
) -> Option<Duration> {
    // 非対応が確定済みなら間隔判定より先に止める（再描画・再読み取りを予約しない）。
    if matches!(view(state), PowerView::Unavailable) {
        return None;
    }
    if let Some(prev) = *last {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            let remaining = READ_INTERVAL - elapsed;
            sender.request_repaint_after(remaining);
            return Some(remaining);
        }
    }
    *last = Some(now);

    let result = match ps.read() {
        Ok(snapshot) => Ok(to_reading(snapshot)),
        Err(DeviceError::Unsupported(_)) => Err(PowerReadError::Unavailable),
        // 画面へは機能名程度の Display のみ（OS の生コードは出さない、security.md A09）。
        Err(e) => Err(PowerReadError::Failed(e.to_string())),
    };
    let unchanged = match (&result, view(state)) {
        (Ok(r), PowerView::Read(prev)) => r == prev,
        (Err(PowerReadError::Unavailable), PowerView::Unavailable) => true,
        (Err(PowerReadError::Failed(m)), PowerView::Failed(prev)) => m == prev,
        _ => false,
    };
    if !unchanged {
        // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
        let r = result.clone();
        let _ = sender.send(Box::new(move |s: &mut PowerStateShowState| {
            finish_read(s, r)
        }));
    }
    if matches!(result, Err(PowerReadError::Unavailable)) {
        return None;
    }
    sender.request_repaint_after(READ_INTERVAL);
    Some(READ_INTERVAL)
}

/// `registry.rs` の `effect` 区画から `power_state_show` 画面用に配線される effect ドライバ。
pub fn power_state_show_effect(
    state: &PowerStateShowState,
    sender: &CompletionSender<PowerStateShowState>,
) {
    let mut last = LAST_READ
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        platform_power_state().as_ref(),
        Instant::now(),
        &mut last,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mock {
        count: AtomicUsize,
        f: Box<dyn Fn() -> Result<PowerSnapshot, DeviceError> + Send + Sync>,
    }

    impl PowerState for Mock {
        fn read(&self) -> Result<PowerSnapshot, DeviceError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            (self.f)()
        }
    }

    fn mock(f: impl Fn() -> Result<PowerSnapshot, DeviceError> + Send + Sync + 'static) -> Mock {
        Mock {
            count: AtomicUsize::new(0),
            f: Box::new(f),
        }
    }

    fn snap() -> PowerSnapshot {
        PowerSnapshot {
            battery: Battery::Absent,
            low_power_mode: Some(false),
            thermal: Thermal::Nominal,
        }
    }

    fn channel() -> (
        CompletionSender<PowerStateShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<PowerStateShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply(
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<PowerStateShowState>>,
    ) -> PowerStateShowState {
        let mut s = PowerStateShowState::default();
        (rx.recv().expect("完了が送られる"))(&mut s);
        s
    }

    #[test]
    fn first_read_sends_and_schedules() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(snap()));
        let mut last = None;
        let d = drive(
            &PowerStateShowState::default(),
            &sender,
            &ps,
            Instant::now(),
            &mut last,
        );
        assert_eq!(d, Some(READ_INTERVAL));
        assert!(matches!(view(&apply(&rx)), PowerView::Read(_)));
    }

    #[test]
    fn within_interval_does_not_read_but_schedules_remaining() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(snap()));
        let t0 = Instant::now();
        let mut last = Some(t0);
        let d = drive(
            &PowerStateShowState::default(),
            &sender,
            &ps,
            t0 + Duration::from_millis(400),
            &mut last,
        );
        assert_eq!(d, Some(Duration::from_millis(600)));
        assert_eq!(ps.count.load(Ordering::SeqCst), 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn same_result_is_not_sent_but_repaint_is_scheduled() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(snap()));
        let mut state = PowerStateShowState::default();
        finish_read(&mut state, Ok(to_reading(snap())));
        let mut last = None;
        let d = drive(&state, &sender, &ps, Instant::now(), &mut last);
        assert_eq!(d, Some(READ_INTERVAL));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unsupported_sends_unavailable_and_stops_repaint() {
        let (sender, rx) = channel();
        let ps = mock(|| Err(DeviceError::Unsupported("power_state")));
        let mut last = None;
        let d = drive(
            &PowerStateShowState::default(),
            &sender,
            &ps,
            Instant::now(),
            &mut last,
        );
        assert_eq!(d, None);
        assert_eq!(view(&apply(&rx)), &PowerView::Unavailable);
    }

    #[test]
    fn unavailable_state_stops_before_interval_check() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(snap()));
        let mut state = PowerStateShowState::default();
        finish_read(&mut state, Err(PowerReadError::Unavailable));
        let t0 = Instant::now();
        let mut last = Some(t0);
        let d = drive(
            &state,
            &sender,
            &ps,
            t0 + Duration::from_millis(400),
            &mut last,
        );
        assert_eq!(d, None);
        assert_eq!(ps.count.load(Ordering::SeqCst), 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn internal_error_sends_failed_and_keeps_polling() {
        let (sender, rx) = channel();
        let ps = mock(|| Err(DeviceError::Internal("power_state read")));
        let mut last = None;
        let d = drive(
            &PowerStateShowState::default(),
            &sender,
            &ps,
            Instant::now(),
            &mut last,
        );
        assert_eq!(d, Some(READ_INTERVAL));
        assert!(matches!(view(&apply(&rx)), PowerView::Failed(_)));
    }

    #[test]
    fn to_reading_maps_every_variant() {
        for (c, v) in [
            (Charging::Charging, ChargingView::Charging),
            (Charging::Discharging, ChargingView::Discharging),
            (Charging::Full, ChargingView::Full),
            (Charging::Unknown, ChargingView::Unknown),
        ] {
            let r = to_reading(PowerSnapshot {
                battery: Battery::Present {
                    level: Some(5),
                    charging: c,
                },
                low_power_mode: Some(true),
                thermal: Thermal::Unknown,
            });
            assert_eq!(
                r.battery,
                BatteryView::Present {
                    level: Some(5),
                    charging: v
                }
            );
            assert_eq!(r.low_power_mode, Some(true));
        }
        for (t, v) in [
            (Thermal::Nominal, ThermalView::Nominal),
            (Thermal::Fair, ThermalView::Fair),
            (Thermal::Serious, ThermalView::Serious),
            (Thermal::Critical, ThermalView::Critical),
            (Thermal::Unknown, ThermalView::Unknown),
        ] {
            let mut s = snap();
            s.thermal = t;
            assert_eq!(to_reading(s).thermal, v);
        }
        let mut s = snap();
        s.battery = Battery::Unknown;
        assert_eq!(to_reading(s).battery, BatteryView::Unknown);
    }
}
