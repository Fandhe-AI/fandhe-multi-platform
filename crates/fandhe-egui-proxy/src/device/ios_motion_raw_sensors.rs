//! iOS 向けジャイロ・磁気・線形加速度の生値実装（Issue #900、親 #180）。
//! `ios/fandhe_ios_motion_raw_sensors.m`（CoreMotion `CMMotionManager`、`build.rs` が iOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 3 関数で呼ぶ。#898 のスタブを
//! 置き換えたもので、dispatch（`device/motion_raw_sensors.rs`）・共通コア画面・trait 契約
//! （`fandhe_device::motion_raw_sensors`）は変えない。
//!
//! 角速度は rad/s、磁場は µT、線形加速度は G を、OS が返した単位と軸のまま渡す。`.m` は生の double と
//! 状態コードを返すだけで、契約値への写像と有限性の検査は [`start_result`] / [`read_result`] が行う
//! （ホストでテストできるよう `cfg(any(ios, test))`）。
//!
//! # 再描画・ライフサイクル
//!
//! `.m` が値・失敗を確定した直後（ジャイロ・磁気の handler、前景復帰）に
//! `fandhe_rust_motion_raw_sensors_changed` を呼び、`crate::ios::request_repaint` で次フレームを起こす。
//! 背景遷移ではジャイロと磁気の購読を止めて値を捨て、前景復帰で開始したままの分だけ張り直す。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 非搭載（シミュレータ含む）を `NotPresent` にする、拒否・失敗・非有限値を成功値へ畳まない、
//!   種類ごとに二重購読しない、停止と背景遷移で保持値を捨てる、古い世代の通知を捨てる
//! - 守らない: 絶対精度と較正、サンプリング周期と表示の遅れ、複数プロセス
//!
//! # 既知の天井
//!
//! - シミュレータでは 3 種とも `NotPresent` になる
//! - deviceMotion は tilt と共有していて止めないので、線形加速度の停止後も OS のセンサーは動き続ける
//!   （値は返さない）
//! - 実際の配信周期は実機で確認する（#885）

#[cfg(any(target_os = "ios", test))]
use fandhe_device::motion_raw_sensors::{
    validate_reading, RawKind, RawReading, RawSensorFailure, RawUnit,
};

/// `.m` の kind コード（0 角速度 / 1 磁場 / 2 線形加速度）。
#[cfg(any(target_os = "ios", test))]
fn kind_code(kind: RawKind) -> i32 {
    match kind {
        RawKind::AngularVelocity => 0,
        RawKind::MagneticField => 1,
        RawKind::LinearAcceleration => 2,
    }
}

/// OS が返す単位（角速度 rad/s・磁場 µT・線形加速度 G）。
#[cfg(any(target_os = "ios", test))]
fn unit_for(kind: RawKind) -> RawUnit {
    match kind {
        RawKind::AngularVelocity => RawUnit::RadPerSec,
        RawKind::MagneticField => RawUnit::MicroTesla,
        RawKind::LinearAcceleration => RawUnit::StandardGravity,
    }
}

/// `.m` の start の戻り値を契約へ写す（0 成功 / 1 非搭載 / 2 権限拒否 / 他 失敗）。
#[cfg(any(target_os = "ios", test))]
fn start_result(code: i32) -> Result<(), RawSensorFailure> {
    match code {
        0 => Ok(()),
        1 => Err(RawSensorFailure::NotPresent),
        2 => Err(RawSensorFailure::PermissionDenied),
        _ => Err(RawSensorFailure::Failed),
    }
}

