//! Android 向け画面輝度実装（Issue #635、親 #126）。`driver` の Android
//! 経路から呼ばれ、`WindowManager.LayoutParams.screenBrightness` を Window に設定・解除する
//! （解除は `BRIGHTNESS_OVERRIDE_NONE`）。Java 側の SSOT は `ScreenBrightnessHelper`
//! （結果コード・番兵値・UI スレッドへの橋渡し）。
//!
//! JNI 窓口（`ScreenBrightnessHelper` の static。Java と本ファイルの `jni_sig!` の 2 か所でそろえる）:
//!
//! - `int set(Activity, float)`: UI スレッドで適用し、最長 1 秒待つ
//! - `int clear(Activity)`: 同上で解除する
//! - `float read(Activity)`: UI スレッドが読み戻してキャッシュした値
//!
//! 期限: 待ちは Java 側で最長 1 秒（GameActivity の `onPause` との相互待ちを避ける）。
//! 期限切れは `Internal` で、期限後に遅れて適用されることはない。成否は読み戻しで決まる。
//!
//! 読み取り: 上書きしていない間は `level=None` / `overriding=false`（システム値は端末の曲線と
//! 一致しないため出さない）。OS の読み戻しは `validate_level` を通してから使う。
//!
//! Activity 再生成: 要求値を Java の static で持ち越して `ScreenBrightnessLifecycle`（`onHostCreated`）で掛け直し、実終了（`onHostDestroyed`）で捨てる。
//! 保留中の旧要求は Window に触れない。
//!
//! 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・期限後に適用しないこと・
//! 範囲外の値を OS に渡さないこと。守らない範囲: 背景にいる間は OS が利用者の値で表示すること、
//! 複数ウィンドウ・分割画面、自動輝度や利用者の操作による一時的な食い違い、プロセス強制終了時
//! （Window 破棄で OS が上書きを解く）。`Unsupported` / `PermissionDenied` は返さない
//! （API 3 から使え、権限不要）。

use std::sync::Arc;

use crate::contract::{BrightnessReading, ScreenBrightness, validate_level};
use crate::error::ScreenBrightnessError;

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
fn decode_result(code: i32) -> Result<(), ScreenBrightnessError> {
    match code {
        RESULT_OK => Ok(()),
        RESULT_INVALID => Err(ScreenBrightnessError::InvalidInput),
        _ => Err(ScreenBrightnessError::Internal),
    }
}

/// 読み取り値を `BrightnessReading` へ写す純関数。範囲の検証を先に行い、丸めはしない。
/// OS の読み戻しの異常は入力の誤りではないため `Internal` にする。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(v: f32) -> Result<BrightnessReading, ScreenBrightnessError> {
    if v.is_nan() {
        return Err(ScreenBrightnessError::Internal);
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
        Err(_) => Err(ScreenBrightnessError::Internal),
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

/// `ScreenBrightnessHelper` の static メソッド（`set` / `clear` / `read`）を JNI で呼ぶ。`None` は呼び出し失敗。
/// クラスは Activity の class loader で解決する（`fandhe_platform_android::with_activity_class`）。
#[cfg(target_os = "android")]
fn call_helper(call: Call) -> Option<Reply> {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    use jni::objects::JValue;
    let app = android_app()?;
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ScreenBrightnessHelper"),
        |env, activity, class| -> Result<Reply, JniCallFailed> {
            match call {
                Call::Set(level) => Ok(Reply::Code(
                    env.call_static_method(
                        class,
                        jni::jni_str!("set"),
                        jni::jni_sig!((android.app.Activity, float) -> int),
                        &[JValue::Object(activity), JValue::Float(level)],
                    )?
                    .i()?,
                )),
                Call::Clear => Ok(Reply::Code(
                    env.call_static_method(
                        class,
                        jni::jni_str!("clear"),
                        jni::jni_sig!((android.app.Activity) -> int),
                        &[JValue::Object(activity)],
                    )?
                    .i()?,
                )),
                Call::Read => Ok(Reply::Level(
                    env.call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.app.Activity) -> float),
                        &[JValue::Object(activity)],
                    )?
                    .f()?,
                )),
            }
        },
    )
    .ok()
}

#[cfg(target_os = "android")]
fn code_of(reply: Option<Reply>) -> Result<(), ScreenBrightnessError> {
    match reply {
        Some(Reply::Code(code)) => decode_result(code),
        _ => Err(ScreenBrightnessError::Internal),
    }
}

#[cfg(target_os = "android")]
impl ScreenBrightness for AndroidScreenBrightness {
    fn set_override(&self, level: f32) -> Result<(), ScreenBrightnessError> {
        // OS へ渡す前に検証する。
        let level = validate_level(level)?;
        code_of(call_helper(Call::Set(level)))
    }

    fn clear_override(&self) -> Result<(), ScreenBrightnessError> {
        code_of(call_helper(Call::Clear))
    }

    fn read(&self) -> Result<BrightnessReading, ScreenBrightnessError> {
        match call_helper(Call::Read) {
            Some(Reply::Level(v)) => decode_read(v),
            _ => Err(ScreenBrightnessError::Internal),
        }
    }
}

/// Android の `ScreenBrightness` 実装を返す（`driver` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn ScreenBrightness> {
    Arc::new(AndroidScreenBrightness)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn ScreenBrightness> {
    Arc::new(crate::contract::UnsupportedScreenBrightness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_result_maps_codes() {
        assert!(decode_result(RESULT_OK).is_ok());
        assert!(matches!(
            decode_result(RESULT_INVALID),
            Err(ScreenBrightnessError::InvalidInput)
        ));
        for c in [RESULT_TIMEOUT, RESULT_FAILED, -1, 99] {
            assert!(matches!(
                decode_result(c),
                Err(ScreenBrightnessError::Internal)
            ));
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
                matches!(decode_read(v), Err(ScreenBrightnessError::Internal)),
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
