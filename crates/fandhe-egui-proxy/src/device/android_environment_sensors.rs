//! Android 向け気圧・照度・近接センサー実装（Issue #611、親 #123）。`device/environment_sensors.rs` の
//! Android 経路から egui スレッドで呼ばれ、`MainActivity.environmentSensorsStart` / `Read` / `Stop` を
//! 同期 JNI で呼んで最新値を pull する（継続配信型のため要求 id・`RequestSlot` は使わない）。
//! Java 側は `EnvironmentSensorsHelper`（`TYPE_PRESSURE` / `TYPE_LIGHT` / `TYPE_PROXIMITY`）。
//!
//! # 結果コード（SSOT は Java `EnvironmentSensorsHelper`）
//!
//! - kind: 0 = 気圧 / 1 = 照度 / 2 = 近接
//! - `environmentSensorsStart`: 0 = OK / 1 = 端末に無い（`NotPresent`）/ 2 = 登録失敗・破棄済み・不正な kind（`Failed`）
//! - `environmentSensorsRead`: `long[5] = [state, valueBits, extraBits, receivedAtMs, nowMs]`。
//!   state は -1 = 購読喪失（`Failed`）/ 0 = 値なし / 1 = 値あり。valueBits は double の生ビット
//!   （hPa / lx / 距離 cm）、extraBits は近接だけ最大測定範囲の double 生ビット。時刻は
//!   `SystemClock.elapsedRealtime()`（単調時計）。JNI 失敗・null・長さ違いは `Failed`
//!
//! 権限は不要のため `PermissionDenied` は返さない。範囲・鮮度の検査は `fandhe_device` の関数で行う。
//!
//! # ライフサイクル
//!
//! `onPause` で解除して値を捨て（購読の意思は残す）、`onResume` で新世代のリスナーを再登録する。
//! `onDestroy`（再生成を含む）は購読を破棄し、新しい Activity の read は購読喪失になるため
//! `Failed` として driver が `stop` し失敗表示にする。背景遷移は Java から
//! `nativeOnForegroundChanged` で無条件に通知し、`note_backgrounded` / `note_resumed` を呼ぶ。
//!
//! 守る範囲: 嘘の値（0・前回値・範囲外・古い気圧）を出さない、非搭載と OS 非対応を取り違えない、
//! 種類ごとに二重登録しない、停止 / 背景 / 破棄で解除して値を捨てる、旧世代の値を捨てる。
//! 守らない範囲: 絶対精度、on-change の次の event までの遅れ、近接の距離、2 値しか返さない機種の挙動差、
//! 画面下近接センサーの点滅、背景から戻った直後の値なし、再生成後の失敗表示（利用者の再開始が必要）、
//! 複数プロセス。センサー値はログに出さない。実機確認は #602。

use std::sync::Arc;

use fandhe_device::environment_sensors::{
    is_fresh, proximity_from_distance, validate_lux, validate_pressure_hpa, EnvReading,
    EnvSensorFailure, EnvironmentSensors, SensorKind, PRESSURE_STALE_MS,
};

const START_OK: i32 = 0;
const START_NOT_PRESENT: i32 = 1;

const STATE_NONE: i64 = 0;
const STATE_VALUE: i64 = 1;
const READ_LEN: usize = 5;

/// Java 側 `KIND_*` への写像。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn kind_code(kind: SensorKind) -> i32 {
    match kind {
        SensorKind::Pressure => 0,
        SensorKind::Light => 1,
        SensorKind::Proximity => 2,
    }
}

/// `environmentSensorsStart` の結果コードを契約の結果へ写像する。未知のコードは `Failed`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_start(code: i32) -> Result<(), EnvSensorFailure> {
    match code {
        START_OK => Ok(()),
        START_NOT_PRESENT => Err(EnvSensorFailure::NotPresent),
        _ => Err(EnvSensorFailure::Failed),
    }
}

