//! Android 向け画面キャプチャの防止・検知実装（Issue #584、親 #120）。
//! `device/capture_protection.rs` の Android 経路から呼ばれ、`FLAG_SECURE` の付け外しと、
//! スクリーンショット（API 34+）・録画（API 35+）の検知結果を読む。Java 側の SSOT は
//! `CaptureProtectionHelper`（結果コード・状態値・対応ビット・UI スレッドへの橋渡し）。
//!
//! JNI（`MainActivity` の窓口）は pull 型だけで、Java から Rust への push は無い。
//! 署名は Java 側・本ファイルの実体・非 Android の no-op スタブの 3 か所でそろえる。
//! - `captureProtectionSet(boolean) -> int`: UI スレッドで適用し最長 1 秒待つ。
//! - `captureProtectionIsProtected() -> int`: 読み戻した値。`-1` は読めない。
//! - `captureProtectionSupport() -> int`: 対応ビット。
//! - `captureProtectionRecording() -> int`: `-1` 読めない / `0` / `1`。
//! - `captureProtectionScreenshotCount() -> long`: `-1` 読めない / 回数。
//!
//! 期限: 待ちは Java 側で最長 1 秒（GameActivity の `onPause` との相互待ちを避ける）。期限切れは
//! `Internal`（失敗表示）で、期限後に遅れて適用されることはない。
//! Activity 再生成: 保護の要求は Java 側が持ち越して新しい Window へ掛け直し、旧 Activity への
//! 保留要求は破棄される。
//!
//! 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・期限後に適用しないこと。
//! 守らない範囲: 背景中のスクリーンショット、adb・計装の撮影、外部カメラ、掛ける前に始まった録画。
//! 返すエラーは `Internal` だけ（防止は API 1 から使え、権限不要）。JNI 呼び出しの失敗や未知の値は
//! 「読めない」（`None`）に倒し、成功へは倒さない。

use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::capture_protection::CaptureObservation;
use fandhe_device::capture_protection::{CaptureProtection, CaptureSupport};
use fandhe_device::DeviceError;

/// 結果コード（SSOT は Java 側 `CaptureProtectionHelper.RESULT_*`）。0 以外はすべて失敗。
const RESULT_OK: i32 = 0;
#[cfg(test)]
const RESULT_TIMEOUT: i32 = 1;
#[cfg(test)]
const RESULT_FAILED: i32 = 2;

/// 対応ビット（SSOT は Java 側 `CaptureProtectionHelper.SUPPORT_*`）。
const SUPPORT_PREVENT: i32 = 1;
const SUPPORT_RECORDING: i32 = 2;
const SUPPORT_SCREENSHOT: i32 = 4;

/// 結果コードを契約の結果へ写す純関数。0 以外と未知の値はすべて `Internal`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_set(code: i32) -> Result<(), DeviceError> {
    if code == RESULT_OK {
        Ok(())
    } else {
        Err(DeviceError::Internal("capture-protection"))
    }
}

/// `0` / `1` を `Some(false)` / `Some(true)` にする。`-1`（読めない）と未知の値は `None`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_tri(code: i32) -> Option<bool> {
    match code {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// 検知回数。負数（`-1` = 読めない）は `None`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_count(count: i64) -> Option<u64> {
    u64::try_from(count).ok()
}

/// 対応ビットを `CaptureSupport` へ写す。未知のビットは無視する。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_support(bits: i32) -> CaptureSupport {
    CaptureSupport {
        prevent: bits & SUPPORT_PREVENT != 0,
        detect_recording: bits & SUPPORT_RECORDING != 0,
        detect_screenshot: bits & SUPPORT_SCREENSHOT != 0,
    }
}

#[cfg(target_os = "android")]
struct AndroidCaptureProtection;

/// 呼び出す `MainActivity` の窓口。
#[cfg(target_os = "android")]
enum Call {
    Set(bool),
    IsProtected,
    Support,
    Recording,
    ScreenshotCount,
}

