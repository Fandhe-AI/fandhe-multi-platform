//! Android 向けシステムの効果音（ビープ）実装（Issue #940、親 #183）。`device/system_sound.rs` の
//! Android 経路（egui スレッドから同期）から呼ばれ、`ToneGenerator.startTone(TONE_PROP_BEEP)` を
//! JNI で叩く。契約は `fandhe_device::system_sound`、Java 側と結果コードの SSOT は `SystemSoundHelper`。
//!
//! JNI: `MainActivity.systemSoundBeep() -> int`（`()I`）。UI スレッドへ post せず同期で戻る。
//! 結果コード: 0 = `Requested`（`startTone` が true）、1 = `NotPlayed`（false）、
//! それ以外（2 = 失敗・未知値・負値）と JNI 呼び出しの失敗は `Internal`。成功へは倒さない。
//!
//! Activity 再生成: 本機能は同期の一発呼び出しで保留要求を持たない。照合は driver の世代が担い、
//! 旧 Activity の `ToneGenerator` は `onDestroy` で解放される。
//!
//! 守る範囲: 1 プロセス内の順序・`NotPlayed`/失敗を成功に畳まないこと・破棄後に資源を作らないこと。
//! 守らない範囲: 実際に聞こえたか（消音・音量 0）、連打時の置き換え、背景中の押下。

use std::sync::Arc;

#[cfg(any(target_os = "android", test))]
use fandhe_device::system_sound::BeepOutcome;
use fandhe_device::system_sound::SystemSound;
#[cfg(any(target_os = "android", test))]
use fandhe_device::DeviceError;

/// `SystemSoundHelper.BEEP_REQUESTED` / `BEEP_NOT_PLAYED` の写し（`BEEP_FAILED = 2` は未知値として扱う）。
#[cfg(any(target_os = "android", test))]
const BEEP_REQUESTED: i32 = 0;
#[cfg(any(target_os = "android", test))]
const BEEP_NOT_PLAYED: i32 = 1;

/// JNI の結果コード（`None` は呼び出し失敗）を契約上の結果へ写す。未知値は成功にしない。
#[cfg(any(target_os = "android", test))]
fn decode_beep(code: Option<i32>) -> Result<BeepOutcome, DeviceError> {
    match code {
        Some(BEEP_REQUESTED) => Ok(BeepOutcome::Requested),
        Some(BEEP_NOT_PLAYED) => Ok(BeepOutcome::NotPlayed),
        _ => Err(DeviceError::Internal("system_sound")),
    }
}

#[cfg(target_os = "android")]
struct AndroidSystemSound;

/// `MainActivity.systemSoundBeep()` を JNI で呼ぶ。`None` は呼び出し失敗。`unsafe` 2 箇所は
/// `android_flashlight.rs` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_beep() -> Option<i32> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        Ok(env
            .call_method(
                &activity,
                jni::jni_str!("systemSoundBeep"),
                jni::jni_sig!(() -> int),
                &[],
            )?
            .i()?)
    })
    .ok()
}

#[cfg(target_os = "android")]
impl SystemSound for AndroidSystemSound {
    fn beep(&self) -> Result<BeepOutcome, DeviceError> {
        decode_beep(call_beep())
    }
}

/// Android の `SystemSound` 実装を返す（`device/system_sound.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn SystemSound> {
    Arc::new(AndroidSystemSound)
}

/// ホストのテストビルド向け。dispatch は Android でしか呼ばない。非対応を返し成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn SystemSound> {
    Arc::new(fandhe_device::system_sound::UnsupportedSystemSound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_impl_is_unsupported() {
        assert!(matches!(
            implementation().beep(),
            Err(DeviceError::Unsupported(_))
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
            assert!(matches!(decode_beep(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `SystemSoundHelper` の BEEP_REQUESTED / BEEP_NOT_PLAYED と同じ値。
        assert_eq!((BEEP_REQUESTED, BEEP_NOT_PLAYED), (0, 1));
    }
}
