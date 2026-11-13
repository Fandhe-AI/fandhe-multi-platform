//! Android 向けシステムの効果音（ビープ）実装（Issue #940、親 #183）。`driver.rs` の
//! Android 経路（egui スレッドから同期）から呼ばれ、`ToneGenerator.startTone(TONE_PROP_BEEP)` を
//! JNI で叩く。契約は `crate::contract`、Java 側と結果コードの SSOT は `SystemSoundHelper`。
//!
//! JNI: static の `SystemSoundHelper.beep(Activity) -> int`（`(Landroid/app/Activity;)I`）。UI スレッドへ post せず同期で戻る。
//! 結果コード: 0 = `Requested`（`startTone` が true）、1 = `NotPlayed`（false）、
//! それ以外（2 = 失敗・未知値・負値）と JNI 呼び出しの失敗は `Internal`。成功へは倒さない。
//!
//! Activity 再生成: 本機能は同期の一発呼び出しで保留要求を持たない。照合は driver の世代が担い、
//! 旧 Activity の `ToneGenerator` は `SystemSoundLifecycle` の `onHostDestroyed` で解放される。
//!
//! 守る範囲: 1 プロセス内の順序・`NotPlayed`/失敗を成功に畳まないこと・破棄後に資源を作らないこと。
//! 守らない範囲: 実際に聞こえたか（消音・音量 0）、連打時の置き換え、背景中の押下。

use std::sync::Arc;

#[cfg(any(target_os = "android", test))]
use crate::contract::BeepOutcome;
use crate::contract::SystemSound;
#[cfg(any(target_os = "android", test))]
use crate::error::SystemSoundError;

/// `SystemSoundHelper.BEEP_REQUESTED` / `BEEP_NOT_PLAYED` の写し（`BEEP_FAILED = 2` は未知値として扱う）。
#[cfg(any(target_os = "android", test))]
const BEEP_REQUESTED: i32 = 0;
#[cfg(any(target_os = "android", test))]
const BEEP_NOT_PLAYED: i32 = 1;

/// JNI の結果コード（`None` は呼び出し失敗）を契約上の結果へ写す。未知値は成功にしない。
#[cfg(any(target_os = "android", test))]
fn decode_beep(code: Option<i32>) -> Result<BeepOutcome, SystemSoundError> {
    match code {
        Some(BEEP_REQUESTED) => Ok(BeepOutcome::Requested),
        Some(BEEP_NOT_PLAYED) => Ok(BeepOutcome::NotPlayed),
        _ => Err(SystemSoundError::Internal("system_sound")),
    }
}

#[cfg(target_os = "android")]
struct AndroidSystemSound;

/// `SystemSoundHelper.beep(Activity)` を JNI で呼ぶ。`None` は呼び出し失敗。クラスは Activity の
/// class loader で解決する（`fandhe_platform_android::with_activity_class`）。
#[cfg(target_os = "android")]
fn call_beep() -> Option<i32> {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    use jni::objects::JValue;
    let app = android_app()?;
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.SystemSoundHelper"),
        |env, activity, class| -> Result<i32, JniCallFailed> {
            Ok(env
                .call_static_method(
                    class,
                    jni::jni_str!("beep"),
                    jni::jni_sig!((android.app.Activity) -> int),
                    &[JValue::Object(activity)],
                )?
                .i()?)
        },
    )
    .ok()
}

#[cfg(target_os = "android")]
impl SystemSound for AndroidSystemSound {
    fn beep(&self) -> Result<BeepOutcome, SystemSoundError> {
        decode_beep(call_beep())
    }
}

/// Android の `SystemSound` 実装を返す（`driver.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn SystemSound> {
    Arc::new(AndroidSystemSound)
}

/// ホストのテストビルド向け。dispatch は Android でしか呼ばない。非対応を返し成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn SystemSound> {
    Arc::new(crate::contract::UnsupportedSystemSound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_impl_is_unsupported() {
        assert!(matches!(
            implementation().beep(),
            Err(SystemSoundError::Unsupported)
        ));
    }

    #[test]
    fn decode_maps_known_codes() {
        assert!(matches!(decode_beep(Some(0)), Ok(BeepOutcome::Requested)));
        assert!(matches!(decode_beep(Some(1)), Ok(BeepOutcome::NotPlayed)));
    }

    #[test]
    fn decode_never_folds_failure_into_success() {
        for c in [Some(2), Some(99), Some(-1), None] {
            assert!(matches!(decode_beep(c), Err(SystemSoundError::Internal(_))));
        }
    }

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `SystemSoundHelper` の BEEP_REQUESTED / BEEP_NOT_PLAYED と同じ値。
        assert_eq!((BEEP_REQUESTED, BEEP_NOT_PLAYED), (0, 1));
    }
}
