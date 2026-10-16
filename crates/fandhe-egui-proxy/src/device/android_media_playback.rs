//! Android 向け音声再生実装（Issue #270、親 #86 の b-android）。
//! `device/media_playback.rs::platform_playback` の Android 経路から egui スレッドで呼ばれ、
//! `MainActivity.mediaPlaybackLoad` / `Control` / `Snapshot` / `Shutdown` を同期 JNI で呼ぶ
//! pull 型。Java 側は `MediaPlaybackHelper`（`MediaPlayer` + `MediaDataSource`）。
//! Java から Rust への native コールバックは無い（ドライバが `Loading` / `Playing` の間
//! `snapshot` をポーリングするため、状態変化はそのまま拾える）ので、JNI 署名の照合先は Java のみ。
//!
//! # コード表（SSOT は Java `MediaPlaybackHelper`）
//!
//! - 操作の戻り: 0 = 受付 / 1 = 失敗 / 2 = WAV 不正 / 3 = 世代不一致
//! - `mediaPlaybackControl` の op: 0 = play / 1 = pause / 2 = stop
//! - snapshot: `long[4]` = {世代, phase, 位置 ms, 長さ ms}。未取得は負値。phase は
//!   0 Idle / 1 Ready / 2 Playing / 3 Paused / 4 Ended / 5 Failed(Decode) / 6 Loading /
//!   7 Interrupted / 8 Failed(Internal)。未知の値は成功に倒さず `Failed(Internal)`
//!
//! # 世代・ライフサイクル
//!
//! 世代は本モジュールが `AtomicU64` で採番する（1 から。Activity の作り直しでも戻さない）。
//! `onPause` で再生中・開始中なら中断（Interrupted。開始中は期限切れより先にドライバが見る）、作り直しでは Java 側が release し、Rust 側の
//! 保留記録は `device/media_playback.rs::reset_pending_tracking` が捨てる。
//!
//! 守る範囲: 世代の照合・失敗を成功値へ畳まないこと・資源の解放。
//! 守らない範囲: 位置表示の遅れ、音声フォーカス（他アプリの音声と混ざる）、複数プロセス間。
//! 再生内容・OS の生エラー値はログに出さない。実機確認は #266。

#[cfg(any(target_os = "android", test))]
use fandhe_device::media_playback::{
    ms_from_i64, PlaybackFailure, PlaybackPhase, PlaybackSnapshot,
};
#[cfg(any(target_os = "android", test))]
use fandhe_device::DeviceError;

/// `load` に渡せる WAV の上限（Java 側と同じ。異常に大きい入力を FFI に渡さない）。
#[cfg(any(target_os = "android", test))]
const MAX_WAV_BYTES: usize = 16 * 1024 * 1024;

/// 操作の戻りコードを契約へ写す。0 以外は固定の種別へ畳む。
#[cfg(any(target_os = "android", test))]
fn result_from_code(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        2 => Err(DeviceError::InvalidInput("media_playback wav")),
        3 => Err(DeviceError::InvalidInput("media_playback generation")),
        _ => Err(DeviceError::Internal("media_playback")),
    }
}

/// phase コードを契約の状態へ写す。未知の値は `Failed(Internal)`。
#[cfg(any(target_os = "android", test))]
fn phase_from_code(code: i64) -> PlaybackPhase {
    match code {
        0 => PlaybackPhase::Idle,
        1 => PlaybackPhase::Ready,
        2 => PlaybackPhase::Playing,
        3 => PlaybackPhase::Paused,
        4 => PlaybackPhase::Ended,
        5 => PlaybackPhase::Failed(PlaybackFailure::Decode),
        6 => PlaybackPhase::Loading,
        7 => PlaybackPhase::Interrupted,
        _ => PlaybackPhase::Failed(PlaybackFailure::Internal),
    }
}

