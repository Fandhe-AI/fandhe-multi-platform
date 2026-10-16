//! macOS 向け音声再生実装（Issue #267。iOS は親 #86 の b-ios が同じ `.m` を使う）。
//! `device/media_playback.rs::platform_playback` の macOS 経路から呼ばれる。
//! `apple/fandhe_apple_media_playback.m`（`AVAudioPlayer`、`build.rs` の macOS / iOS 分岐がコンパイル）を
//! C ABI で呼ぶ薄いラッパーで、世代の採番だけを Rust 側で持つ。戻りコードを契約上のエラーへ写す
//! 純関数は [`result_from_code`]。`unsafe` は extern 呼び出しに閉じ、再生内容はログに出さない。

// iOS からはまだ呼ばれない（親 #86 の b-ios が `ios_media_playback` から使うときに外す）。
#![cfg_attr(target_os = "ios", allow(dead_code))]

use std::sync::Arc;

use fandhe_device::media_playback::MediaPlayback;

/// 現在の OS 向け実装を返す。
pub(crate) fn implementation() -> Arc<dyn MediaPlayback> {
    Arc::new(AppleMediaPlayback)
}

struct AppleMediaPlayback;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use fandhe_device::media_playback::{
        secs_to_ms, MediaPlayback, PlaybackFailure, PlaybackPhase, PlaybackSnapshot,
    };
    use fandhe_device::DeviceError;

    /// `load` に渡せる WAV の上限（異常に大きい入力を FFI に渡さない）。
    const MAX_WAV_BYTES: usize = 16 * 1024 * 1024;

    /// `load` ごとに進める世代（0 は「player なし」を表すため 1 から）。
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    // SAFETY: `apple/fandhe_apple_media_playback.m` が定義する C ABI（`build.rs` がコンパイル、
    // AVFoundation は既存のリンク）。契約は `.m` 冒頭コメントに集約する。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_apple_media_playback_load(bytes: *const u8, len: usize, generation: u64) -> i32;
        fn fandhe_apple_media_playback_play(generation: u64) -> i32;
        fn fandhe_apple_media_playback_pause(generation: u64) -> i32;
        fn fandhe_apple_media_playback_stop(generation: u64) -> i32;
        fn fandhe_apple_media_playback_snapshot(
            generation: *mut u64,
            phase: *mut i32,
            pos: *mut f64,
            dur: *mut f64,
        );
        fn fandhe_apple_media_playback_shutdown();
    }

    /// `.m` の戻りコードを契約へ写す純関数。0 以外は OS の生値を見ずに固定の種別へ畳む。
    pub(super) fn result_from_code(code: i32) -> Result<(), DeviceError> {
        match code {
            0 => Ok(()),
            2 => Err(DeviceError::InvalidInput("media_playback wav")),
            3 => Err(DeviceError::InvalidInput("media_playback generation")),
            _ => Err(DeviceError::Internal("media_playback")),
        }
    }

    /// `.m` の phase コードを契約の状態へ写す純関数。未知のコードは成功にせず `Internal` 失敗。
    pub(super) fn phase_from_code(code: i32) -> PlaybackPhase {
        match code {
            0 => PlaybackPhase::Idle,
            1 => PlaybackPhase::Ready,
            2 => PlaybackPhase::Playing,
            3 => PlaybackPhase::Paused,
            4 => PlaybackPhase::Ended,
            5 => PlaybackPhase::Failed(PlaybackFailure::Decode),
            _ => PlaybackPhase::Failed(PlaybackFailure::Internal),
        }
    }

    impl MediaPlayback for super::AppleMediaPlayback {
        #[allow(unsafe_code)]
        fn load(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError> {
            if wav.is_empty() || wav.len() > MAX_WAV_BYTES {
                return Err(DeviceError::InvalidInput("media_playback wav"));
            }
            let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            // SAFETY: `wav` はこの呼び出しの間生きており、`.m` は同期で NSData へコピーして
            // ポインタを保持しない。長さは上で 1..=MAX_WAV_BYTES に検証済み。
            let code =
                unsafe { fandhe_apple_media_playback_load(wav.as_ptr(), wav.len(), generation) };
            result_from_code(code).map(|()| generation)
        }

        #[allow(unsafe_code)]
        fn play(&self, generation: u64) -> Result<(), DeviceError> {
            // SAFETY: 値渡しの引数のみ。`.m` は 1 本のミューテックスの内側で処理する。
            result_from_code(unsafe { fandhe_apple_media_playback_play(generation) })
        }

        #[allow(unsafe_code)]
        fn pause(&self, generation: u64) -> Result<(), DeviceError> {
            // SAFETY: 同上。
            result_from_code(unsafe { fandhe_apple_media_playback_pause(generation) })
        }

        #[allow(unsafe_code)]
        fn stop(&self, generation: u64) -> Result<(), DeviceError> {
            // SAFETY: 同上。
            result_from_code(unsafe { fandhe_apple_media_playback_stop(generation) })
        }

        #[allow(unsafe_code)]
        fn snapshot(&self) -> PlaybackSnapshot {
            let (mut generation, mut phase, mut pos, mut dur) = (0u64, 0i32, -1.0f64, -1.0f64);
            // SAFETY: 4 つの出力先はこの呼び出し中のみ有効なスタック上の値で、`.m` は必ず 1 回ずつ書く。
            unsafe {
                fandhe_apple_media_playback_snapshot(
                    &mut generation,
                    &mut phase,
                    &mut pos,
                    &mut dur,
                )
            };
            PlaybackSnapshot {
                generation,
                phase: phase_from_code(phase),
                position_ms: secs_to_ms(pos),
                duration_ms: secs_to_ms(dur),
            }
        }

        #[allow(unsafe_code)]
        fn shutdown(&self) {
            // SAFETY: 引数なし。`.m` 側は冪等で、旧 player を止めて解放する。
            unsafe { fandhe_apple_media_playback_shutdown() };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fandhe_device::media_playback::demo_wav;

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
            assert!(matches!(result_from_code(1), Err(DeviceError::Internal(_))));
            assert!(matches!(
                result_from_code(99),
                Err(DeviceError::Internal(_))
            ));
            assert_eq!(
                phase_from_code(5),
                PlaybackPhase::Failed(PlaybackFailure::Decode)
            );
            assert_eq!(
                phase_from_code(42),
                PlaybackPhase::Failed(PlaybackFailure::Internal)
            );
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn load_rejects_invalid_and_stale_generation() {
            let p = super::super::AppleMediaPlayback;
            assert!(matches!(
                p.load(Arc::from(Vec::<u8>::new())),
                Err(DeviceError::InvalidInput("media_playback wav"))
            ));
            // デコードできないバイト列は成功にならない。
            assert!(p.load(Arc::from(vec![1u8, 2, 3, 4])).is_err());
            let g = p.load(demo_wav()).expect("load");
            assert!(p.play(g + 100).is_err(), "古い・別世代は拒否");
            assert!(p.pause(g).is_ok() && p.stop(g).is_ok());
            assert_eq!(p.snapshot().phase, PlaybackPhase::Ready);
            assert!(p.stop(g).is_ok());
            p.shutdown();
            assert!(p.stop(g).is_err(), "解放後の世代は拒否");
        }
    }
}
