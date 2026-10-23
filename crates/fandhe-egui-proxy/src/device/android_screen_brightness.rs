//! Android 向け画面輝度実装（Issue #635、親 #126）。`device/screen_brightness.rs` の Android
//! 経路から呼ばれ、`WindowManager.LayoutParams.screenBrightness` を Window に設定・解除する
//! （解除は `BRIGHTNESS_OVERRIDE_NONE`）。Java 側の SSOT は `ScreenBrightnessHelper`
//! （結果コード・番兵値・UI スレッドへの橋渡し）。
//!
//! JNI 窓口（`MainActivity`。Java・本ファイルの `jni_sig!`・非 Android スタブの 3 か所でそろえる）:
//!
//! - `int screenBrightnessSet(float)`: UI スレッドで適用し、最長 1 秒待つ
//! - `int screenBrightnessClear()`: 同上で解除する
//! - `float screenBrightnessRead()`: UI スレッドが読み戻してキャッシュした値
//!
//! 期限: 待ちは Java 側で最長 1 秒（GameActivity の `onPause` との相互待ちを避ける）。
//! 期限切れは `Internal` で、期限後に遅れて適用されることはない。成否は読み戻しで決まる。
//!
//! 読み取り: 上書きしていない間は `level=None` / `overriding=false`（システム値は端末の曲線と
//! 一致しないため出さない）。OS の読み戻しは `validate_level` を通してから使う。
//!
//! Activity 再生成: 要求値を Java の static で持ち越して `onCreate` で掛け直し、実終了で捨てる。
//! 保留中の旧要求は Window に触れない。
//!
//! 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・期限後に適用しないこと・
//! 範囲外の値を OS に渡さないこと。守らない範囲: 背景にいる間は OS が利用者の値で表示すること、
//! 複数ウィンドウ・分割画面、自動輝度や利用者の操作による一時的な食い違い、プロセス強制終了時
//! （Window 破棄で OS が上書きを解く）。`Unsupported` / `PermissionDenied` は返さない
//! （API 3 から使え、権限不要）。

use std::sync::Arc;

use fandhe_device::screen_brightness::{validate_level, BrightnessReading, ScreenBrightness};
use fandhe_device::DeviceError;

/// 結果コード（SSOT は Java 側 `ScreenBrightnessHelper.RESULT_*`）。
const RESULT_OK: i32 = 0;
#[cfg(test)]
const RESULT_TIMEOUT: i32 = 1;
#[cfg(test)]
const RESULT_FAILED: i32 = 2;
const RESULT_INVALID: i32 = 3;

/// 読み取りの番兵（SSOT は Java 側 `READ_*`）。値域 0〜1 と衝突しない。
const READ_NONE: f32 = -1.0;
#[cfg(test)]
const READ_UNKNOWN: f32 = -2.0;

/// 結果コードを契約の結果へ写す純関数。3 は `InvalidInput`、それ以外の 0 以外は `Internal`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_result(code: i32) -> Result<(), DeviceError> {
    match code {
        RESULT_OK => Ok(()),
        RESULT_INVALID => Err(DeviceError::InvalidInput("screen-brightness level")),
        _ => Err(DeviceError::Internal("screen-brightness")),
    }
}

/// 読み取り値を `BrightnessReading` へ写す純関数。範囲の検証を先に行い、丸めはしない。
/// OS の読み戻しの異常は入力の誤りではないため `Internal` にする。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(v: f32) -> Result<BrightnessReading, DeviceError> {
    if v.is_nan() {
        return Err(DeviceError::Internal("screen-brightness"));
    }
    if v == READ_NONE {
        return Ok(BrightnessReading {
            level: None,
            overriding: false,
        });
    }
    match validate_level(v) {
        Ok(level) => Ok(BrightnessReading {
            level: Some(level),
            overriding: true,
        }),
        Err(_) => Err(DeviceError::Internal("screen-brightness")),
    }
}