/// `long[4]` を契約のスナップショットへ。形が違う・世代が負なら世代 0 の `Failed(Internal)`
/// （世代 0 はドライバが照合で捨てるので成功には化けない）。
#[cfg(any(target_os = "android", test))]
fn decode_snapshot(v: &[i64]) -> PlaybackSnapshot {
    let bad = PlaybackSnapshot {
        generation: 0,
        phase: PlaybackPhase::Failed(PlaybackFailure::Internal),
        position_ms: None,
        duration_ms: None,
    };
    let [generation, phase, pos, dur] = *v else {
        return bad;
    };
    let Ok(generation) = u64::try_from(generation) else {
        return bad;
    };
    PlaybackSnapshot {
        generation,
        phase: phase_from_code(phase),
        position_ms: ms_from_i64(pos),
        duration_ms: ms_from_i64(dur),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use fandhe_device::media_playback::{
        MediaPlayback, PlaybackFailure, PlaybackPhase, PlaybackSnapshot,
    };
    use fandhe_device::DeviceError;
    use jni::objects::{JLongArray, JObject, JValue};

    use super::{decode_snapshot, result_from_code, MAX_WAV_BYTES};

    /// `load` ごとに進める世代（0 は「player なし」を表すため 1 から）。
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    pub(super) struct AndroidMediaPlayback;

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

    fn control(generation: u64, op: i32) -> Result<(), DeviceError> {
        let code = call_activity(|env, a| {
            Ok(env
                .call_method(
                    a,
                    jni::jni_str!("mediaPlaybackControl"),
                    jni::jni_sig!((long, int) -> int),
                    &[JValue::Long(generation as jni::sys::jlong), JValue::Int(op)],
                )?
                .i()?)
        })
        .ok_or(DeviceError::Internal("media_playback"))?;
        result_from_code(code)
    }

    impl MediaPlayback for AndroidMediaPlayback {
        fn load(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError> {
            if wav.is_empty() || wav.len() > MAX_WAV_BYTES {
                // load 契約（旧プレーヤーの解放・旧世代の無効化）を失敗経路でも守る。
                self.shutdown();
                return Err(DeviceError::InvalidInput("media_playback wav"));
            }
            let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            let code = call_activity(|env, a| {
                let j_wav = env.byte_array_from_slice(&wav)?;
                Ok(env
                    .call_method(
                        a,
                        jni::jni_str!("mediaPlaybackLoad"),
                        jni::jni_sig!((long, byte[]) -> int),
                        &[
                            JValue::Long(generation as jni::sys::jlong),
                            JValue::Object(&j_wav),
                        ],
                    )?
                    .i()?)
            })
            .ok_or(DeviceError::Internal("media_playback"))?;
            result_from_code(code).map(|()| generation)
        }

        fn play(&self, generation: u64) -> Result<(), DeviceError> {
            control(generation, 0)
        }

        fn pause(&self, generation: u64) -> Result<(), DeviceError> {
            control(generation, 1)
        }

        fn stop(&self, generation: u64) -> Result<(), DeviceError> {
            control(generation, 2)
        }

        fn snapshot(&self) -> PlaybackSnapshot {
            let values = call_activity(|env, a| {
                let ret = env.call_method(
                    a,
                    jni::jni_str!("mediaPlaybackSnapshot"),
                    jni::jni_sig!(() -> long[]),
                    &[],
                )?;
                let arr = env.cast_local::<JLongArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(None);
                }
                // 長さを検証してから読む。
                let n = arr.len(env)?;
                if n != 4 {
                    return Ok(None);
                }
                let mut v = [0i64; 4];
                arr.get_region(env, 0, &mut v)?;
                Ok(Some(v))
            })
            .flatten();
            match values {
                Some(v) => decode_snapshot(&v),
                None => PlaybackSnapshot {
                    generation: 0,
                    phase: PlaybackPhase::Failed(PlaybackFailure::Internal),
                    position_ms: None,
                    duration_ms: None,
                },
            }
        }

        fn shutdown(&self) {
            let _ = call_activity(|env, a| {
                env.call_method(
                    a,
                    jni::jni_str!("mediaPlaybackShutdown"),
                    jni::jni_sig!(() -> ()),
                    &[],
                )?;
                Ok(())
            });
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::media_playback::MediaPlayback> {
    std::sync::Arc::new(imp::AndroidMediaPlayback)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::media_playback::MediaPlayback> {
    std::sync::Arc::new(fandhe_device::media_playback::UnsupportedMediaPlayback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_to_contract_errors() {
        assert!(result_from_code(0).is_ok());
        assert!(matches!(
            result_from_code(2),
            Err(DeviceError::InvalidInput("media_playback wav"))
        ));
        assert!(matches!(
            result_from_code(3),
            Err(DeviceError::InvalidInput("media_playback generation"))
        ));
        for c in [1, 99, -1] {
            assert!(matches!(result_from_code(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn phase_codes() {
        use PlaybackPhase::*;
        let want = [
            Idle,
            Ready,
            Playing,
            Paused,
            Ended,
            Failed(PlaybackFailure::Decode),
            Loading,
            Interrupted,
            Failed(PlaybackFailure::Internal),
        ];
        for (i, p) in want.into_iter().enumerate() {
            assert_eq!(phase_from_code(i as i64), p);
        }
        for c in [9, 42, -1] {
            assert_eq!(phase_from_code(c), Failed(PlaybackFailure::Internal));
        }
    }

    #[test]
    fn snapshot_decoding_never_fakes_success() {
        let s = decode_snapshot(&[3, 2, 1500, 5000]);
        assert_eq!(s.generation, 3);
        assert_eq!(s.phase, PlaybackPhase::Playing);
        assert_eq!((s.position_ms, s.duration_ms), (Some(1500), Some(5000)));
        let s = decode_snapshot(&[3, 6, -1, -1]);
        assert_eq!((s.position_ms, s.duration_ms), (None, None));
        for bad in [
            &[][..],
            &[1, 2, 3][..],
            &[1, 2, 3, 4, 5][..],
            &[-1, 2, 0, 0][..],
        ] {
            let s = decode_snapshot(bad);
            assert_eq!(s.generation, 0);
            assert_eq!(s.phase, PlaybackPhase::Failed(PlaybackFailure::Internal));
        }
    }
}