/// `MainActivity.captureProtection*` を JNI で呼び、結果を `i64` で返す（`int` は拡張、
/// `long` はそのまま）。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_screen_wake_lock.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(call: Call) -> Option<i64> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i64, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        match call {
            Call::Set(on) => Ok(i64::from(
                env.call_method(
                    &activity,
                    jni::jni_str!("captureProtectionSet"),
                    jni::jni_sig!((boolean) -> int),
                    &[jni::objects::JValue::Bool(on)],
                )?
                .i()?,
            )),
            Call::IsProtected => Ok(i64::from(
                env.call_method(
                    &activity,
                    jni::jni_str!("captureProtectionIsProtected"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
            )),
            Call::Support => Ok(i64::from(
                env.call_method(
                    &activity,
                    jni::jni_str!("captureProtectionSupport"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
            )),
            Call::Recording => Ok(i64::from(
                env.call_method(
                    &activity,
                    jni::jni_str!("captureProtectionRecording"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
            )),
            Call::ScreenshotCount => Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("captureProtectionScreenshotCount"),
                    jni::jni_sig!(() -> long),
                    &[],
                )?
                .j()?),
        }
    })
    .ok()
}

/// `int` を返す窓口の結果を `i32` へ戻す。範囲外（あり得ない）は `None` 扱いにする。
#[cfg(target_os = "android")]
fn call_int(call: Call) -> Option<i32> {
    call_activity(call).and_then(|v| i32::try_from(v).ok())
}

#[cfg(target_os = "android")]
impl CaptureProtection for AndroidCaptureProtection {
    fn support(&self) -> CaptureSupport {
        // 呼び出し失敗は「何も対応していない」に倒す（嘘の対応を出さない）。
        call_int(Call::Support)
            .map(decode_support)
            .unwrap_or_default()
    }

    fn set_protected(&self, on: bool) -> Result<(), DeviceError> {
        match call_int(Call::Set(on)) {
            Some(code) => decode_set(code),
            None => Err(DeviceError::Internal("capture-protection")),
        }
    }

    fn is_protected(&self) -> Option<bool> {
        call_int(Call::IsProtected).and_then(decode_tri)
    }

    fn observe(&self) -> CaptureObservation {
        CaptureObservation {
            recording: call_int(Call::Recording).and_then(decode_tri),
            screenshot_count: call_activity(Call::ScreenshotCount).and_then(decode_count),
        }
    }
}

/// Android の `CaptureProtection` 実装を返す（`device/capture_protection.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn CaptureProtection> {
    Arc::new(AndroidCaptureProtection)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn CaptureProtection> {
    Arc::new(fandhe_device::capture_protection::UnsupportedCaptureProtection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_set_ok_only_for_zero() {
        assert!(decode_set(RESULT_OK).is_ok());
        for c in [RESULT_TIMEOUT, RESULT_FAILED, -1, 99] {
            assert!(matches!(decode_set(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn decode_tri_maps_only_zero_and_one() {
        assert_eq!(decode_tri(0), Some(false));
        assert_eq!(decode_tri(1), Some(true));
        for c in [-1, 2, 99, i32::MIN, i32::MAX] {
            assert_eq!(decode_tri(c), None);
        }
    }

    #[test]
    fn decode_count_rejects_negative() {
        assert_eq!(decode_count(0), Some(0));
        assert_eq!(decode_count(7), Some(7));
        assert_eq!(decode_count(-1), None);
        assert_eq!(decode_count(i64::MIN), None);
    }

    #[test]
    fn decode_support_ignores_unknown_bits() {
        assert_eq!(decode_support(0), CaptureSupport::default());
        let all = decode_support(SUPPORT_PREVENT | SUPPORT_RECORDING | SUPPORT_SCREENSHOT);
        assert!(all.prevent && all.detect_recording && all.detect_screenshot);
        let only_prevent = decode_support(SUPPORT_PREVENT | 0x40);
        assert!(only_prevent.prevent);
        assert!(!only_prevent.detect_recording && !only_prevent.detect_screenshot);
        let _ = decode_support(-1);
    }

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `CaptureProtectionHelper.RESULT_*` / `SUPPORT_*` と同じ値であること。
        assert_eq!((RESULT_OK, RESULT_TIMEOUT, RESULT_FAILED), (0, 1, 2));
        assert_eq!(
            (SUPPORT_PREVENT, SUPPORT_RECORDING, SUPPORT_SCREENSHOT),
            (1, 2, 4)
        );
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn non_android_stub_is_not_a_fake_success() {
        let p = implementation();
        assert!(matches!(
            p.set_protected(true),
            Err(DeviceError::Unsupported(_))
        ));
        assert_eq!(p.is_protected(), None);
    }
}
