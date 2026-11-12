//! Android 向け動画・音声の変換実装（Issue #314、親 #91 の b-android）。
//! `device/media_transcode.rs::platform` の Android 経路から、ドライバのワーカースレッド
//! （`media-transcode`）で呼ばれ、`MediaTranscodeHelper.transcode`（static）を同期 JNI で 1 回呼ぶ。
//! Java 側は `MediaTranscodeHelper`（`MediaCodec` の AAC エンコーダ + `MediaMuxer` の MPEG-4）。
//! Java から Rust への native コールバックは無いので、JNI 署名の照合先は Java のみ。
//!
//! # コード表（SSOT は Java `MediaTranscodeHelper`）
//!
//! 戻りの `byte[]` の先頭 1 バイト: 0 = 完了（残りが m4a 本体）/ 1 = 期限切れ / 2 = 失敗。
//! null・空・未知のコードは `Internal` に倒し、成功にはしない。m4a の妥当性は
//! ドライバの `check_m4a` だけが決める（Java は成功を判定しない）。
//!
//! # 期限・ライフサイクル
//!
//! 期限は呼び出しに渡す残り時間で、Java が単調時計で確かめて中断する（資源を解放してから 1 を返す）。
//! 変換は短いため背景遷移では中断しない。Activity の作り直しでは Rust 側の保留記録を
//! `device/media_transcode.rs::reset_pending_tracking` が捨てる（要求 id は単調に増えるので、
//! 古い要求の後始末が新しい要求を消すことはない）。
//!
//! 守る範囲: 失敗を成功値へ畳まないこと・資源の解放・FFI に渡す長さの上限。
//! 守らない範囲: 端末ごとの所要時間とビットレートの違い、複数プロセス間。
//! PCM・出力・OS の生エラー値はログに出さない。実機確認は #310。

#[cfg(any(target_os = "android", test))]
use crate::contract::TIMEOUT;
#[cfg(any(target_os = "android", test))]
use crate::error::MediaTranscodeError;

/// 渡せる PCM の上限（Java 側と同じ。異常に大きい入力を FFI に渡さない）。
#[cfg(any(target_os = "android", test))]
const MAX_PCM_BYTES: usize = 16 * 1024 * 1024;
/// 受け取る m4a の上限（Java 側と同じ。確保の前に長さを検査する）。
#[cfg(any(target_os = "android", test))]
const MAX_M4A_BYTES: usize = 4 * 1024 * 1024;
/// 画面側へ見せる失敗の識別子（文言はドライバの `to_failure` が固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "media_transcode android";

/// Java の戻り（先頭 1 バイト = コード）を契約へ写す。成功以外はすべて `Err`。
#[cfg(any(target_os = "android", test))]
fn decode_reply(reply: &[u8]) -> Result<Vec<u8>, MediaTranscodeError> {
    match reply.split_first() {
        Some((0, rest)) => Ok(rest.to_vec()),
        Some((1, _)) => Err(MediaTranscodeError::Internal(TIMEOUT)),
        _ => Err(MediaTranscodeError::Internal(FAILED)),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::time::Duration;

    use crate::contract::{MediaTranscode, TIMEOUT, validate_input};
    use crate::error::MediaTranscodeError;
    use jni::objects::{JByteArray, JObject, JValue};

    use super::{FAILED, MAX_M4A_BYTES, MAX_PCM_BYTES, decode_reply};

    pub(super) struct AndroidMediaTranscode;

    /// `MediaTranscodeHelper` の static メソッドを同期で呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じている。
    fn call_helper<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &JObject,
            &jni::objects::JClass,
        ) -> Result<T, fandhe_platform_android::JniCallFailed>,
    ) -> Option<T> {
        let app = fandhe_platform_android::android_app()?;
        fandhe_platform_android::with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.MediaTranscodeHelper"),
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

    impl MediaTranscode for AndroidMediaTranscode {
        fn transcode(
            &self,
            pcm: &[i16],
            sample_rate: u32,
            remaining: Duration,
        ) -> Result<Vec<u8>, MediaTranscodeError> {
            validate_input(pcm, sample_rate)?;
            if pcm.len() * 2 > MAX_PCM_BYTES {
                return Err(MediaTranscodeError::InvalidInput);
            }
            let rate = i32::try_from(sample_rate).map_err(|_| MediaTranscodeError::InvalidInput)?;
            let ms = i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX);
            if ms == 0 {
                return Err(MediaTranscodeError::Internal(TIMEOUT));
            }
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            let reply = call_helper(|env, activity, class| {
                let j_pcm = env.byte_array_from_slice(&bytes)?;
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("transcode"),
                    jni::jni_sig!((android.content.Context, byte[], int, long) -> byte[]),
                    &[
                        JValue::Object(activity),
                        JValue::Object(&j_pcm),
                        JValue::Int(rate),
                        JValue::Long(ms as jni::sys::jlong),
                    ],
                )?;
                let arr = env.cast_local::<JByteArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(None);
                }
                // 長さを検証してから確保・コピーする。
                if arr.len(env)? > 1 + MAX_M4A_BYTES {
                    return Ok(Some(Vec::new()));
                }
                Ok(Some(env.convert_byte_array(&arr)?))
            })
            .flatten()
            .ok_or(MediaTranscodeError::Internal(FAILED))?;
            decode_reply(&reply)
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::MediaTranscode> {
    std::sync::Arc::new(imp::AndroidMediaTranscode)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::MediaTranscode> {
    std::sync::Arc::new(crate::contract::UnsupportedMediaTranscode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_codes_never_fake_success() {
        assert_eq!(decode_reply(&[0, 1, 2, 3]).unwrap(), vec![1, 2, 3]);
        // 0 バイトの本体はそのまま返す（EMPTY の判定はドライバの check_m4a）。
        assert_eq!(decode_reply(&[0]).unwrap(), Vec::<u8>::new());
        assert!(matches!(
            decode_reply(&[1]),
            Err(MediaTranscodeError::Internal(TIMEOUT))
        ));
        for bad in [&[][..], &[2][..], &[99, 1][..]] {
            assert!(matches!(
                decode_reply(bad),
                Err(MediaTranscodeError::Internal(FAILED))
            ));
        }
    }
}
