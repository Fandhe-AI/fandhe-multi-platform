//! iOS 向け電源状態実装（Issue #1275）。`ios/fandhe_ios_power_state.m`（`UIDevice` +
//! `NSProcessInfo`、`build.rs` が iOS ターゲットの feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 1 関数で呼ぶ。#1274 のスタブを置き換えたもので、dispatch
//! （`device/power_state.rs`）と共通コア画面は変えない。契約は `fandhe_device::power_state`。
//!
//! `.m` は OS の生値を返すだけで、値域チェックと契約値への写像は [`from_raw`] が行う
//! （ホストでテストできるようモジュールは `cfg(any(ios, test))`）。`UIDevice` は UIKit のため
//! main スレッド限定で、main 以外は `Internal` を返す。
//!
//! # 再描画
//!
//! OS コールバックを持たず結果を共有状態へ書く経路が無いため、`crate::ios::request_repaint` は
//! 呼ばない。再描画は `drive` の完了通知と 1 秒間隔の再描画予約が起こす。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 1 プロセス内の取得、電池不明（シミュレータ等）を 0% / 電池なしへ倒さないこと、
//!   `-1.0` 番兵・NaN・範囲外を `None` / Unknown へ落とすこと、失敗を成功値へ畳まないこと
//! - 守らない: OS 間の値の一致、変化の反映遅れ（読み取り間隔 1 秒ぶん）、複数プロセス
//!
//! # 既知の天井
//!
//! - 変化通知は購読せず 1 秒 pull のみ（購読は trait 契約の変更で先行 docs PR が要る）
//! - `batteryMonitoringEnabled` は有効化して戻さない（他に使う箇所が無い）。有効化直後の
//!   1 回目は Unknown になりうる
//! - シミュレータは電池を不明で返す
//! - 実機の目視確認は #1271

use fandhe_device::power_state::{percent_from_raw, Battery, Charging, PowerSnapshot, Thermal};

/// `.m` の `FandheIosPowerStateRaw` と同じレイアウト。
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Raw {
    battery_state: i32,
    battery_level: f32,
    low_power: i32,
    thermal: i32,
}

/// `.m` の生値を契約の値へ写す。
fn from_raw(raw: Raw) -> PowerSnapshot {
    // 有限値かつ 0.0..=1.0 のみ採用（`as i64` は NaN を 0 にするため先に弾く）。
    let level = if raw.battery_level.is_finite() && (0.0..=1.0).contains(&raw.battery_level) {
        percent_from_raw((raw.battery_level * 100.0).round() as i64)
    } else {
        None
    };
    let battery = match raw.battery_state {
        1 => Battery::Present {
            level,
            charging: Charging::Discharging,
        },
        2 => Battery::Present {
            level,
            charging: Charging::Charging,
        },
        3 => Battery::Present {
            level,
            charging: Charging::Full,
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

#[cfg(target_os = "ios")]
mod ffi {
    use super::Raw;

    // SAFETY: `ios/fandhe_ios_power_state.m`（`build.rs` が iOS ターゲットの feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_power_state_read(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_ios_power_state_read(out) }
    }
}

#[cfg(target_os = "ios")]
struct IosPowerState;

#[cfg(target_os = "ios")]
impl fandhe_device::power_state::PowerState for IosPowerState {
    fn read(&self) -> Result<PowerSnapshot, fandhe_device::DeviceError> {
        let mut raw = Raw::default();
        if ffi::read(&mut raw) != 0 {
            return Err(fandhe_device::DeviceError::Internal("power_state read"));
        }
        Ok(from_raw(raw))
    }
}

/// iOS の `PowerState` 実装を返す（`device/power_state.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::power_state::PowerState> {
    Box::new(IosPowerState)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(battery_state: i32, battery_level: f32, low_power: i32, thermal: i32) -> Raw {
        Raw {
            battery_state,
            battery_level,
            low_power,
            thermal,
        }
    }

    fn level_of(l: f32) -> Option<u8> {
        match from_raw(raw(1, l, 0, 0)).battery {
            Battery::Present { level, .. } => level,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn battery_state_codes() {
        for (code, want) in [
            (1, Charging::Discharging),
            (2, Charging::Charging),
            (3, Charging::Full),
        ] {
            assert_eq!(
                from_raw(raw(code, 0.5, 0, 0)).battery,
                Battery::Present {
                    level: Some(50),
                    charging: want
                }
            );
        }
        for code in [0, -1, 9] {
            assert_eq!(from_raw(raw(code, 0.5, 0, 0)).battery, Battery::Unknown);
        }
    }

    #[test]
    fn level_sentinel_and_invalid_are_none() {
        for l in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.5] {
            assert_eq!(level_of(l), None);
        }
    }

    #[test]
    fn level_values_and_rounding() {
        assert_eq!(level_of(0.0), Some(0));
        assert_eq!(level_of(0.5), Some(50));
        assert_eq!(level_of(1.0), Some(100));
        assert_eq!(level_of(0.004), Some(0));
        assert_eq!(level_of(0.006), Some(1));
    }

    #[test]
    fn low_power_and_thermal() {
        assert_eq!(from_raw(raw(0, 0.0, 0, 0)).low_power_mode, Some(false));
        assert_eq!(from_raw(raw(0, 0.0, 1, 0)).low_power_mode, Some(true));
        assert_eq!(from_raw(raw(0, 0.0, 2, 0)).low_power_mode, None);
        for (code, want) in [
            (0, Thermal::Nominal),
            (1, Thermal::Fair),
            (2, Thermal::Serious),
            (3, Thermal::Critical),
            (4, Thermal::Unknown),
            (-1, Thermal::Unknown),
        ] {
            assert_eq!(from_raw(raw(0, 0.0, 0, code)).thermal, want);
        }
    }
}
