//! Android 向け動画・音声の変換実装（Issue #314、親 #91 の b-android）。
//! `device/media_transcode.rs::platform` の Android 経路から、ドライバのワーカースレッド
//! （`media-transcode`）で呼ばれ、`MainActivity.mediaTranscode` を同期 JNI で 1 回呼ぶ。
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
use fandhe_device::media_transcode::TIMEOUT;
#[cfg(any(target_os = "android", test))]
use fandhe_device::DeviceError;

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
fn decode_reply(reply: &[u8]) -> Result<Vec<u8>, DeviceError> {
    match reply.split_first() {
        Some((0, rest)) => Ok(rest.to_vec()),
        Some((1, _)) => Err(DeviceError::Internal(TIMEOUT)),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::time::Duration;

    use fandhe_device::media_transcode::{validate_input, MediaTranscode, TIMEOUT};
    use fandhe_device::DeviceError;
    use jni::objects::{JByteArray, JObject, JValue};

    use super::{decode_reply, FAILED, MAX_M4A_BYTES, MAX_PCM_BYTES};

    pub(super) struct AndroidMediaTranscode;

    /// `MainActivity` の同期メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
    /// 2 箇所の `unsafe` は `android_compass.rs::call_activity` と同一の根拠。
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

    impl MediaTranscode for AndroidMediaTranscode {
        fn transcode(
            &self,
            pcm: &[i16],
            sample_rate: u32,
            remaining: Duration,
        ) -> Result<Vec<u8>, DeviceError> {
            validate_input(pcm, sample_rate)?;
            if pcm.len() * 2 > MAX_PCM_BYTES {
                return Err(DeviceError::InvalidInput("media_transcode input"));
            }
            let rate = i32::try_from(sample_rate)
                .map_err(|_| DeviceError::InvalidInput("media_transcode input"))?;
            let ms = i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX);
            if ms == 0 {
                return Err(DeviceError::Internal(TIMEOUT));
            }
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            let reply = call_activity(|env, a| {
                let j_pcm = env.byte_array_from_slice(&bytes)?;
                let ret = env.call_method(
                    a,
                    jni::jni_str!("mediaTranscode"),
                    jni::jni_sig!((byte[], int, long) -> byte[]),
                    &[
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
            .ok_or(DeviceError::Internal(FAILED))?;
            decode_reply(&reply)
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::media_transcode::MediaTranscode>
{
    std::sync::Arc::new(imp::AndroidMediaTranscode)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::media_transcode::MediaTranscode>
{
    std::sync::Arc::new(fandhe_device::media_transcode::UnsupportedMediaTranscode)
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
            Err(DeviceError::Internal(TIMEOUT))
        ));
        for bad in [&[][..], &[2][..], &[99, 1][..]] {
            assert!(matches!(
                decode_reply(bad),
                Err(DeviceError::Internal(FAILED))
            ));
        }
    }
}