#[cfg(target_os = "android")]
struct AndroidScreenBrightness;

#[cfg(target_os = "android")]
enum Call {
    Set(f32),
    Clear,
    Read,
}

#[cfg(target_os = "android")]
enum Reply {
    Code(i32),
    Level(f32),
}

/// `MainActivity.screenBrightness*` を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_screen_wake_lock.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(call: Call) -> Option<Reply> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<Reply, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        match call {
            Call::Set(level) => Ok(Reply::Code(
                env.call_method(
                    &activity,
                    jni::jni_str!("screenBrightnessSet"),
                    jni::jni_sig!((float) -> int),
                    &[jni::objects::JValue::Float(level)],
                )?
                .i()?,
            )),
            Call::Clear => Ok(Reply::Code(
                env.call_method(
                    &activity,
                    jni::jni_str!("screenBrightnessClear"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
            )),
            Call::Read => Ok(Reply::Level(
                env.call_method(
                    &activity,
                    jni::jni_str!("screenBrightnessRead"),
                    jni::jni_sig!(() -> float),
                    &[],
                )?
                .f()?,
            )),
        }
    })
    .ok()
}

#[cfg(target_os = "android")]
fn code_of(reply: Option<Reply>) -> Result<(), DeviceError> {
    match reply {
        Some(Reply::Code(code)) => decode_result(code),
        _ => Err(DeviceError::Internal("screen-brightness")),
    }
}

#[cfg(target_os = "android")]
impl ScreenBrightness for AndroidScreenBrightness {
    fn set_override(&self, level: f32) -> Result<(), DeviceError> {
        // OS へ渡す前に検証する。
        let level = validate_level(level)?;
        code_of(call_activity(Call::Set(level)))
    }

    fn clear_override(&self) -> Result<(), DeviceError> {
        code_of(call_activity(Call::Clear))
    }

    fn read(&self) -> Result<BrightnessReading, DeviceError> {
        match call_activity(Call::Read) {
            Some(Reply::Level(v)) => decode_read(v),
            _ => Err(DeviceError::Internal("screen-brightness")),
        }
    }
}

/// Android の `ScreenBrightness` 実装を返す（`device/screen_brightness.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn ScreenBrightness> {
    Arc::new(AndroidScreenBrightness)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn ScreenBrightness> {
    Arc::new(fandhe_device::screen_brightness::UnsupportedScreenBrightness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_result_maps_codes() {
        assert!(decode_result(RESULT_OK).is_ok());
        assert!(matches!(
            decode_result(RESULT_INVALID),
            Err(DeviceError::InvalidInput(_))
        ));
        for c in [RESULT_TIMEOUT, RESULT_FAILED, -1, 99] {
            assert!(matches!(decode_result(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn decode_read_none_means_not_overriding() {
        let r = decode_read(READ_NONE).unwrap();
        assert_eq!(r.level, None);
        assert!(!r.overriding);
    }

    #[test]
    fn decode_read_valid_levels() {
        for v in [0.0_f32, 0.5, 1.0] {
            let r = decode_read(v).unwrap();
            assert_eq!(r.level, Some(v));
            assert!(r.overriding);
        }
    }

    #[test]
    fn decode_read_rejects_abnormal_values() {
        for v in [
            READ_UNKNOWN,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1.01,
            -0.01,
        ] {
            assert!(
                matches!(decode_read(v), Err(DeviceError::Internal(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `ScreenBrightnessHelper` の定数と同じ値であること。
        assert_eq!(
            (RESULT_OK, RESULT_TIMEOUT, RESULT_FAILED, RESULT_INVALID),
            (0, 1, 2, 3)
        );
        assert_eq!((READ_NONE, READ_UNKNOWN), (-1.0, -2.0));
    }

    #[test]
    fn host_implementation_never_succeeds() {
        let imp = implementation();
        assert!(imp.set_override(0.5).is_err());
        assert!(imp.clear_override().is_err());
        assert!(imp.read().is_err());
    }
}