/// `environmentSensorsRead` の配列を契約の結果へ写像する。長さ違い・購読喪失・未知の状態は
/// `Failed`、値なし・範囲外・古い気圧・無効な近接は `Ok(None)`。検査は変換の前に行う。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(kind: SensorKind, v: &[i64]) -> Result<Option<EnvReading>, EnvSensorFailure> {
    if v.len() != READ_LEN {
        return Err(EnvSensorFailure::Failed);
    }
    match v[0] {
        STATE_NONE => return Ok(None),
        STATE_VALUE => {}
        // -1（購読喪失）と未知の状態はどちらも失敗として扱う。
        _ => return Err(EnvSensorFailure::Failed),
    }
    let value = f64::from_bits(v[1] as u64);
    Ok(match kind {
        SensorKind::Pressure => {
            let fresh = is_fresh(v[3] as f64, v[4] as f64, PRESSURE_STALE_MS);
            validate_pressure_hpa(value)
                .filter(|_| fresh)
                .map(EnvReading::PressureHpa)
        }
        SensorKind::Light => validate_lux(value).map(EnvReading::IlluminanceLux),
        SensorKind::Proximity => {
            proximity_from_distance(value, f64::from_bits(v[2] as u64)).map(EnvReading::Proximity)
        }
    })
}

#[cfg(target_os = "android")]
struct AndroidEnvironmentSensors;

/// `MainActivity` の同期メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`。
/// 2 箇所の `unsafe` は `android_accessibility_prefs.rs::call_read` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity<T>(
    f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> Result<T, super::android::JniCallFailed>,
) -> Option<T> {
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        f(env, &activity)
    })
    .ok()
}

#[cfg(target_os = "android")]
impl EnvironmentSensors for AndroidEnvironmentSensors {
    fn start(&self, kind: SensorKind) -> Result<(), EnvSensorFailure> {
        let code = call_activity(|env, a| {
            Ok(env
                .call_method(
                    a,
                    jni::jni_str!("environmentSensorsStart"),
                    jni::jni_sig!((int) -> int),
                    &[jni::JValue::Int(kind_code(kind))],
                )?
                .i()?)
        })
        .ok_or(EnvSensorFailure::Failed)?;
        decode_start(code)
    }

