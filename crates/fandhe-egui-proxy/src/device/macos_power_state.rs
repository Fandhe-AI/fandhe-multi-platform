//! macOS 向け電源状態実装（Issue #1274）。`macos/fandhe_macos_power_state.m`（IOKit +
//! `NSProcessInfo`、`build.rs` が macOS ターゲットの feature `device` 有効時にコンパイル・
//! リンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で
//! 契約を明記する。`device/power_state.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::power_state`。
//!
//! `.m` が返す生値（番兵 -1 など）は [`from_raw`] で契約の値へ写す。範囲外の値
//! （`level > 100`、`thermal > 3`、未知の充電コード）は FFI 境界の入力として Unknown / `None` へ
//! 落とす。写像はホストでもテストできるようモジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 変化通知（低電力モード・熱・スリープ復帰）は購読せず 1 秒 pull のみ
//! - 実機の目視確認は #1271

use fandhe_device::power_state::{percent_from_raw, Battery, Charging, PowerSnapshot, Thermal};

/// `.m` の `FandhePowerStateRaw` と同じレイアウト。
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Raw {
    battery: i32,
    level: i32,
    charging: i32,
    low_power: i32,
    thermal: i32,
}

/// `.m` の生値を契約の値へ写す。
fn from_raw(raw: Raw) -> PowerSnapshot {
    let battery = match raw.battery {
        0 => Battery::Absent,
        1 => Battery::Present {
            level: percent_from_raw(i64::from(raw.level)),
            charging: match raw.charging {
                1 => Charging::Charging,
                2 => Charging::Discharging,
                3 => Charging::Full,
                _ => Charging::Unknown,
            },
        },
        _ => Battery::Unknown,
    };
    PowerSnapshot {
        battery,
        low_power_mode: match raw.low_power {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        thermal: match raw.thermal {
            0 => Thermal::Nominal,
            1 => Thermal::Fair,
            2 => Thermal::Serious,
            3 => Thermal::Critical,
            _ => Thermal::Unknown,
        },
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_power_state.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイル・リンクする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_power_state_read(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_power_state_read(out) }
    }
}

#[cfg(target_os = "macos")]
struct MacPowerState;

#[cfg(target_os = "macos")]
impl fandhe_device::power_state::PowerState for MacPowerState {
    fn read(&self) -> Result<PowerSnapshot, fandhe_device::DeviceError> {
        let mut raw = Raw::default();
        if ffi::read(&mut raw) != 0 {
            return Err(fandhe_device::DeviceError::Internal("power_state read"));
        }
        Ok(from_raw(raw))
    }
}

/// macOS の `PowerState` 実装を返す（`device/power_state.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::power_state::PowerState> {
    Box::new(MacPowerState)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(battery: i32, level: i32, charging: i32, low_power: i32, thermal: i32) -> Raw {
        Raw {
            battery,
            level,
            charging,
            low_power,
            thermal,
        }
    }

    /// 実 OS を 1 回読めること（リンクと FFI の疎通。値は機種依存のため成功だけ見る）。
    #[cfg(target_os = "macos")]
    #[test]
    fn real_read_succeeds() {
        assert!(implementation().read().is_ok());
    }

    #[test]
    fn battery_absent_and_unknown() {
        assert_eq!(from_raw(raw(0, -1, 0, 0, 0)).battery, Battery::Absent);
        assert_eq!(from_raw(raw(-1, -1, 0, 0, 0)).battery, Battery::Unknown);
        assert_eq!(from_raw(raw(7, 50, 1, 0, 0)).battery, Battery::Unknown);
    }

    #[test]
    fn out_of_range_level_is_none_not_zero() {
        for level in [-1, 101, 1000] {
            assert_eq!(
                from_raw(raw(1, level, 1, 0, 0)).battery,
                Battery::Present {
                    level: None,
                    charging: Charging::Charging
                }
            );
        }
    }

    #[test]
    fn charging_codes() {
        for (code, want) in [
            (1, Charging::Charging),
            (2, Charging::Discharging),
            (3, Charging::Full),
            (0, Charging::Unknown),
            (9, Charging::Unknown),
        ] {
            assert_eq!(
                from_raw(raw(1, 50, code, 0, 0)).battery,
                Battery::Present {
                    level: Some(50),
                    charging: want
                }
            );
        }
    }

    #[test]
    fn thermal_mapping_and_range() {
        for (code, want) in [
            (0, Thermal::Nominal),
            (1, Thermal::Fair),
            (2, Thermal::Serious),
            (3, Thermal::Critical),
            (4, Thermal::Unknown),
            (-1, Thermal::Unknown),
        ] {
            assert_eq!(from_raw(raw(0, -1, 0, 0, code)).thermal, want);
        }
    }

    #[test]
    fn low_power_mapping() {
        assert_eq!(from_raw(raw(0, -1, 0, 1, 0)).low_power_mode, Some(true));
        assert_eq!(from_raw(raw(0, -1, 0, 0, 0)).low_power_mode, Some(false));
        assert_eq!(from_raw(raw(0, -1, 0, -1, 0)).low_power_mode, None);
    }
}
