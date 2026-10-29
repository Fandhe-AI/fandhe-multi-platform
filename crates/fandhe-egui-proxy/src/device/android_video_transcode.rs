//! Android 向け動画の変換実装（Issue #1017、親 #340 の b-android）。
//! `device/video_transcode.rs::platform` の Android 経路から、ドライバのワーカースレッドで呼ばれ、
//! `MainActivity.videoTranscode` を同期 JNI で 1 回呼ぶ。Java 側は `VideoTranscodeHelper`
//! （`MediaCodec` で元動画を作り、復号・縮小・符号化し、`MediaMuxer` で MPEG-4）。
//! 元動画の絵は Java が YUV で直接描く（巨大な BGRA を JNI で渡さないため）。
//! Java から Rust への native コールバックは無いので、JNI 署名の照合先は Java のみ。
//!
//! # コード表（SSOT は Java `VideoTranscodeHelper`）
//!
//! 戻りの `byte[]` の先頭 1 バイト: 0 = 完了（残りが mp4 本体）/ 1 = 期限切れ / 2 = 失敗 /
//! 3 = 非対応（`isSizeSupported` が偽・codec を作れない）/ 4 = 前回の解放を確認できない。
//! null・空・未知のコードは `Internal` に倒し、成功にはしない。mp4 の妥当性は
//! ドライバの `check_mp4_video` だけが決める（Java は成功を判定しない）。
//!
//! # 期限・ライフサイクル
//!
//! 期限は呼び出しに渡す残り時間で、Java が単調時計で確かめて中断する（資源を解放してから 1 を返す）。
//! 変換は短いため背景遷移では中断しない。Activity の作り直しでは Rust 側の保留記録を
//! `device/video_transcode.rs::reset_pending_tracking` が捨てる（要求 id は単調に増え、古いワーカーの
//! 結果は古いチャネルへ届くので、新しい要求の id を消費しない）。Java の実行中フラグは static なので、
//! 古い呼び出しが走り切るまでの新しい要求は 4 で失敗にする。
//!
//! 守る範囲: 失敗を成功値へ畳まないこと・資源の解放・FFI に渡す長さの上限。
//! 守らない範囲: 端末ごとの所要時間・ビットレート・符号形式の違い、複数プロセス間。
//! 映像・出力・OS の生エラー値はログに出さない。実機確認は #1006。

#[cfg(any(target_os = "android", test))]
use fandhe_device::video_transcode::{RELEASE_PENDING, TIMEOUT};
#[cfg(any(target_os = "android", test))]
use fandhe_device::DeviceError;

/// 受け取る mp4 の上限（Java 側と同じ。確保の前に長さを検査する）。
#[cfg(any(target_os = "android", test))]
const MAX_MP4_BYTES: usize = 4 * 1024 * 1024;
/// 画面側へ見せる失敗の識別子（文言はドライバの `to_failure` が固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "video_transcode android";

/// Java の戻り（先頭 1 バイト = コード）を契約へ写す。成功以外はすべて `Err`。
#[cfg(any(target_os = "android", test))]
fn decode_reply(reply: &[u8]) -> Result<Vec<u8>, DeviceError> {
    match reply.split_first() {
        Some((0, rest)) => Ok(rest.to_vec()),
        Some((1, _)) => Err(DeviceError::Internal(TIMEOUT)),
        Some((3, _)) => Err(DeviceError::Unsupported("video_transcode")),
        Some((4, _)) => Err(DeviceError::Internal(RELEASE_PENDING)),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::time::Duration;

    use fandhe_device::video_transcode::{
        VideoTranscode, SOURCE_FPS, SOURCE_FRAMES, SOURCE_HEIGHT, SOURCE_WIDTH, TARGET_HEIGHT,
        TARGET_WIDTH, TIMEOUT,
    };
    use fandhe_device::DeviceError;
    use jni::objects::{JByteArray, JObject, JValue};

    use super::{decode_reply, FAILED, MAX_MP4_BYTES};

    pub(super) struct AndroidVideoTranscode;

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

    impl VideoTranscode for AndroidVideoTranscode {
        fn transcode(&self, remaining: Duration) -> Result<Vec<u8>, DeviceError> {
            let ms = i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX);
            if ms == 0 {
                return Err(DeviceError::Internal(TIMEOUT));
            }
            let reply = call_activity(|env, a| {
                let ret = env.call_method(
                    a,
                    jni::jni_str!("videoTranscode"),
                    jni::jni_sig!((int, int, int, int, int, int, long) -> byte[]),
                    &[
                        // 寸法・fps・フレーム数は fandhe_device の定数が SSOT（Java へ複製しない）。
                        JValue::Int(SOURCE_WIDTH as i32),
                        JValue::Int(SOURCE_HEIGHT as i32),
                        JValue::Int(SOURCE_FPS as i32),
                        JValue::Int(SOURCE_FRAMES as i32),
                        JValue::Int(TARGET_WIDTH as i32),
                        JValue::Int(TARGET_HEIGHT as i32),
                        JValue::Long(ms as jni::sys::jlong),
                    ],
                )?;
                let arr = env.cast_local::<JByteArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(None);
                }
                // 長さを検証してから確保・コピーする。超過は失敗コード 2 の 1 バイトに倒す。
                if arr.len(env)? > 1 + MAX_MP4_BYTES {
                    return Ok(Some(vec![2]));
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
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::video_transcode::VideoTranscode>
{
    std::sync::Arc::new(imp::AndroidVideoTranscode)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::video_transcode::VideoTranscode>
{
    std::sync::Arc::new(fandhe_device::video_transcode::UnsupportedVideoTranscode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_codes_never_fake_success() {
        assert_eq!(decode_reply(&[0, 1, 2, 3]).unwrap(), vec![1, 2, 3]);
        // 0 バイトの本体はそのまま返す（EMPTY の判定はドライバの check_mp4_video）。
        assert_eq!(decode_reply(&[0]).unwrap(), Vec::<u8>::new());
        assert!(matches!(
            decode_reply(&[1]),
            Err(DeviceError::Internal(TIMEOUT))
        ));
        assert!(matches!(
            decode_reply(&[3]),
            Err(DeviceError::Unsupported("video_transcode"))
        ));
        assert!(matches!(
            decode_reply(&[4]),
            Err(DeviceError::Internal(RELEASE_PENDING))
        ));
        for bad in [&[][..], &[2][..], &[99, 1][..]] {
            assert!(matches!(
                decode_reply(bad),
                Err(DeviceError::Internal(FAILED))
            ));
        }
    }
}
