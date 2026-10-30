//! Android 向けアラーム（正確な時刻の予約）の実装（Issue #1078、親 #1056）。`device/alarm.rs` の
//! Android 経路から、ワーカースレッドが操作ごとに同期で呼ぶ。契約の SSOT は `fandhe_device::alarm`。
//!
//! Java 側 `MainActivity.alarmSchedule(long)I` / `alarmCancel()I` / `alarmState()J` が
//! `AlarmHelper`（`AlarmManager.setAlarmClock` + `FLAG_ONE_SHOT` の `PendingIntent`）へ委譲する。
//! 戻りコードの意味は本ファイルの純関数 `match` が SSOT（Java 側の値と一致させること。ホストでもテストする）。
//!
//! | schedule / cancel の戻り | 意味 |
//! |---|---|
//! | 0 | 成功 |
//! | 1 | 正確なアラームの許可なし（許可画面を開いた）→ `PermissionDenied` |
//! | 2 | 通知が無効 → `PermissionDenied` |
//! | 3・その他 | `SecurityException` など → `Internal("alarm add")`（cancel は `"alarm remove"`） |
//!
//! | state の戻り | 意味 |
//! |---|---|
//! | 0 | `PendingIntent` が引けない → `NotScheduled`（鳴った・電源断・許可の取り消しも同じ） |
//! | 正の値 | 予約中。記録した鳴る時刻（UNIX ms） |
//! | 負の値 | 予約はあるが記録が無い・例外 → `Internal("alarm list")` |
//!
//! 番兵の根拠: 鳴る時刻は `validate_fire_at` により常に現在時刻より後なので、正の値だけが時刻で、
//! 0 と負の値は番兵に使える。
//!
//! - 同期 JNI の一発呼び出しで、保留中の要求・要求 id・native コールバックを持たない。Activity が
//!   再生成されても破棄するものは無く、旧結果が新しい要求の id を消費することは構造上ない
//! - `android_app()` 無し（Activity 準備前）・JNI 失敗は各操作の `Internal` で、成功にしない
//! - 時刻・例外の文言・結果コードはログに出さない
//!
//! 守る範囲: 嘘の成功を返さない・1 プロセス内の直列化（Java 側 `synchronized`）・資源の解放。
//! 守らない範囲: 再起動後の再登録、スヌーズ、通知からアプリを開くこと、許可後の自動再予約、
//! 複数プロセス、システム時刻の変更。実機の確認は #711。

use std::sync::Arc;

use fandhe_device::alarm::{Alarm, AlarmState};
use fandhe_device::DeviceError;

/// 鳴る時刻（UNIX ms）を JNI の `long` へ。範囲外は入力不正（丸めない）。
fn to_jlong(fire_at: u64) -> Result<i64, DeviceError> {
    i64::try_from(fire_at).map_err(|_| DeviceError::InvalidInput("alarm time"))
}

/// `alarmSchedule` の戻りコードを契約の結果へ写す。
fn schedule_result(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 | 2 => Err(DeviceError::PermissionDenied("alarm")),
        _ => Err(DeviceError::Internal("alarm add")),
    }
}

/// `alarmCancel` の戻りコードを契約の結果へ写す。
fn cancel_result(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        _ => Err(DeviceError::Internal("alarm remove")),
    }
}

