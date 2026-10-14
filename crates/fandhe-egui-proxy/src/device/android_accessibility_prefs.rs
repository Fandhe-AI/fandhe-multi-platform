//! Android 向けアクセシビリティ設定実装（Issue #1308 で枠、#1310 で実装）。
//! `device/accessibility_prefs.rs::implementation` の Android 経路から呼ばれ、Java 側
//! `MainActivity.accessibilityPrefsRead`（`AccessibilityPrefsHelper`）を JNI で橋渡しする。
//!
//! # 同期 pull（変化通知は購読しない）
//!
//! 画面は 1 秒間隔で `read()` を呼ぶ。コールバック登録・要求 id・保留状態を持たないので、
//! Activity 再生成・期限・世代の問題は構造上起きない（再生成後も `android_app()` が最新の
//! Activity を指すため、旧結果が新しい要求を消費する経路が無い）。
//!
//! # 結果コード表（SSOT は Java の `AccessibilityPrefsHelper` 定数。JNI 署名 `()I`）
//!
//! Apple 実装（`apple_accessibility_prefs.rs`）と同じ詰め方。
//!
//! | 値 | 意味 | 写像 |
//! |---|---|---|
//! | 負値 | 取得失敗 | `Internal` |
//! | bit0-1 | 視差軽減（0 = 不明, 1 = オフ, 2 = オン） | `None` / `Some(false)` / `Some(true)` |
//! | bit2-3 | コントラスト増加（同上） | 同上 |
//!
//! 値 3・bit4 以上は `Internal`（成功にしない）。
//!
//! # OS API
//!
//! - 視差軽減: `Settings.Global.ANIMATOR_DURATION_SCALE == 0` を「オン」とみなす（契約どおり）。
//!   未設定時の既定 1.0 は OS の既定と同じなので「オフ」は嘘にならない。
//! - コントラスト: `UiModeManager.getContrast() > 0`（API 34+）。API 33 以下は不明。
//!   `AccessibilityManager.isHighContrastTextEnabled` は非公開 API のため使わない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 失敗・Activity 未確立・未定義コードを「オン」「オフ」にしない。設定値をログへ出さない。
//! 守らない: 変化の反映遅れ（最大 1 秒 + 1 フレーム）、OS 間の意味の一致、API 33 以下の
//! コントラスト、バッテリーセーバー等の別経路によるアニメ無効化。

use fandhe_device::accessibility_prefs::AccessibilityPrefs;
use fandhe_device::DeviceError;

const FAILED_MSG: &str = "accessibility_prefs";

/// 2 ビットの値（0 = 不明, 1 = オフ, 2 = オン）を契約の値へ戻す。
fn flag(bits: i32) -> Result<Option<bool>, DeviceError> {
    match bits {
        0 => Ok(None),
        1 => Ok(Some(false)),
        2 => Ok(Some(true)),
        _ => Err(DeviceError::Internal(FAILED_MSG)),
    }
}

/// Java が返した結果コードを設定値へ写す純関数（ホストの `cargo test` で固定する）。
fn prefs_from_code(code: i32) -> Result<AccessibilityPrefs, DeviceError> {
    if code < 0 || code >> 4 != 0 {
        return Err(DeviceError::Internal(FAILED_MSG));
    }
    Ok(AccessibilityPrefs {
        reduce_motion: flag(code & 0b11)?,
        increase_contrast: flag((code >> 2) & 0b11)?,
    })
}

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::accessibility_prefs::AccessibilityPrefsSource;

/// Android 実装（`MainActivity.accessibilityPrefsRead` を同期で呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidAccessibilityPrefs;

#[cfg(target_os = "android")]
impl AndroidAccessibilityPrefs {
    /// `MainActivity.accessibilityPrefsRead()` を呼ぶ。Activity 未確立・JNI 失敗は `None`。
    /// 2 箇所の `unsafe` は `android_background_task.rs::call_schedule` と同じ根拠・同じ形。
    #[allow(unsafe_code)]
    fn call_read(&self) -> Option<i32> {
        let app = crate::android::android_app()?;
        // SAFETY: `call_schedule` と同一の根拠。`app.vm_as_ptr()` は android-activity が管理する
        // 有効な `JavaVM*` で、`android_app()` は最新の `AndroidApp` を返す。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(
            |env: &mut jni::Env| -> Result<i32, super::android::JniCallFailed> {
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: `call_schedule` と同一の根拠。`Cast` は所有権を持たず delete しない。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                let value = env.call_method(
                    &activity,
                    jni::jni_str!("accessibilityPrefsRead"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?;
                Ok(value.i()?)
            },
        )
        .ok()
    }
}

#[cfg(target_os = "android")]
impl AccessibilityPrefsSource for AndroidAccessibilityPrefs {
    fn read(&self) -> Result<AccessibilityPrefs, DeviceError> {
        // Activity 未確立は「非対応」ではなく一時的な失敗（Android は対応 OS）。
        let code = self.call_read().ok_or(DeviceError::Internal(FAILED_MSG))?;
        prefs_from_code(code)
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn AccessibilityPrefsSource> {
    Arc::new(AndroidAccessibilityPrefs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(m: Option<bool>, c: Option<bool>) -> AccessibilityPrefs {
        AccessibilityPrefs {
            reduce_motion: m,
            increase_contrast: c,
        }
    }

    #[test]
    fn decodes_each_state() {
        let cases = [
            (0, prefs(None, None)),
            (1 | (2 << 2), prefs(Some(false), Some(true))),
            (2 | (1 << 2), prefs(Some(true), Some(false))),
            (2, prefs(Some(true), None)),
        ];
        for (code, want) in cases {
            assert_eq!(prefs_from_code(code).unwrap(), want, "code={code}");
        }
    }

    #[test]
    fn invalid_codes_are_internal() {
        for code in [-1, i32::MIN, 3, 3 << 2, 1 << 4, i32::MAX] {
            assert!(
                matches!(prefs_from_code(code), Err(DeviceError::Internal(_))),
                "code={code}"
            );
        }
    }
}
