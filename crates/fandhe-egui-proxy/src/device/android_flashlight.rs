//! Android 向けライト（トーチ）実装（Issue #461、親 #106）。`device/flashlight.rs` の Android 経路
//! （egui スレッドから同期）から呼ばれ、`CameraManager.setTorchMode` を JNI で叩く。
//! Java 側の SSOT は `FlashlightHelper`、結果コードの SSOT は `fandhe_device::flashlight`。
//!
//! JNI: `MainActivity.flashlightSet(boolean) -> int` / `flashlightRead() -> int`。
//! どちらも UI スレッドへ post せず同期で戻る。期限は driver の `CONFIRM_TIMEOUT` だけが持つ。
//!
//! Activity 再生成: Java の `onDestroy` で消灯・コールバック解除し旧要求は残らない。
//! 新しい helper は最初のコールバックまで「まだ分からない」を返すため、再生成をまたいで
//! 確認中だった要求は期限切れで失敗に確定し、成功には倒れない。
//!
//! 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・背景中と破棄後に点けないこと。
//! 守らない範囲: 外部からの点け消しへの追従遅れ、他アプリのカメラ使用中の一時的な食い違い、
//! 熱による減光、プロセス終了時の後片付け（OS に任せる）。

use std::sync::Arc;

use fandhe_device::flashlight::Flashlight;
#[cfg(target_os = "android")]
use fandhe_device::flashlight::{decode_read, decode_set};
#[cfg(target_os = "android")]
use fandhe_device::DeviceError;

#[cfg(target_os = "android")]
struct AndroidFlashlight;

/// `MainActivity.flashlightSet(boolean)`（`Some(on)`）または `flashlightRead()`（`None`）を
/// JNI で呼ぶ。`None` は呼び出し失敗。`unsafe` 2 箇所は `android_screen_wake_lock.rs` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(set: Option<bool>) -> Option<i32> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        match set {
            Some(on) => Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("flashlightSet"),
                    jni::jni_sig!((boolean) -> int),
                    &[jni::objects::JValue::Bool(on)],
                )?
                .i()?),
            None => Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("flashlightRead"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?),
        }
    })
    .ok()
}

#[cfg(target_os = "android")]
impl Flashlight for AndroidFlashlight {
    fn set_on(&self, on: bool) -> Result<(), DeviceError> {
        match call_activity(Some(on)) {
            Some(code) => decode_set(code),
            None => Err(DeviceError::Internal("flashlight")),
        }
    }

    fn is_on(&self) -> Result<Option<bool>, DeviceError> {
        match call_activity(None) {
            Some(code) => decode_read(code),
            None => Err(DeviceError::Internal("flashlight")),
        }
    }
}

/// Android の `Flashlight` 実装を返す（`device/flashlight.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn Flashlight> {
    Arc::new(AndroidFlashlight)
}

/// ホストのテストビルド向け。dispatch は Android でしか呼ばない。非対応を返し成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn Flashlight> {
    Arc::new(fandhe_device::flashlight::UnsupportedFlashlight)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::flashlight::{
        READ_OFF, READ_ON, READ_UNKNOWN, READ_UNSUPPORTED, SET_OK, SET_UNSUPPORTED,
    };
    use fandhe_device::DeviceError;

    #[test]
    fn host_impl_is_never_success() {
        let f = implementation();
        assert!(matches!(f.set_on(true), Err(DeviceError::Unsupported(_))));
        assert!(matches!(f.set_on(false), Err(DeviceError::Unsupported(_))));
        assert!(matches!(f.is_on(), Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `FlashlightHelper` の SET_* / READ_* と同じ値であること
        // （SET_FAILED / READ_FAILED = 2 は未知値として Internal になる）。
        assert_eq!((SET_OK, SET_UNSUPPORTED), (0, 1));
        assert_eq!(
            (READ_OFF, READ_ON, READ_UNSUPPORTED, READ_UNKNOWN),
            (0, 1, -1, -2)
        );
    }
}