/// `alarmState` の戻りを契約の状態へ写す。
fn state_result(raw: i64) -> Result<AlarmState, DeviceError> {
    match raw {
        0 => Ok(AlarmState::NotScheduled),
        t if t > 0 => Ok(AlarmState::Scheduled {
            fire_at_unix_ms: t as u64,
        }),
        _ => Err(DeviceError::Internal("alarm list")),
    }
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::{cancel_result, schedule_result, state_result, to_jlong};
    use crate::device::alarm::{now_unix_ms, validate_fire_at};
    use crate::device::android::JniCallFailed;
    use fandhe_device::alarm::{Alarm, AlarmState};
    use fandhe_device::DeviceError;

    enum Call {
        Schedule(i64),
        Cancel,
        State,
    }

    /// `MainActivity` のメソッドを 1 回呼び、戻り値を `i64` に揃えて返す（int は拡張）。
    #[allow(unsafe_code)]
    fn call(c: Call) -> Option<i64> {
        let app = crate::android::android_app()?;
        // SAFETY: `AndroidDeviceIdentifier::read` と同一の根拠。`vm_as_ptr()` は android-activity が
        // 管理する有効な `JavaVM*`、`app` は呼び出し中生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<i64, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 同上。未所有の JNI グローバル参照を `Cast` で包むだけで delete は試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let ret = match c {
                Call::Schedule(ms) => env
                    .call_method(
                        &activity,
                        jni::jni_str!("alarmSchedule"),
                        jni::jni_sig!((long) -> int),
                        &[jni::objects::JValue::Long(ms as jni::sys::jlong)],
                    )
                    .and_then(|v| v.i().map(i64::from)),
                Call::Cancel => env
                    .call_method(
                        &activity,
                        jni::jni_str!("alarmCancel"),
                        jni::jni_sig!(() -> int),
                        &[],
                    )
                    .and_then(|v| v.i().map(i64::from)),
                Call::State => env
                    .call_method(
                        &activity,
                        jni::jni_str!("alarmState"),
                        jni::jni_sig!(() -> long),
                        &[],
                    )
                    .and_then(|v| v.j()),
            };
            match ret {
                Ok(v) => Ok(v),
                Err(e) => {
                    // 例外は自動クリアされない（jni 0.22）。文言は読まない。
                    if env.exception_check() {
                        env.exception_clear();
                    }
                    Err(e.into())
                }
            }
        })
        .ok()
    }

    /// Android の `Alarm` 実装。
    pub(super) struct AndroidAlarm;

    impl Alarm for AndroidAlarm {
        fn schedule(&self, fire_at_unix_ms: u64) -> Result<(), DeviceError> {
            validate_fire_at(fire_at_unix_ms, now_unix_ms()?)?;
            let ms = to_jlong(fire_at_unix_ms)?;
            match call(Call::Schedule(ms)) {
                Some(code) => schedule_result(code as i32),
                None => Err(DeviceError::Internal("alarm add")),
            }
        }

        fn cancel(&self) -> Result<(), DeviceError> {
            match call(Call::Cancel) {
                Some(code) => cancel_result(code as i32),
                None => Err(DeviceError::Internal("alarm remove")),
            }
        }

        fn state(&self) -> Result<AlarmState, DeviceError> {
            match call(Call::State) {
                Some(raw) => state_result(raw),
                None => Err(DeviceError::Internal("alarm list")),
            }
        }
    }
}

/// Android の `Alarm` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn Alarm> {
    Arc::new(jni_impl::AndroidAlarm)
}

/// ホスト（`cfg(test)`）ではビルド確認用に `Unsupported` を返す（Android ビルドでは使われない）。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn Alarm> {
    Arc::new(fandhe_device::alarm::UnsupportedAlarm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_codes() {
        assert!(schedule_result(0).is_ok());
        for c in [1, 2] {
            assert!(matches!(
                schedule_result(c),
                Err(DeviceError::PermissionDenied(_))
            ));
        }
        for c in [3, 4, -1, 99] {
            assert!(matches!(schedule_result(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn cancel_codes() {
        assert!(cancel_result(0).is_ok());
        for c in [1, 4, -1, 99] {
            assert!(matches!(cancel_result(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn state_values() {
        assert!(matches!(state_result(0), Ok(AlarmState::NotScheduled)));
        assert!(matches!(
            state_result(1),
            Ok(AlarmState::Scheduled { fire_at_unix_ms: 1 })
        ));
        assert!(matches!(
            state_result(i64::MAX),
            Ok(AlarmState::Scheduled { fire_at_unix_ms }) if fire_at_unix_ms == i64::MAX as u64
        ));
        for v in [-1, i64::MIN] {
            assert!(matches!(state_result(v), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn jlong_range() {
        assert_eq!(to_jlong(5).unwrap(), 5);
        assert!(matches!(
            to_jlong(u64::MAX),
            Err(DeviceError::InvalidInput(_))
        ));
    }
}
