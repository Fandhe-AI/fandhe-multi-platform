//! Android 向け単語分割実装（Issue #1095、#1187）。`driver.rs` の effect ドライバから egui スレッドで同期に
//! 呼ばれ、Java の `NlpHelper.segment(String)`（static、`android.icu.text.BreakIterator`）を JNI で呼ぶ。
//! 非同期コールバックを持たないため mailbox を介さず、戻り値（トークンを U+0000 で連結した文字列。
//! トークン 0 件は空文字列、失敗は null）を [`decode_android_tokens`] で写すだけで済む。
//!
//! 守る範囲: 入力検査（NUL・文字数上限）を通ったテキストだけを OS へ渡し、失敗を空の結果に畳まないこと。
//! 守らない範囲: OS 間の分割結果の一致。

use std::sync::Arc;

use crate::contract::{WordSegmenter, validate_input};
use crate::driver::decode_android_tokens;
use crate::error::NlpError;

/// Android の単語分割実装。
pub(crate) struct AndroidWordSegmenter;

#[cfg(target_os = "android")]
impl WordSegmenter for AndroidWordSegmenter {
    fn segment(&self, text: &str) -> Result<Vec<String>, NlpError> {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        validate_input(text)?;
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let Some(app) = android_app() else {
            return Err(NlpError::Internal);
        };
        let result = with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.NlpHelper"),
            |env, _activity, class| -> Result<Option<String>, JniCallFailed> {
                let j_text = env.new_string(text)?;
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("segment"),
                    jni::jni_sig!((java.lang.String) -> java.lang.String),
                    &[jni::objects::JValue::Object(&j_text)],
                );
                let ret = match ret {
                    Ok(v) => v,
                    Err(e) => {
                        // 例外は自動クリアされない（jni 0.22）ため後続の JNI 呼び出しへ
                        // 残さない。文言は読まない（security.md A09）。
                        if env.exception_check() {
                            env.exception_clear();
                        }
                        return Err(e.into());
                    }
                };
                let jstring = env.cast_local::<jni::objects::JString>(ret.l()?)?;
                if jstring.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(jstring.try_to_string(env)?))
                }
            },
        );
        match result {
            Ok(raw) => decode_android_tokens(raw),
            Err(_) => Err(NlpError::Internal),
        }
    }
}

/// ホストのテストビルドにも載せるための形。呼ばれても成功へは倒さない。
#[cfg(not(target_os = "android"))]
impl WordSegmenter for AndroidWordSegmenter {
    fn segment(&self, text: &str) -> Result<Vec<String>, NlpError> {
        validate_input(text)?;
        decode_android_tokens(None)
    }
}

#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn WordSegmenter> {
    Arc::new(AndroidWordSegmenter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_build_never_succeeds() {
        assert_eq!(AndroidWordSegmenter.segment("a"), Err(NlpError::Internal));
    }
}
