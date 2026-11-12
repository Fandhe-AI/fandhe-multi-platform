//! Android 向けアクセシビリティ設定実装（Issue #1308 で枠、#1310 で実装）。
//! `driver.rs::implementation` の Android 経路から呼ばれ、Java 側
//! `AccessibilityPrefsHelper.read(Context)` の static メソッドを JNI で橋渡しする。
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

use crate::contract::AccessibilityPrefs;
use crate::error::AccessibilityPrefsError;

/// 2 ビットの値（0 = 不明, 1 = オフ, 2 = オン）を契約の値へ戻す。
fn flag(bits: i32) -> Result<Option<bool>, AccessibilityPrefsError> {
    match bits {
        0 => Ok(None),
        1 => Ok(Some(false)),
        2 => Ok(Some(true)),
        _ => Err(AccessibilityPrefsError::Internal),
    }
}

/// Java が返した結果コードを設定値へ写す純関数（ホストの `cargo test` で固定する）。
fn prefs_from_code(code: i32) -> Result<AccessibilityPrefs, AccessibilityPrefsError> {
    if code < 0 || code >> 4 != 0 {
        return Err(AccessibilityPrefsError::Internal);
    }
    Ok(AccessibilityPrefs {
        reduce_motion: flag(code & 0b11)?,
        increase_contrast: flag((code >> 2) & 0b11)?,
    })
}

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use crate::contract::AccessibilityPrefsSource;

/// Android 実装（`AccessibilityPrefsHelper.read` を同期で呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidAccessibilityPrefs;

#[cfg(target_os = "android")]
impl AndroidAccessibilityPrefs {
    /// `AccessibilityPrefsHelper.read(Context)` を呼ぶ。Activity 未確立・JNI 失敗は `None`。
    /// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じている。
    fn call_read(&self) -> Option<i32> {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.AccessibilityPrefsHelper"),
            |env, activity, class| -> Result<i32, JniCallFailed> {
                let result = (|| {
                    let value = env.call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.content.Context) -> int),
                        &[jni::objects::JValue::Object(activity)],
                    )?;
                    value.i()
                })();
                if result.is_err() && env.exception_check() {
                    // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                    env.exception_clear();
                }
                Ok(result?)
            },
        )
        .ok()
    }
}

#[cfg(target_os = "android")]
impl AccessibilityPrefsSource for AndroidAccessibilityPrefs {
    fn read(&self) -> Result<AccessibilityPrefs, AccessibilityPrefsError> {
        // Activity 未確立は「非対応」ではなく一時的な失敗（Android は対応 OS）。
        let code = self.call_read().ok_or(AccessibilityPrefsError::Internal)?;
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
                matches!(
                    prefs_from_code(code),
                    Err(AccessibilityPrefsError::Internal)
                ),
                "code={code}"
            );
        }
    }
}