    fn read(&self, kind: SensorKind) -> Result<Option<EnvReading>, EnvSensorFailure> {
        let values = call_activity(|env, a| {
            use jni::objects::JLongArray;
            let ret = env.call_method(
                a,
                jni::jni_str!("environmentSensorsRead"),
                jni::jni_sig!((int) -> long[]),
                &[jni::JValue::Int(kind_code(kind))],
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
        .ok_or(EnvSensorFailure::Failed)?;
        decode_read(kind, &values)
    }

    fn stop(&self, kind: SensorKind) {
        let _ = call_activity(|env, a| {
            env.call_method(
                a,
                jni::jni_str!("environmentSensorsStop"),
                jni::jni_sig!((int) -> ()),
                &[jni::JValue::Int(kind_code(kind))],
            )?;
            Ok(())
        });
    }
}

/// Android の `EnvironmentSensors` 実装を返す（`device/environment_sensors.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn EnvironmentSensors> {
    Arc::new(AndroidEnvironmentSensors)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn EnvironmentSensors> {
    Arc::new(fandhe_device::environment_sensors::UnsupportedEnvironmentSensors)
}

/// 前景・背景の通知（Java `EnvironmentSensorsHelper.onPause` / `onResume` から無条件に呼ばれる）。
/// 背景中は driver が `NoReading` の待機を進めない。atomics を書くだけでロックは取らない。
/// シンボル名は JNI 命名規約上の固定名。署名 `(Z)V` は Java 宣言・`android.rs` の no-op スタブと 3 か所で一致させる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じる。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_EnvironmentSensorsHelper_nativeOnForegroundChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    foreground: jni::sys::jboolean,
) {
    if foreground {
        super::environment_sensors::note_resumed();
        crate::android::request_repaint();
    } else {
        super::environment_sensors::note_backgrounded();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::environment_sensors::Proximity;

    fn arr(state: i64, value: f64, extra: f64, received: i64, now: i64) -> [i64; 5] {
        [
            state,
            value.to_bits() as i64,
            extra.to_bits() as i64,
            received,
            now,
        ]
    }

    #[test]
    fn host_implementation_never_succeeds() {
        let imp = implementation();
        for k in SensorKind::ALL {
            assert_eq!(imp.start(k), Err(EnvSensorFailure::UnsupportedOnOs));
            assert_eq!(imp.read(k), Err(EnvSensorFailure::UnsupportedOnOs));
        }
    }

    #[test]
    fn kind_codes_match_java() {
        assert_eq!(kind_code(SensorKind::Pressure), 0);
        assert_eq!(kind_code(SensorKind::Light), 1);
        assert_eq!(kind_code(SensorKind::Proximity), 2);
    }

    #[test]
    fn start_codes() {
        assert_eq!(decode_start(0), Ok(()));
        assert_eq!(decode_start(1), Err(EnvSensorFailure::NotPresent));
        assert_eq!(decode_start(2), Err(EnvSensorFailure::Failed));
        assert_eq!(decode_start(9), Err(EnvSensorFailure::Failed));
        assert_eq!(decode_start(-1), Err(EnvSensorFailure::Failed));
    }

    #[test]
    fn read_state_and_length() {
        let k = SensorKind::Light;
        assert_eq!(
            decode_read(k, &arr(-1, 1.0, 0.0, 0, 0)),
            Err(EnvSensorFailure::Failed)
        );
        assert_eq!(
            decode_read(k, &arr(7, 1.0, 0.0, 0, 0)),
            Err(EnvSensorFailure::Failed)
        );
        assert_eq!(decode_read(k, &arr(0, 1.0, 0.0, 0, 0)), Ok(None));
        assert_eq!(decode_read(k, &[1, 0, 0, 0]), Err(EnvSensorFailure::Failed));
        assert_eq!(
            decode_read(k, &[1, 0, 0, 0, 0, 0]),
            Err(EnvSensorFailure::Failed)
        );
        assert_eq!(decode_read(k, &[]), Err(EnvSensorFailure::Failed));
    }

    #[test]
    fn pressure_range_and_freshness() {
        let k = SensorKind::Pressure;
        assert_eq!(
            decode_read(k, &arr(1, 1013.25, 0.0, 1000, 1500)),
            Ok(Some(EnvReading::PressureHpa(1013.25)))
        );
        assert_eq!(decode_read(k, &arr(1, 100.0, 0.0, 1000, 1500)), Ok(None));
        assert_eq!(decode_read(k, &arr(1, 5000.0, 0.0, 1000, 1500)), Ok(None));
        assert_eq!(decode_read(k, &arr(1, f64::NAN, 0.0, 1000, 1500)), Ok(None));
        assert_eq!(decode_read(k, &arr(1, 1013.0, 0.0, 1000, 3001)), Ok(None));
        assert_eq!(decode_read(k, &arr(1, 1013.0, 0.0, 1000, 999)), Ok(None));
    }

    #[test]
    fn light_range() {
        let k = SensorKind::Light;
        assert_eq!(
            decode_read(k, &arr(1, 0.0, 0.0, 0, 0)),
            Ok(Some(EnvReading::IlluminanceLux(0.0)))
        );
        assert_eq!(decode_read(k, &arr(1, -1.0, 0.0, 0, 0)), Ok(None));
        assert_eq!(decode_read(k, &arr(1, f64::NAN, 0.0, 0, 0)), Ok(None));
    }

    #[test]
    fn proximity_near_far() {
        let k = SensorKind::Proximity;
        assert_eq!(
            decode_read(k, &arr(1, 0.0, 5.0, 0, 0)),
            Ok(Some(EnvReading::Proximity(Proximity::Near)))
        );
        assert_eq!(
            decode_read(k, &arr(1, 5.0, 5.0, 0, 0)),
            Ok(Some(EnvReading::Proximity(Proximity::Far)))
        );
        assert_eq!(decode_read(k, &arr(1, 1.0, 0.0, 0, 0)), Ok(None));
        assert_eq!(decode_read(k, &arr(1, f64::NAN, 5.0, 0, 0)), Ok(None));
    }
}
