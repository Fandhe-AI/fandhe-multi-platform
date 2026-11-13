//! Android 向け文字サイズ追従実装（`Configuration.fontScale`、Issue #914）。`driver` の Android 経路から
//! 呼ばれ、Java 側 `TextScaleHelper.textScaleFactor(Context)` の static メソッドを JNI で同期に呼ぶ。
//!
//! JNI 窓口の署名は Java（`TextScaleHelper`）と本ファイルの `jni_sig!` の 2 か所でそろえる。
//! コールバックも保留状態も持たない純 pull のため、期限・世代・Activity 再生成の問題は構造上起きない。
//! NaN・範囲外の丸めは `read_scale` に一本化し、ここでは JNI から得た値をそのまま返す。
//! JNI 呼び出し自体の失敗（Activity 未確立・メソッド未検出・例外）は `Internal`（「使えない」とは区別する）。

use crate::contract::TextScale;
use crate::error::DynamicTypeError;

/// Android 実文字サイズ追従実装。
pub(crate) struct AndroidTextScale;

impl TextScale for AndroidTextScale {
    fn scale_factor(&self) -> Result<f32, DynamicTypeError> {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        let app = android_app().ok_or(DynamicTypeError::Unsupported)?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.TextScaleHelper"),
            |env, activity, class| -> Result<f32, JniCallFailed> {
                let result = (|| {
                    let value = env.call_static_method(
                        class,
                        jni::jni_str!("textScaleFactor"),
                        jni::jni_sig!((android.content.Context) -> float),
                        &[jni::objects::JValue::Object(activity)],
                    )?;
                    value.f()
                })();
                if result.is_err() && env.exception_check() {
                    // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                    env.exception_clear();
                }
                Ok(result?)
            },
        )
        .map_err(|_| DynamicTypeError::Internal)
    }
}
