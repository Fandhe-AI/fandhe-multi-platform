//! Android 向け触覚フィードバック実装（Issue #1092、#1187）。`driver.rs` の effect ドライバから egui スレッドで
//! 同期に呼ばれ、Java の `HapticsHelper.tap(Context)`（static、`Vibrator.vibrate`）を JNI で呼ぶ。
//! 非同期コールバックを持たないため mailbox を介さず、戻り値を [`decode_haptics_result`] で写すだけで済む。
//! Java が返す結果コード（SSOT は `HapticsHelper.RESULT_*`）: 0 = 要求を渡した（サイレント・システム触覚オフでは
//! 無反応で、検知できない）/ 1 = 振動子なし（`Unsupported`）/ 2 = 失敗（`SecurityException` 等。`Internal`）。
//!
//! 守る範囲: 振動子が無い・失敗を成功に畳まないこと。守らない範囲: 実際に振動したかの検知。

use std::sync::Arc;

use crate::contract::Haptics;
use crate::driver::decode_haptics_result;
use crate::error::HapticsError;

/// Android の触覚フィードバック実装。
pub(crate) struct AndroidHaptics;

#[cfg(target_os = "android")]
impl Haptics for AndroidHaptics {
    fn tap(&self) -> Result<(), HapticsError> {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        let Some(app) = android_app() else {
            return Err(HapticsError::Internal);
        };
        let result = with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.HapticsHelper"),
            |env, activity, class| -> Result<i32, JniCallFailed> {
                let value = env.call_static_method(
                    class,
                    jni::jni_str!("tap"),
                    jni::jni_sig!((android.content.Context) -> int),
                    &[jni::objects::JValue::Object(activity)],
                )?;
                Ok(value.i()?)
            },
        );
        match result {
            Ok(code) => decode_haptics_result(code),
            Err(_) => Err(HapticsError::Internal),
        }
    }
}

/// ホストのテストビルドにも載せるための形。呼ばれても成功へは倒さない。
#[cfg(not(target_os = "android"))]
impl Haptics for AndroidHaptics {
    fn tap(&self) -> Result<(), HapticsError> {
        decode_haptics_result(-1)
    }
}

#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn Haptics> {
    Arc::new(AndroidHaptics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_build_never_succeeds() {
        assert_eq!(AndroidHaptics.tap(), Err(HapticsError::Internal));
    }
}
