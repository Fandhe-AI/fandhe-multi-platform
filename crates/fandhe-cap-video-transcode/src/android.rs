//! Android 向け動画の変換実装（Issue #1017、親 #340 の b-android）。
//! `driver::platform_video_transcode` の Android 経路から、ドライバのワーカースレッドで呼ばれ、
//! `VideoTranscodeHelper.transcode(Context, ...)`（static）を同期 JNI で 1 回呼ぶ。Java 側は `VideoTranscodeHelper`
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
//! `driver::reset_pending_tracking` が捨てる（要求 id は単調に増え、古いワーカーの
//! 結果は古いチャネルへ届くので、新しい要求の id を消費しない）。Java の実行中フラグは static なので、
//! 古い呼び出しが走り切るまでの新しい要求は 4 で失敗にする。
//!
//! 守る範囲: 失敗を成功値へ畳まないこと・資源の解放・FFI に渡す長さの上限。
//! 守らない範囲: 端末ごとの所要時間・ビットレート・符号形式の違い、複数プロセス間。
//! 映像・出力・OS の生エラー値はログに出さない。実機確認は #1006。

#[cfg(any(target_os = "android", test))]
use crate::contract::{RELEASE_PENDING, TIMEOUT};
#[cfg(any(target_os = "android", test))]
use crate::error::VideoTranscodeError;

/// 受け取る mp4 の上限（Java 側と同じ。確保の前に長さを検査する）。
#[cfg(any(target_os = "android", test))]
const MAX_MP4_BYTES: usize = 4 * 1024 * 1024;
/// 画面側へ見せる失敗の識別子（文言はドライバの `to_failure` が固定文言にする）。
#[cfg(any(target_os = "android", test))]
const FAILED: &str = "video_transcode android";

/// Java の戻り（先頭 1 バイト = コード）を契約へ写す。成功以外はすべて `Err`。
#[cfg(any(target_os = "android", test))]
fn decode_reply(reply: &[u8]) -> Result<Vec<u8>, VideoTranscodeError> {
    match reply.split_first() {
        Some((0, rest)) => Ok(rest.to_vec()),
        Some((1, _)) => Err(VideoTranscodeError::Internal(TIMEOUT)),
        Some((3, _)) => Err(VideoTranscodeError::Unsupported),
        Some((4, _)) => Err(VideoTranscodeError::Internal(RELEASE_PENDING)),
        _ => Err(VideoTranscodeError::Internal(FAILED)),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::time::Duration;

    use crate::contract::{
        SOURCE_FPS, SOURCE_FRAMES, SOURCE_HEIGHT, SOURCE_WIDTH, TARGET_HEIGHT, TARGET_WIDTH,
        TIMEOUT, VideoTranscode,
    };
    use crate::error::VideoTranscodeError;
    use jni::objects::{JByteArray, JObject, JValue};

    use super::{FAILED, MAX_MP4_BYTES, decode_reply};

    pub(super) struct AndroidVideoTranscode;

    /// `VideoTranscodeHelper` の static メソッドを同期で呼ぶ。Activity 未確立・JNI 失敗は `None`（例外は消す）。
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
            jni::jni_str!("com.fandhe.platform.VideoTranscodeHelper"),
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

    impl VideoTranscode for AndroidVideoTranscode {
        fn transcode(&self, remaining: Duration) -> Result<Vec<u8>, VideoTranscodeError> {
            let ms = i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX);
            if ms == 0 {
                return Err(VideoTranscodeError::Internal(TIMEOUT));
            }
            let reply = call_helper(|env, activity, class| {
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("transcode"),
                    jni::jni_sig!((android.content.Context, int, int, int, int, int, int, long) -> byte[]),
                    &[
                        JValue::Object(activity),
                        // 寸法・fps・フレーム数は `contract` の定数が SSOT（Java へ複製しない）。
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
            .ok_or(VideoTranscodeError::Internal(FAILED))?;
            decode_reply(&reply)
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::VideoTranscode> {
    std::sync::Arc::new(imp::AndroidVideoTranscode)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::VideoTranscode> {
    std::sync::Arc::new(crate::contract::UnsupportedVideoTranscode)
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
            Err(VideoTranscodeError::Internal(TIMEOUT))
        ));
        assert!(matches!(
            decode_reply(&[3]),
            Err(VideoTranscodeError::Unsupported)
        ));
        assert!(matches!(
            decode_reply(&[4]),
            Err(VideoTranscodeError::Internal(RELEASE_PENDING))
        ));
        for bad in [&[][..], &[2][..], &[99, 1][..]] {
            assert!(matches!(
                decode_reply(bad),
                Err(VideoTranscodeError::Internal(FAILED))
            ));
        }
    }
}
