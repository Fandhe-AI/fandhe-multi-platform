//! Windows 向け電源状態実装（Issue #1274）。`device/power_state.rs` の Windows 経路から呼ばれ、
//! Win32 `GetSystemPowerStatus` を UI スレッドから同期で 1 回呼ぶ（数 µs。package identity 不要）。
//! `PowerState` の契約は `fandhe_device::power_state`。
//!
//! 値の写像は純関数 [`from_win32`] に切り出し、ホストでもテストする（Windows でしか走らない
//! 写像テストを作らない、#1257 の反省）。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-system_power_status>
//!
//! # 既知の天井
//!
//! - 熱状態は取る API が無いため常に `Unknown`
//! - 実機確認は #1271。ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use fandhe_device::power_state::{percent_from_raw, Battery, Charging, PowerSnapshot, Thermal};

/// `BatteryFlag`: 充電中ビット。
const FLAG_CHARGING: u8 = 8;
/// `BatteryFlag`: 電池なしビット。
const FLAG_NO_BATTERY: u8 = 128;
/// `BatteryFlag` / `BatteryLifePercent` / `ACLineStatus` の「不明」番兵。
const UNKNOWN: u8 = 255;

/// `SYSTEM_POWER_STATUS` の 4 フィールドを契約の値へ写す。
fn from_win32(ac_line: u8, battery_flag: u8, percent: u8, system_status_flag: u8) -> PowerSnapshot {
    let battery = if battery_flag == UNKNOWN {
        Battery::Unknown
    } else if battery_flag & FLAG_NO_BATTERY != 0 {
        Battery::Absent
    } else {
        let level = percent_from_raw(i64::from(percent));
        // 満充電は「充電中ビットかつ 100%」のときだけ。AC 接続で充電ビット無しは放電中と偽らない。
        let charging = if battery_flag & FLAG_CHARGING != 0 {
            if level == Some(100) {
                Charging::Full
            } else {
                Charging::Charging
            }
        } else if ac_line == 0 {
            Charging::Discharging
        } else {
            Charging::Unknown
        };
        Battery::Present { level, charging }
    };
    PowerSnapshot {
        battery,
        low_power_mode: match system_status_flag {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        thermal: Thermal::Unknown,
    }
}

#[cfg(target_os = "windows")]
struct WindowsPowerState;

#[cfg(target_os = "windows")]
impl fandhe_device::power_state::PowerState for WindowsPowerState {
    #[allow(unsafe_code)]
    fn read(&self) -> Result<PowerSnapshot, fandhe_device::DeviceError> {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut s = SYSTEM_POWER_STATUS::default();
        // SAFETY: `s` は呼び出しの間有効なスタック上の構造体で、排他的な書き込み先として渡す。
        // 関数は保持しない。
        unsafe { GetSystemPowerStatus(&mut s) }
            .map_err(|_| fandhe_device::DeviceError::Internal("power_state read"))?;
        Ok(from_win32(
            s.ACLineStatus,
            s.BatteryFlag,
            s.BatteryLifePercent,
            s.SystemStatusFlag,
        ))
    }
}

/// Windows の `PowerState` 実装を返す（`device/power_state.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::power_state::PowerState> {
    Box::new(WindowsPowerState)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(s: PowerSnapshot) -> (Option<u8>, Charging) {
        match s.battery {
            Battery::Present { level, charging } => (level, charging),
            other => panic!("Present のはず: {other:?}"),
        }
    }

    #[test]
    fn flag_255_is_unknown_and_128_is_absent() {
        assert_eq!(from_win32(1, 255, 50, 0).battery, Battery::Unknown);
        assert_eq!(from_win32(1, 128, 255, 0).battery, Battery::Absent);
    }

    #[test]
    fn percent_255_is_none_not_zero() {
        assert_eq!(present(from_win32(0, 1, 255, 0)).0, None);
        assert_eq!(present(from_win32(0, 1, 0, 0)).0, Some(0));
    }

    #[test]
    fn charging_bit_with_full_and_partial() {
        assert_eq!(present(from_win32(1, 8, 100, 0)).1, Charging::Full);
        assert_eq!(present(from_win32(1, 8, 99, 0)).1, Charging::Charging);
    }

    #[test]
    fn ac_offline_is_discharging_but_ac_online_without_bit_is_unknown() {
        assert_eq!(present(from_win32(0, 1, 50, 0)).1, Charging::Discharging);
        assert_eq!(present(from_win32(1, 1, 50, 0)).1, Charging::Unknown);
        assert_eq!(present(from_win32(255, 1, 50, 0)).1, Charging::Unknown);
    }

    #[test]
    fn system_status_flag_maps_battery_saver() {
        assert_eq!(from_win32(1, 1, 50, 0).low_power_mode, Some(false));
        assert_eq!(from_win32(1, 1, 50, 1).low_power_mode, Some(true));
        assert_eq!(from_win32(1, 1, 50, 2).low_power_mode, None);
    }

    #[test]
    fn thermal_is_always_unknown() {
        assert_eq!(from_win32(1, 1, 50, 0).thermal, Thermal::Unknown);
    }
}
