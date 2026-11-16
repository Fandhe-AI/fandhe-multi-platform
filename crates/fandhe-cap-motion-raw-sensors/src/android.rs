//! Android 向けジャイロ・磁気・線形加速度の生値実装（Issue #901、親 #180）。
//! 本 crate の `driver` の Android 経路から egui スレッドで呼ばれ、
//! `MotionRawSensorsHelper` の static `start` / `read` / `stop`（第 1 引数は Activity）を同期 JNI で呼んで最新値を pull する
//! （継続配信型のため要求 id・`RequestSlot` は使わない）。Java 側は `MotionRawSensorsHelper`
//! （`TYPE_GYROSCOPE` / `TYPE_MAGNETIC_FIELD` / `TYPE_LINEAR_ACCELERATION`）。
//! API・単位・背景遷移の扱いは `crate::contract` のモジュール doc に従う。
//!
//! # 結果コード（SSOT は Java `MotionRawSensorsHelper`）
//!
//! - kind: 0 = 角速度 / 1 = 磁場 / 2 = 線形加速度（`RawKind::ALL` の順）
//! - `motionRawSensorsStart`: 0 = OK / 1 = 端末に無い（`NotPresent`）/ 2 = 登録失敗・破棄済み・不正な kind（`Failed`）
//! - `motionRawSensorsRead`: `long[4] = [state, xBits, yBits, zBits]`。state は -1 = 購読喪失（`Failed`）/
//!   0 = 値なし / 1 = 値あり。各 bits は double の生ビット（rad/s・µT・m/s²）。
//!   JNI 失敗・null・長さ違いは `Failed`
//!
//! 権限は不要のため `PermissionDenied` は返さない。
//!
//! # ライフサイクル
//!
//! `onPause` で解除して値を捨て（購読の意思は残す）、`onResume` で新世代のリスナーを再登録する。
//! `onDestroy`（再生成を含む）は購読を破棄し、新しい Activity の read は購読喪失になるため
//! `Failed` として driver が `stop` し失敗表示にする。driver は背景フラグを持たないため
//! 前景・背景の native 通知は無い。
//!
//! 守る範囲: 嘘の値（0・前回値・非有限）を出さない、非搭載と OS 非対応を取り違えない、
//! 種類ごとに二重登録しない、停止 / 背景 / 破棄で解除して値を捨てる、旧世代の値を捨てる。
//! 守らない範囲: 絶対精度と較正、サンプリング周期と表示の遅れ、画面回転時の軸、再生成後の失敗表示
//! （利用者の再開始が必要）、複数プロセス。センサー値はログに出さない。実機確認は #885。

use std::sync::Arc;

use crate::contract::{
    MotionRawSensors, RawKind, RawReading, RawSensorFailure, RawUnit, validate_reading,
};

const START_OK: i32 = 0;
const START_NOT_PRESENT: i32 = 1;

const STATE_NONE: i64 = 0;
const STATE_VALUE: i64 = 1;
const READ_LEN: usize = 4;

/// Java 側 kind への写像。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn kind_code(kind: RawKind) -> i32 {
    match kind {
        RawKind::AngularVelocity => 0,
        RawKind::MagneticField => 1,
        RawKind::LinearAcceleration => 2,
    }
}

/// Android の `SensorEvent.values` の単位（rad/s・µT・m/s²）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn unit_of(kind: RawKind) -> RawUnit {
    match kind {
        RawKind::AngularVelocity => RawUnit::RadPerSec,
        RawKind::MagneticField => RawUnit::MicroTesla,
        RawKind::LinearAcceleration => RawUnit::MetersPerSec2,
    }
}

/// `motionRawSensorsStart` の結果コードを契約の結果へ写像する。未知のコードは `Failed`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_start(code: i32) -> Result<(), RawSensorFailure> {
    match code {
        START_OK => Ok(()),
        START_NOT_PRESENT => Err(RawSensorFailure::NotPresent),
        _ => Err(RawSensorFailure::Failed),
    }
}

/// `motionRawSensorsRead` の配列を契約の結果へ写像する。長さ違い・購読喪失・未知の状態は
/// `Failed`、値なし・非有限は `Ok(None)`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(kind: RawKind, v: &[i64]) -> Result<Option<RawReading>, RawSensorFailure> {
    if v.len() != READ_LEN {
        return Err(RawSensorFailure::Failed);
    }
    match v[0] {
        STATE_NONE => return Ok(None),
        STATE_VALUE => {}
        // -1（購読喪失）と未知の状態はどちらも失敗として扱う。
        _ => return Err(RawSensorFailure::Failed),
    }
    let axis = |b: i64| f64::from_bits(b as u64);
    Ok(validate_reading(
        kind,
        RawReading {
            x: axis(v[1]),
            y: axis(v[2]),
            z: axis(v[3]),
            unit: unit_of(kind),
        },
    ))
}

#[cfg(target_os = "android")]
struct AndroidMotionRawSensors;

/// `MotionRawSensorsHelper` の static メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`。`unsafe` は共通層
/// `fandhe_platform_android::with_activity_class` に閉じている。
#[cfg(target_os = "android")]
fn call_helper<T>(
    f: impl FnOnce(
        &mut jni::Env,
        &jni::objects::JObject,
        &jni::objects::JClass,
    ) -> Result<T, fandhe_platform_android::JniCallFailed>,
) -> Option<T> {
    let app = fandhe_platform_android::android_app()?;
    fandhe_platform_android::with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.MotionRawSensorsHelper"),
        f,
    )
    .ok()
}

