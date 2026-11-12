//! Android 向け画面スリープ抑止実装（Issue #1280、親 #1129）。`driver.rs` の
//! Android 経路から呼ばれ、`FLAG_KEEP_SCREEN_ON` を Window に付け外しする。Java 側の SSOT は
//! `ScreenWakeLockHelper`（結果コード・UI スレッドへの橋渡し）。
//!
//! 同期呼び出し: `drive` は `set_keep_awake` の直後に `is_held` を読むため、JNI
//! `ScreenWakeLockHelper.set` は UI スレッドでの適用と読み戻しを終えてから戻る。
//! 待ちは Java 側で最長 1 秒（GameActivity の `onPause` との相互待ちを避ける）で、
//! 期限切れは `Internal`（失敗表示）。期限後に遅れて適用されることはなく、`is_held` は
//! 前の値のまま残る。`is_held` は UI スレッドで読み戻した `FLAG_KEEP_SCREEN_ON`。
//!
//! Activity 再生成: Java の `onDestroy` で要求を捨て、Rust 側の画面も未要求から作り直される。
//!
//! 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・期限後に適用しないこと。
//! 守らない範囲: 背景にいる間は効かないこと（見えないため実害なし）、他要因との一時的な
//! 食い違い（プロセス終了は OS が片付ける）。`Unsupported` / `PermissionDenied` は返さない
//! （API 1 から使え、権限不要）。

use std::sync::Arc;

use crate::contract::ScreenWakeLock;
use crate::error::ScreenWakeLockError;

/// 結果コード（SSOT は Java 側 `ScreenWakeLockHelper.RESULT_*`）。0 以外はすべて失敗。
const RESULT_OK: i32 = 0;
#[cfg(test)]
const RESULT_TIMEOUT: i32 = 1;
#[cfg(test)]
const RESULT_FAILED: i32 = 2;

/// 結果コードを契約の結果へ写す純関数。0 以外と未知の値はすべて `Internal`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode(code: i32) -> Result<(), ScreenWakeLockError> {
    if code == RESULT_OK {
        Ok(())
    } else {
        Err(ScreenWakeLockError::Internal)
    }
}

#[cfg(target_os = "android")]
struct AndroidScreenWakeLock;

/// `ScreenWakeLockHelper.set(Activity, boolean)`（`Some(keep)`）または
/// `isHeld(Activity)`（`None`、真偽を 1/0 で返す）を JNI で呼ぶ。`None` は呼び出し失敗。
/// クラスは Activity の class loader で解決する（`fandhe_platform_android::with_activity_class`）。
#[cfg(target_os = "android")]
fn call_activity(set: Option<bool>) -> Option<i32> {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let app = android_app()?;
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ScreenWakeLockHelper"),
        |env, activity, class| -> Result<i32, JniCallFailed> {
            match set {
                Some(keep) => Ok(env
                    .call_static_method(
                        class,
                        jni::jni_str!("set"),
                        jni::jni_sig!((android.app.Activity, boolean) -> int),
                        &[
                            jni::objects::JValue::Object(activity),
                            jni::objects::JValue::Bool(keep),
                        ],
                    )?
                    .i()?),
                None => Ok(i32::from(
                    env.call_static_method(
                        class,
                        jni::jni_str!("isHeld"),
                        jni::jni_sig!((android.app.Activity) -> boolean),
                        &[jni::objects::JValue::Object(activity)],
                    )?
                    .z()?,
                )),
            }
        },
    )
    .ok()
}

#[cfg(target_os = "android")]
impl ScreenWakeLock for AndroidScreenWakeLock {
    fn set_keep_awake(&self, keep: bool) -> Result<(), ScreenWakeLockError> {
        match call_activity(Some(keep)) {
            Some(code) => decode(code),
            None => Err(ScreenWakeLockError::Internal),
        }
    }

    fn is_held(&self) -> bool {
        call_activity(None) == Some(1)
    }
}

/// Android の `ScreenWakeLock` 実装を返す（`driver.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn ScreenWakeLock> {
    Arc::new(AndroidScreenWakeLock)
}

/// `lib.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn ScreenWakeLock> {
    Arc::new(crate::contract::UnsupportedScreenWakeLock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_ok_only_for_zero() {
        assert!(decode(RESULT_OK).is_ok());
        for c in [RESULT_TIMEOUT, RESULT_FAILED, -1, 99] {
            assert!(matches!(decode(c), Err(ScreenWakeLockError::Internal)));
        }
    }

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `ScreenWakeLockHelper.RESULT_*` と同じ値であること。
        assert_eq!((RESULT_OK, RESULT_TIMEOUT, RESULT_FAILED), (0, 1, 2));
    }
}
