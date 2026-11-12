//! Android 向け集中モード・おやすみモードの状態実装（Issue #439）。
//! `driver.rs::platform_focus_mode_status` の Android 経路から呼ばれ、Java 側
//! `FocusModeStatusHelper.read(Context)` を JNI の static 呼び出しで橋渡しする（`MainActivity` には何も書かない）。
//!
//! # 同期 pull（変化通知は購読しない）
//!
//! 画面は 1 秒間隔で `read()` を呼ぶ。コールバック登録・要求 id・保留状態を持たないので、
//! Activity 再生成・期限・世代の問題は構造上起きない（再生成後も `android_app()` が最新の
//! Activity を指すため、旧結果が新しい要求を消費する経路が無い）。
//!
//! # 結果コード表（JNI 署名 `(Landroid/content/Context;)I`）
//!
//! 写像の SSOT は `crate::contract::focus_from_code`。Java の戻り値を
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
use crate::contract::{FocusStatusSource, focus_from_code};
#[cfg(target_os = "android")]
use crate::error::FocusModeStatusError;

/// Android 実装（`FocusModeStatusHelper.read` を static で同期に呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidFocusStatus;

#[cfg(target_os = "android")]
impl AndroidFocusStatus {
    /// `FocusModeStatusHelper.read(Context)` を呼ぶ。Activity 未確立・JNI 失敗は `None`。
    /// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じている。
    fn call_read(&self) -> Option<i32> {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.FocusModeStatusHelper"),
            |env, activity, class| -> Result<i32, JniCallFailed> {
                let value = env.call_static_method(
                    class,
                    jni::jni_str!("read"),
                    jni::jni_sig!((android.content.Context) -> int),
                    &[jni::objects::JValue::Object(activity)],
                )?;
                Ok(value.i()?)
            },
        )
        .ok()
    }
}

#[cfg(target_os = "android")]
impl FocusStatusSource for AndroidFocusStatus {
    fn read(&self) -> Result<bool, FocusModeStatusError> {
        let code = self.call_read().ok_or(FocusModeStatusError::Internal)?;
        focus_from_code(code)
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn FocusStatusSource> {
    Arc::new(AndroidFocusStatus)
}