#[cfg(target_os = "android")]
impl MotionRawSensors for AndroidMotionRawSensors {
    fn start(&self, kind: RawKind) -> Result<(), RawSensorFailure> {
        let code = call_helper(|env, a, class| {
            Ok(env
                .call_static_method(
                    class,
                    jni::jni_str!("start"),
                    jni::jni_sig!((android.app.Activity, int) -> int),
                    &[jni::JValue::Object(a), jni::JValue::Int(kind_code(kind))],
                )?
                .i()?)
        })
        .ok_or(RawSensorFailure::Failed)?;
        decode_start(code)
    }

    fn read(&self, kind: RawKind) -> Result<Option<RawReading>, RawSensorFailure> {
        let values = call_helper(|env, a, class| {
            use jni::objects::JLongArray;
            let ret = env.call_static_method(
                class,
                jni::jni_str!("read"),
                jni::jni_sig!((android.app.Activity, int) -> long[]),
                &[jni::JValue::Object(a), jni::JValue::Int(kind_code(kind))],
            )?;
            let arr = env.cast_local::<JLongArray>(ret.l()?)?;
            if arr.is_null() {
                return Ok(None);
            }
            // 長さを検証してから読む。
            if arr.len(env)? != READ_LEN {
                return Ok(None);
            }
            let mut v = [0i64; READ_LEN];
            arr.get_region(env, 0, &mut v)?;
            Ok(Some(v))
        })
        .flatten()
        .ok_or(RawSensorFailure::Failed)?;
        decode_read(kind, &values)
    }

    fn stop(&self, kind: RawKind) {
        let _ = call_helper(|env, a, class| {
            env.call_static_method(
                class,
                jni::jni_str!("stop"),
                jni::jni_sig!((android.app.Activity, int) -> ()),
                &[jni::JValue::Object(a), jni::JValue::Int(kind_code(kind))],
            )?;
            Ok(())
        });
    }
}

/// Android の `MotionRawSensors` 実装を返す（本 crate の `driver` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn MotionRawSensors> {
    Arc::new(AndroidMotionRawSensors)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn MotionRawSensors> {
    Arc::new(crate::contract::UnsupportedMotionRawSensors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arr(state: i64, x: f64, y: f64, z: f64) -> [i64; 4] {
        [
            state,
            x.to_bits() as i64,
            y.to_bits() as i64,
            z.to_bits() as i64,
        ]
    }

    #[test]
    fn host_implementation_never_succeeds() {
        let imp = implementation();
        for k in RawKind::ALL {
            assert_eq!(imp.start(k), Err(RawSensorFailure::UnsupportedOnOs));
            assert_eq!(imp.read(k), Err(RawSensorFailure::UnsupportedOnOs));
        }
    }

    #[test]
    fn kind_codes_match_java() {
        for (i, k) in RawKind::ALL.into_iter().enumerate() {
            assert_eq!(kind_code(k), i as i32);
        }
    }

    #[test]
    fn start_codes() {
        assert_eq!(decode_start(0), Ok(()));
        assert_eq!(decode_start(1), Err(RawSensorFailure::NotPresent));
        assert_eq!(decode_start(2), Err(RawSensorFailure::Failed));
        assert_eq!(decode_start(9), Err(RawSensorFailure::Failed));
        assert_eq!(decode_start(-1), Err(RawSensorFailure::Failed));
    }

    #[test]
    fn read_state_and_length() {
        let k = RawKind::AngularVelocity;
        assert_eq!(
            decode_read(k, &arr(-1, 1.0, 2.0, 3.0)),
            Err(RawSensorFailure::Failed)
        );
        assert_eq!(
            decode_read(k, &arr(7, 1.0, 2.0, 3.0)),
            Err(RawSensorFailure::Failed)
        );
        assert_eq!(decode_read(k, &arr(0, 1.0, 2.0, 3.0)), Ok(None));
        for len in [0usize, 3, 5] {
            assert_eq!(
                decode_read(k, &vec![1i64; len]),
                Err(RawSensorFailure::Failed)
            );
        }
    }

    #[test]
    fn values_carry_unit_per_kind() {
        let cases = [
            (RawKind::AngularVelocity, RawUnit::RadPerSec),
            (RawKind::MagneticField, RawUnit::MicroTesla),
            (RawKind::LinearAcceleration, RawUnit::MetersPerSec2),
        ];
        for (k, unit) in cases {
            assert_eq!(
                decode_read(k, &arr(1, 0.5, -1.5, 0.0)),
                Ok(Some(RawReading {
                    x: 0.5,
                    y: -1.5,
                    z: 0.0,
                    unit
                }))
            );
        }
    }

    #[test]
    fn non_finite_axis_is_no_value() {
        let k = RawKind::MagneticField;
        assert_eq!(decode_read(k, &arr(1, f64::NAN, 0.0, 0.0)), Ok(None));
        assert_eq!(decode_read(k, &arr(1, 0.0, f64::INFINITY, 0.0)), Ok(None));
        assert_eq!(
            decode_read(k, &arr(1, 0.0, 0.0, f64::NEG_INFINITY)),
            Ok(None)
        );
    }
}
