//! Android 向けシステムのアクセント色実装（Issue #422）。
//! `driver.rs::platform_system_accent_color` の Android 経路から呼ばれ、Java 側
//! `SystemAccentColorHelper.read(Context)` を JNI の static 呼び出しで橋渡しする（`MainActivity` には何も書かない）。
//!
//! # 同期 pull（変化通知は購読しない）
//!
//! 画面は 1 秒間隔で `read()` を呼ぶ。コールバック登録・要求 id・保留状態を持たないので、
//! Activity 再生成・期限・世代の問題は構造上起きない（再生成後も `android_app()` が最新の
//! Activity を指すため、旧結果が新しい要求を消費する経路が無い）。
//!
//! # 結果コード表（JNI 署名 `()I`）
//!
//! 写像の SSOT は `crate::contract::rgb_from_code`。ここでは二重に持たず
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
use crate::contract::{AccentColorSource, Rgb, rgb_from_code};
#[cfg(target_os = "android")]
use crate::error::SystemAccentColorError;

/// Android 実装（`SystemAccentColorHelper.read` を static で同期に呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidAccentColor;

#[cfg(target_os = "android")]
impl AndroidAccentColor {
    /// `SystemAccentColorHelper.read(Context)` を呼ぶ。Activity 未確立・JNI 失敗は `None`。
    /// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じている。
    fn call_read(&self) -> Option<i32> {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.SystemAccentColorHelper"),
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
impl AccentColorSource for AndroidAccentColor {
    fn read(&self) -> Result<Rgb, SystemAccentColorError> {
        let code = self.call_read().ok_or(SystemAccentColorError::Internal)?;
        rgb_from_code(code)
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn AccentColorSource> {
    Arc::new(AndroidAccentColor)
}
