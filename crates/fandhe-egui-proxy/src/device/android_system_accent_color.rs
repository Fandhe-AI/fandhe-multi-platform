//! Android 向けシステムのアクセント色実装（Issue #422）。
//! `device/system_accent_color.rs::implementation` の Android 経路から呼ばれ、Java 側
//! `MainActivity.systemAccentColorRead`（`SystemAccentColorHelper`）を JNI で橋渡しする。
//!
//! # 同期 pull（変化通知は購読しない）
//!
//! 画面は 1 秒間隔で `read()` を呼ぶ。コールバック登録・要求 id・保留状態を持たないので、
//! Activity 再生成・期限・世代の問題は構造上起きない（再生成後も `android_app()` が最新の
//! Activity を指すため、旧結果が新しい要求を消費する経路が無い）。
//!
//! # 結果コード表（JNI 署名 `()I`）
//!
//! 写像の SSOT は `fandhe_device::system_accent_color::rgb_from_code`。ここでは二重に持たず
//! Java の戻り値をそのまま渡す（`-2` = API 30 以下で `Unsupported`、`-1` と範囲外は `Internal`）。
//! Activity 未確立・JNI 失敗は一時的な状態なので `Unsupported` にせず `Internal`。
//!
//! # OS API
//!
//! `Resources.getColor(android.R.color.system_accent1_500)`（API 31+）。
//! <https://developer.android.com/reference/android/R.color#system_accent1_500>
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 失敗・非対応・Activity 未確立を既定色や前回値にしない。色の値をログへ出さない。
//! 守らない: 変化の反映遅れ（最大 1 秒 + 1 フレーム）、AOSP 既定値と壁紙由来の色の区別、
//! Material 3 の `colorPrimary` との一致、壁紙変更後の Resources 反映（実機未確認、#415）。

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::system_accent_color::{rgb_from_code, AccentColorSource, Rgb};
#[cfg(target_os = "android")]
use fandhe_device::DeviceError;

#[cfg(target_os = "android")]
const FAILED_MSG: &str = "system_accent_color read";

/// Android 実装（`MainActivity.systemAccentColorRead` を同期で呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidAccentColor;

#[cfg(target_os = "android")]
impl AndroidAccentColor {
    /// `MainActivity.systemAccentColorRead()` を呼ぶ。Activity 未確立・JNI 失敗は `None`。
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
                    jni::jni_str!("systemAccentColorRead"),
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
impl AccentColorSource for AndroidAccentColor {
    fn read(&self) -> Result<Rgb, DeviceError> {
        let code = self.call_read().ok_or(DeviceError::Internal(FAILED_MSG))?;
        rgb_from_code(code)
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn AccentColorSource> {
    Arc::new(AndroidAccentColor)
}