/// `.m` の read の戻り値を契約へ写す（0 値なし / 1 値あり / 2 権限拒否 / 他 失敗）。
/// 非有限値は値なし（`validate_reading`）。
#[cfg(any(target_os = "ios", test))]
fn read_result(
    kind: RawKind,
    code: i32,
    x: f64,
    y: f64,
    z: f64,
) -> Result<Option<RawReading>, RawSensorFailure> {
    match code {
        0 => Ok(None),
        1 => Ok(validate_reading(
            kind,
            RawReading {
                x,
                y,
                z,
                unit: unit_for(kind),
            },
        )),
        2 => Err(RawSensorFailure::PermissionDenied),
        _ => Err(RawSensorFailure::Failed),
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use super::*;
    use fandhe_device::motion_raw_sensors::MotionRawSensors;
    use std::sync::Arc;

    extern "C" {
        fn fandhe_ios_motion_raw_start(kind: i32) -> i32;
        fn fandhe_ios_motion_raw_read(kind: i32, x: *mut f64, y: *mut f64, z: *mut f64) -> i32;
        fn fandhe_ios_motion_raw_stop(kind: i32);
    }

    struct IosMotionRawSensors;

    impl MotionRawSensors for IosMotionRawSensors {
        #[allow(unsafe_code)]
        fn start(&self, kind: RawKind) -> Result<(), RawSensorFailure> {
            // SAFETY: 値渡しの整数のみ。`.m` は main 以外なら何もせず失敗コードを返す。
            start_result(unsafe { fandhe_ios_motion_raw_start(kind_code(kind)) })
        }

        #[allow(unsafe_code)]
        fn read(&self, kind: RawKind) -> Result<Option<RawReading>, RawSensorFailure> {
            let (mut x, mut y, mut z) = (0.0_f64, 0.0_f64, 0.0_f64);
            // SAFETY: 3 本のポインタは呼び出しの間有効な排他参照で、`.m` は保持しない。
            let code =
                unsafe { fandhe_ios_motion_raw_read(kind_code(kind), &mut x, &mut y, &mut z) };
            read_result(kind, code, x, y, z)
        }

        #[allow(unsafe_code)]
        fn stop(&self, kind: RawKind) {
            // SAFETY: 値渡しの整数のみ。冪等でどのスレッドからでも呼べる。
            unsafe { fandhe_ios_motion_raw_stop(kind_code(kind)) }
        }
    }

    /// iOS の `MotionRawSensors` 実装を返す（`device/motion_raw_sensors.rs` の iOS 経路から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn MotionRawSensors> {
        Arc::new(IosMotionRawSensors)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// `.m` が値・失敗を確定した直後に呼ぶ受け口。引数なし・ポインタなしで、`dispatch_sync` は
/// しない（契約表 11）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_motion_raw_sensors_changed() {
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_never_fold_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(RawSensorFailure::NotPresent));
        assert_eq!(start_result(2), Err(RawSensorFailure::PermissionDenied));
        assert_eq!(start_result(3), Err(RawSensorFailure::Failed));
        assert_eq!(start_result(-1), Err(RawSensorFailure::Failed));
    }

    #[test]
    fn read_codes_never_fold_to_value() {
        let k = RawKind::AngularVelocity;
        assert_eq!(read_result(k, 0, 1.0, 2.0, 3.0), Ok(None));
        assert_eq!(
            read_result(k, 2, 0.0, 0.0, 0.0),
            Err(RawSensorFailure::PermissionDenied)
        );
        assert_eq!(
            read_result(k, 3, 0.0, 0.0, 0.0),
            Err(RawSensorFailure::Failed)
        );
        assert_eq!(
            read_result(k, -1, 0.0, 0.0, 0.0),
            Err(RawSensorFailure::Failed)
        );
    }

    #[test]
    fn units_and_values_pass_through() {
        for (k, u) in [
            (RawKind::AngularVelocity, RawUnit::RadPerSec),
            (RawKind::MagneticField, RawUnit::MicroTesla),
            (RawKind::LinearAcceleration, RawUnit::StandardGravity),
        ] {
            assert_eq!(
                read_result(k, 1, 1.5, -2.5, 3.0),
                Ok(Some(RawReading {
                    x: 1.5,
                    y: -2.5,
                    z: 3.0,
                    unit: u
                }))
            );
        }
    }

    #[test]
    fn non_finite_is_none() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                read_result(RawKind::MagneticField, 1, 0.0, v, 0.0),
                Ok(None)
            );
        }
    }

    #[test]
    fn kind_codes_match_m_contract() {
        assert_eq!(kind_code(RawKind::AngularVelocity), 0);
        assert_eq!(kind_code(RawKind::MagneticField), 1);
        assert_eq!(kind_code(RawKind::LinearAcceleration), 2);
    }
}
