//! Android 向け集中モード・おやすみモードの状態実装（Issue #439）。
//! `device/focus_mode_status.rs::implementation` の Android 経路から呼ばれ、Java 側
//! `MainActivity.focusModeStatusRead`（`FocusModeStatusHelper`）を JNI で橋渡しする。
//!
//! # 同期 pull（変化通知は購読しない）
//!
//! 画面は 1 秒間隔で `read()` を呼ぶ。コールバック登録・要求 id・保留状態を持たないので、
//! Activity 再生成・期限・世代の問題は構造上起きない（再生成後も `android_app()` が最新の
//! Activity を指すため、旧結果が新しい要求を消費する経路が無い）。
//!
//! # 結果コード表（JNI 署名 `()I`）
//!
//! 写像の SSOT は `fandhe_device::focus_mode_status::focus_from_code`。Java の戻り値を
//! そのまま渡す（0 = オフ、1 = オン、それ以外は `Internal`）。Activity 未確立・JNI 失敗も
//! 一時的な状態なので `Unsupported` にせず `Internal`。
//!
//! # OS API
//!
//! `NotificationManager.getCurrentInterruptionFilter()`（API 23+、読み取りに権限不要）。
//! <https://developer.android.com/reference/android/app/NotificationManager#getCurrentInterruptionFilter()>
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 失敗・UNKNOWN・Activity 未確立を「オフ」にしない。状態（在席情報）をログへ出さない。
//! 守らない: フィルタ種類（優先のみ / 完全サイレント / アラームのみ）の区別、
//! 変化の反映遅れ（最大 1 秒 + 1 フレーム）、Android 15 以降のモード（Modes）との対応。

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::focus_mode_status::{focus_from_code, FocusStatusSource};
#[cfg(target_os = "android")]
use fandhe_device::DeviceError;

#[cfg(target_os = "android")]
const FAILED_MSG: &str = "focus_mode_status read";

/// Android 実装（`MainActivity.focusModeStatusRead` を同期で呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidFocusStatus;

#[cfg(target_os = "android")]
impl AndroidFocusStatus {
    /// `MainActivity.focusModeStatusRead()` を呼ぶ。Activity 未確立・JNI 失敗は `None`。
    /// 2 箇所の `unsafe` は `android_accessibility_prefs.rs::call_read` と同じ根拠・同じ形。
    #[allow(unsafe_code)]
    fn call_read(&self) -> Option<i32> {
        let app = crate::android::android_app()?;
        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
        // `android_app()` は最新の `AndroidApp` を返す（`call_schedule` と同一の根拠）。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(
            |env: &mut jni::Env| -> Result<i32, super::android::JniCallFailed> {
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: `Cast` は所有権を持たず delete しない（`call_schedule` と同一の根拠）。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                let ret = env.call_method(
                    &activity,
                    jni::jni_str!("focusModeStatusRead"),
                    jni::jni_sig!(() -> int),
                    &[],
                );
                let value = match ret {
                    Ok(v) => v,
                    Err(e) => {
                        // 例外は自動クリアされない（jni 0.22）。文言は読まない。
                        if env.exception_check() {
                            env.exception_clear();
                        }
                        return Err(e.into());
                    }
                };
                Ok(value.i()?)
            },
        )
        .ok()
    }
}

#[cfg(target_os = "android")]
impl FocusStatusSource for AndroidFocusStatus {
    fn read(&self) -> Result<bool, DeviceError> {
        let code = self.call_read().ok_or(DeviceError::Internal(FAILED_MSG))?;
        focus_from_code(code)
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn FocusStatusSource> {
    Arc::new(AndroidFocusStatus)
}
