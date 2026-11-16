//! Android 向け OS の検索への登録（Issue #562、親 #117 の b-android）。
//! `driver` の Android 経路から、ドライバのワーカースレッドで呼ばれ、
//! `SpotlightIndexHelper.index` / `removeAll`（static。利用者の `MainActivity` に受け口は要らない）を同期 JNI で 1 回呼ぶ。
//! JNI 署名は `(Landroid/content/Context;…)I`。
//! Java 側は `SpotlightIndexHelper`（AppSearch の platform storage、API 31 以上）。
//! Java から Rust への native コールバックは無いので、JNI 署名の照合先は Java のみ。
//! Android に受信経路は無い。dispatch・画面・文言は触らない。
//!
//! # コード表（SSOT は Java `SpotlightIndexHelper`）
//!
//! 0 = 成功 / 1 = 非対応（API 31 未満） / 2 = 期限切れ / 3 = 失敗。
//! 成功は 0 だけで、未知のコード・JNI 失敗・Activity 無しは `Internal` に倒す。
//!
//! # 期限・ライフサイクル
//!
//! 期限は Rust のドライバだけが持つ。`remaining` は Java の完了待ちの上限で、0 は時間切れ。
//! Activity の作り直しでは Rust 側の保留記録を `reset_pending_tracking` が捨て、
//! 古い結果はドライバが要求 id の不一致で捨てる（Java は要求の状態を持たない）。
//!
//! 守る範囲: 失敗を成功へ畳まないこと・待ちの上限・1 プロセス内での要求どうしの順序（Java 側が前の要求の終端を待ち、期限切れ後の未発送の書き込みを止める）。
//! 守らない範囲: 検索への実際の表示（OEM 次第）・反映の時間差・期限後に OS 側で済んでいた場合の食い違い。
//! 識別子・題名・本文はログに出さない。実機確認は #555。

#[cfg(any(target_os = "android", test))]
use crate::contract::TIMEOUT;
#[cfg(any(target_os = "android", test))]
use crate::error::SpotlightIndexError;

/// 画面側へ見せる失敗の識別子（文言はドライバが固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "spotlight_index";

/// Java の結果コードを契約へ写す。成功は 0 だけ。
#[cfg(any(target_os = "android", test))]
fn decode(code: i32) -> Result<(), SpotlightIndexError> {
    match code {
        0 => Ok(()),
        1 => Err(SpotlightIndexError::Unsupported("spotlight_index")),
        2 => Err(SpotlightIndexError::Internal(TIMEOUT)),
        _ => Err(SpotlightIndexError::Internal(FAILED)),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::time::Duration;

    use crate::contract::{DOMAIN_ID, IndexItem, SpotlightIndex, TIMEOUT};
    use crate::error::SpotlightIndexError;
    use fandhe_platform_android::{JniCallFailed, with_activity_class};
    use jni::objects::{JClass, JObject, JValue};

    use super::{FAILED, decode};

    pub(super) struct AndroidSpotlightIndex;

    /// `SpotlightIndexHelper` の static メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じており、本ファイルには無い。
    fn call_helper<T>(
        f: impl FnOnce(&mut jni::Env, &JObject, &JClass) -> Result<T, JniCallFailed>,
    ) -> Option<T> {
        let app = fandhe_platform_android::android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.SpotlightIndexHelper"),
            |env, activity, class| {
                let r = f(env, activity, class);
                if r.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                r
            },
        )
        .ok()
    }

    fn millis(remaining: Duration) -> i64 {
        i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX)
    }

    impl SpotlightIndex for AndroidSpotlightIndex {
        fn index(
            &self,
            item: &IndexItem<'_>,
            remaining: Duration,
        ) -> Result<(), SpotlightIndexError> {
            let ms = millis(remaining);
            if ms == 0 {
                return Err(SpotlightIndexError::Internal(TIMEOUT));
            }
            let code = call_helper(|env, a, class| {
                let domain = env.new_string(DOMAIN_ID)?;
                let id = env.new_string(item.id)?;
                let title = env.new_string(item.title)?;
                let text = env.new_string(item.text)?;
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("index"),
                    jni::jni_sig!((android.content.Context, java.lang.String, java.lang.String, java.lang.String, java.lang.String, long) -> int),
                    &[
                        JValue::Object(a),
                        JValue::Object(&domain),
                        JValue::Object(&id),
                        JValue::Object(&title),
                        JValue::Object(&text),
                        JValue::Long(ms as jni::sys::jlong),
                    ],
                )?;
                Ok(ret.i()?)
            })
            .ok_or(SpotlightIndexError::Internal(FAILED))?;
            decode(code)
        }

        fn remove_all(&self, remaining: Duration) -> Result<(), SpotlightIndexError> {
            let ms = millis(remaining);
            if ms == 0 {
                return Err(SpotlightIndexError::Internal(TIMEOUT));
            }
            let code = call_helper(|env, a, class| {
                let domain = env.new_string(DOMAIN_ID)?;
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("removeAll"),
                    jni::jni_sig!((android.content.Context, java.lang.String, long) -> int),
                    &[
                        JValue::Object(a),
                        JValue::Object(&domain),
                        JValue::Long(ms as jni::sys::jlong),
                    ],
                )?;
                Ok(ret.i()?)
            })
            .ok_or(SpotlightIndexError::Internal(FAILED))?;
            decode(code)
        }
    }
}

/// Android の `SpotlightIndex` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::SpotlightIndex> {
    std::sync::Arc::new(imp::AndroidSpotlightIndex)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::SpotlightIndex> {
    std::sync::Arc::new(crate::contract::UnsupportedSpotlightIndex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_zero_is_success() {
        assert!(decode(0).is_ok());
        assert!(matches!(
            decode(1),
            Err(SpotlightIndexError::Unsupported(_))
        ));
        assert!(matches!(
            decode(2),
            Err(SpotlightIndexError::Internal(TIMEOUT))
        ));
        for bad in [3, -1, 4, 99] {
            assert!(matches!(
                decode(bad),
                Err(SpotlightIndexError::Internal(FAILED))
            ));
        }
    }
}
