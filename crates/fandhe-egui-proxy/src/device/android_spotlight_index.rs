//! Android 向け OS の検索への登録（Issue #562、親 #117 の b-android）。
//! `device/spotlight_index.rs` の Android 経路から、ドライバのワーカースレッドで呼ばれ、
//! `MainActivity.spotlightIndex` / `spotlightRemoveAll` を同期 JNI で 1 回呼ぶ。
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
//! 守る範囲: 失敗を成功へ畳まないこと・待ちの上限。
//! 守らない範囲: 検索への実際の表示（OEM 次第）・反映の時間差・期限後に OS 側で済んでいた場合の食い違い。
//! 識別子・題名・本文はログに出さない。実機確認は #555。

#[cfg(any(target_os = "android", test))]
use fandhe_device::spotlight_index::TIMEOUT;
#[cfg(any(target_os = "android", test))]
use fandhe_device::DeviceError;

/// 画面側へ見せる失敗の識別子（文言はドライバが固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "spotlight_index";

/// Java の結果コードを契約へ写す。成功は 0 だけ。
#[cfg(any(target_os = "android", test))]
fn decode(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::Unsupported("spotlight_index")),
        2 => Err(DeviceError::Internal(TIMEOUT)),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::time::Duration;

    use fandhe_device::spotlight_index::{IndexItem, SpotlightIndex, DOMAIN_ID, TIMEOUT};
    use fandhe_device::DeviceError;
    use jni::objects::{JObject, JValue};

    use super::{decode, FAILED};

    pub(super) struct AndroidSpotlightIndex;

    /// `MainActivity` の同期メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// 2 箇所の `unsafe` は `android_media_transcode.rs::call_activity` と同一の根拠。
    #[allow(unsafe_code)]
    fn call_activity<T>(
        f: impl FnOnce(&mut jni::Env, &JObject) -> Result<T, crate::device::android::JniCallFailed>,
    ) -> Option<T> {
        let app = crate::android::android_app()?;
        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
        // `app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<JObject>(&raw_activity)? };
            let r = f(env, &activity);
            if r.is_err() && env.exception_check() {
                env.exception_clear();
            }
            r
        })
        .ok()
    }

    fn millis(remaining: Duration) -> i64 {
        i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX)
    }

    impl SpotlightIndex for AndroidSpotlightIndex {
        fn index(&self, item: &IndexItem<'_>, remaining: Duration) -> Result<(), DeviceError> {
            let ms = millis(remaining);
            if ms == 0 {
                return Err(DeviceError::Internal(TIMEOUT));
            }
            let code = call_activity(|env, a| {
                let domain = env.new_string(DOMAIN_ID)?;
                let id = env.new_string(item.id)?;
                let title = env.new_string(item.title)?;
                let text = env.new_string(item.text)?;
                let ret = env.call_method(
                    a,
                    jni::jni_str!("spotlightIndex"),
                    jni::jni_sig!((java.lang.String, java.lang.String, java.lang.String, java.lang.String, long) -> int),
                    &[
                        JValue::Object(&domain),
                        JValue::Object(&id),
                        JValue::Object(&title),
                        JValue::Object(&text),
                        JValue::Long(ms as jni::sys::jlong),
                    ],
                )?;
                Ok(ret.i()?)
            })
            .ok_or(DeviceError::Internal(FAILED))?;
            decode(code)
        }

        fn remove_all(&self, remaining: Duration) -> Result<(), DeviceError> {
            let ms = millis(remaining);
            if ms == 0 {
                return Err(DeviceError::Internal(TIMEOUT));
            }
            let code = call_activity(|env, a| {
                let domain = env.new_string(DOMAIN_ID)?;
                let ret = env.call_method(
                    a,
                    jni::jni_str!("spotlightRemoveAll"),
                    jni::jni_sig!((java.lang.String, long) -> int),
                    &[JValue::Object(&domain), JValue::Long(ms as jni::sys::jlong)],
                )?;
                Ok(ret.i()?)
            })
            .ok_or(DeviceError::Internal(FAILED))?;
            decode(code)
        }
    }
}

/// Android の `SpotlightIndex` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::spotlight_index::SpotlightIndex>
{
    std::sync::Arc::new(imp::AndroidSpotlightIndex)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::spotlight_index::SpotlightIndex>
{
    std::sync::Arc::new(fandhe_device::spotlight_index::UnsupportedSpotlightIndex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_zero_is_success() {
        assert!(decode(0).is_ok());
        assert!(matches!(decode(1), Err(DeviceError::Unsupported(_))));
        assert!(matches!(decode(2), Err(DeviceError::Internal(TIMEOUT))));
        for bad in [3, -1, 4, 99] {
            assert!(matches!(decode(bad), Err(DeviceError::Internal(FAILED))));
        }
    }
}
