//! Android 向けライト（トーチ）実装（Issue #461、親 #106）。`driver` の Android 経路
//! （egui スレッドから同期）から呼ばれ、`CameraManager.setTorchMode` を JNI で叩く。
//! Java 側の SSOT は `FlashlightHelper`、結果コードの SSOT は `crate::contract`。
//!
//! JNI: `FlashlightHelper.set(Activity, boolean) -> int` / `read(Activity) -> int`（static）。
//! Helper が無い（生成前・破棄後）要求は Java 側が失敗コードで確定する。
//! no-op スタブは置かない: Java の native は持たず、Rust からの要求でしか動かない。
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

use crate::contract::Flashlight;
#[cfg(target_os = "android")]
use crate::contract::{decode_read, decode_set};
#[cfg(target_os = "android")]
use crate::error::FlashlightError;

#[cfg(target_os = "android")]
struct AndroidFlashlight;

/// `FlashlightHelper.set(Activity, boolean)`（`Some(on)`）または `read(Activity)`（`None`）を
/// JNI の static 呼び出しで呼ぶ。`None` は呼び出し失敗。JavaVM と Activity の取得は
/// `fandhe_platform_android::with_activity_class` に任せ、ここは `unsafe` を持たない。
#[cfg(target_os = "android")]
fn call_activity(set: Option<bool>) -> Option<i32> {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let app = android_app()?;
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.FlashlightHelper"),
        |env, activity, class| -> Result<i32, JniCallFailed> {
            match set {
                Some(on) => Ok(env
                    .call_static_method(
                        class,
                        jni::jni_str!("set"),
                        jni::jni_sig!((android.app.Activity, boolean) -> int),
                        &[
                            jni::objects::JValue::Object(activity),
                            jni::objects::JValue::Bool(on),
                        ],
                    )?
                    .i()?),
                None => Ok(env
                    .call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.app.Activity) -> int),
                        &[jni::objects::JValue::Object(activity)],
                    )?
                    .i()?),
            }
        },
    )
    .ok()
}

#[cfg(target_os = "android")]
impl Flashlight for AndroidFlashlight {
    fn set_on(&self, on: bool) -> Result<(), FlashlightError> {
        match call_activity(Some(on)) {
            Some(code) => decode_set(code),
            None => Err(FlashlightError::Internal("flashlight")),
        }
    }

    fn is_on(&self) -> Result<Option<bool>, FlashlightError> {
        match call_activity(None) {
            Some(code) => decode_read(code),
            None => Err(FlashlightError::Internal("flashlight")),
        }
    }
}

/// Android の `Flashlight` 実装を返す（`driver` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn Flashlight> {
    Arc::new(AndroidFlashlight)
}

/// ホストのテストビルド向け。dispatch は Android でしか呼ばない。非対応を返し成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn Flashlight> {
    Arc::new(crate::contract::UnsupportedFlashlight)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        READ_OFF, READ_ON, READ_UNKNOWN, READ_UNSUPPORTED, SET_OK, SET_UNSUPPORTED,
    };
    use crate::error::FlashlightError;

    #[test]
    fn host_impl_is_never_success() {
        let f = implementation();
        assert!(matches!(f.set_on(true), Err(FlashlightError::Unsupported)));
        assert!(matches!(f.set_on(false), Err(FlashlightError::Unsupported)));
        assert!(matches!(f.is_on(), Err(FlashlightError::Unsupported)));
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
