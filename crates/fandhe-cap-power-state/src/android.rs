//! Android 向け電源状態実装（Issue #1276）。`driver.rs` の Android 経路から
//! egui スレッドで 1 秒ごとに同期で呼ばれる。契約は `crate::contract`。
//!
//! Java 側 `PowerStateHelper.read(Context)`（static。JNI シグネチャ `(Landroid/content/Context;)J`）が5 項目（電池の有無・残量・充電状態・省電力・熱）を 1 つの `long` に詰めて返す。
//! 詰め方の SSOT は `PowerStateHelper.java` 冒頭の表。1 回の呼び出しで全項目を読むので、
//! 項目間は同じ時点の値になる。OS 定数 -> 契約の値の写像は純関数 [`from_android`] に置き、
//! ホストでもテストする（#1257 の反省）。
//!
//! - `android_app()` が無い（Activity 準備前）ときは `Internal` を返す。`Unsupported` にすると
//!   effect ドライバが恒久停止し、起動直後の一時状態で画面が固まるため（`AndroidHaptics` と同判断）
//! - JNI 失敗・Java が返した負値（失敗）も `Internal`。成功値へは畳まない
//! - 保留中の要求・コールバック・購読を持たないので、Activity 再生成時に破棄すべきものは無い
//!
//! 一次情報:
//! <https://developer.android.com/reference/android/os/BatteryManager>、
//! <https://developer.android.com/reference/android/os/PowerManager>

use crate::contract::{Battery, Charging, PowerSnapshot, Thermal, percent_from_raw};

/// Java が返した `long` を (有無, 残量, 充電状態, 省電力, 熱) へ分解する。負値は失敗で `None`。
fn unpack(raw: i64) -> Option<(u8, u8, u8, u8, u8)> {
    if raw < 0 {
        return None;
    }
    let b = |shift: u32| ((raw >> shift) & 0xff) as u8;
    Some((b(0), b(8), b(16), b(24), b(32)))
}

/// `PowerStateHelper` の生値を契約の値へ写す。不明の番兵は 255（範囲外として `_` で拾う）。
fn from_android(present: u8, percent: u8, status: u8, low_power: u8, thermal: u8) -> PowerSnapshot {
    let battery = match present {
        0 => Battery::Absent,
        1 => Battery::Present {
            level: percent_from_raw(i64::from(percent)),
            // BatteryManager.BATTERY_STATUS_*: 2 CHARGING / 3 DISCHARGING / 5 FULL。
            // 4 NOT_CHARGING（接続中だが充電していない）と 1 UNKNOWN は放電中と偽らず不明。
            charging: match status {
                2 => Charging::Charging,
                3 => Charging::Discharging,
                5 => Charging::Full,
                _ => Charging::Unknown,
            },
        },
        _ => Battery::Unknown,
    };
    PowerSnapshot {
        battery,
        low_power_mode: match low_power {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        // PowerManager.THERMAL_STATUS_*: 0 NONE / 1 LIGHT / 2 MODERATE / 3 SEVERE /
        // 4 CRITICAL / 5 EMERGENCY / 6 SHUTDOWN。
        thermal: match thermal {
            0 => Thermal::Nominal,
            1 | 2 => Thermal::Fair,
            3 => Thermal::Serious,
            4..=6 => Thermal::Critical,
            _ => Thermal::Unknown,
        },
    }
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::{from_android, unpack};
    use crate::contract::{PowerSnapshot, PowerState};
    use crate::error::PowerStateError;
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};

    /// Android の `PowerState` 実装（`PowerStateHelper.read(Context)` を static 呼び出しで読む）。
    pub(super) struct AndroidPowerState;

    impl PowerState for AndroidPowerState {
        fn read(&self) -> Result<PowerSnapshot, PowerStateError> {
            const FAILED: PowerStateError = PowerStateError::Internal;
            let Some(app) = android_app() else {
                return Err(FAILED);
            };
            let result = with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.PowerStateHelper"),
                |env, activity, class| -> Result<i64, JniCallFailed> {
                    let value = env.call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.content.Context) -> long),
                        &[jni::objects::JValue::Object(activity)],
                    )?;
                    Ok(value.j()?)
                },
            );

            let (present, percent, status, low_power, thermal) =
                result.ok().and_then(unpack).ok_or(FAILED)?;
            Ok(from_android(present, percent, status, low_power, thermal))
        }
    }
}

/// Android の `PowerState` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn crate::contract::PowerState> {
    Box::new(jni_impl::AndroidPowerState)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(p: u64, l: u64, s: u64, lp: u64, t: u64) -> i64 {
        (p | l << 8 | s << 16 | lp << 24 | t << 32) as i64
    }

    #[test]
    fn unpack_fields_and_failure() {
        assert_eq!(unpack(-1), None);
        assert_eq!(unpack(pack(1, 55, 2, 1, 3)), Some((1, 55, 2, 1, 3)));
    }

    #[test]
    fn absent_ignores_other_fields_and_unknown_is_distinct() {
        assert_eq!(from_android(0, 0, 3, 0, 0).battery, Battery::Absent);
        assert_eq!(from_android(255, 50, 2, 0, 0).battery, Battery::Unknown);
    }

    #[test]
    fn percent_mapping() {
        let lv = |p| match from_android(1, p, 3, 0, 0).battery {
            Battery::Present { level, .. } => level,
            _ => panic!(),
        };
        assert_eq!(lv(255), None);
        assert_eq!(lv(0), Some(0));
        assert_eq!(lv(100), Some(100));
        assert_eq!(lv(101), None);
    }

    #[test]
    fn charging_mapping() {
        let ch = |s| match from_android(1, 50, s, 0, 0).battery {
            Battery::Present { charging, .. } => charging,
            _ => panic!(),
        };
        assert_eq!(ch(2), Charging::Charging);
        assert_eq!(ch(3), Charging::Discharging);
        assert_eq!(ch(5), Charging::Full);
        for s in [1, 4, 255] {
            assert_eq!(ch(s), Charging::Unknown);
        }
    }

    #[test]
    fn low_power_and_thermal_mapping() {
        let lp = |v| from_android(1, 50, 3, v, 0).low_power_mode;
        assert_eq!((lp(0), lp(1), lp(255)), (Some(false), Some(true), None));
        let th = |v| from_android(1, 50, 3, 0, v).thermal;
        assert_eq!(th(0), Thermal::Nominal);
        assert_eq!((th(1), th(2)), (Thermal::Fair, Thermal::Fair));
        assert_eq!(th(3), Thermal::Serious);
        for v in 4..=6 {
            assert_eq!(th(v), Thermal::Critical);
        }
        assert_eq!((th(7), th(255)), (Thermal::Unknown, Thermal::Unknown));
    }
}
