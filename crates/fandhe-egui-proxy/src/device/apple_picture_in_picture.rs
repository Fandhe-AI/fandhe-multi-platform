//! macOS・iOS 共用のピクチャ・イン・ピクチャ実装（Issue #364 macOS、#365 iOS）。
//! `device/picture_in_picture.rs::platform_pip` の macOS・iOS 経路から呼ばれる。
//! `apple/fandhe_apple_picture_in_picture.m`（AVKit の sample buffer 経路、`build.rs` の macOS・iOS 分岐が
//! コンパイル）を C ABI で呼ぶ薄いラッパーで、世代の採番だけを Rust 側で持つ。色の表・間隔・寸法は
//! `fandhe_device::picture_in_picture` の定数を FFI の引数で渡す（SSOT を 1 か所にする）。
//! 戻りコードを契約上のエラーへ写す純関数は [`result_from_code`]、状態コードは [`phase_from_code`]。
//! `unsafe` は extern 呼び出しに閉じ、映像の内容はログに出さない。
//! 呼び出しはすべて egui の update（macOS・iOS とも main thread）から来る前提で、`.m` は main thread 以外を拒否する。

use std::sync::Arc;

use fandhe_device::picture_in_picture::PictureInPicture;

/// 現在の OS 向け実装を返す。
pub(crate) fn implementation() -> Arc<dyn PictureInPicture> {
    Arc::new(ApplePictureInPicture)
}

struct ApplePictureInPicture;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};

    use fandhe_device::picture_in_picture::{
        PictureInPicture, PipFailure, PipPhase, PipSnapshot, FRAME_COLORS, FRAME_HEIGHT,
        FRAME_INTERVAL, FRAME_WIDTH,
    };
    use fandhe_device::DeviceError;

    /// `prepare` ごとに進める世代（0 は「資源なし」を表すため 1 から）。
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    // SAFETY: `apple/fandhe_apple_picture_in_picture.m` が定義する C ABI（`build.rs` がコンパイルし
    // AVKit 等をリンク）。契約は `.m` 冒頭コメントに集約する。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_apple_pip_prepare(
            generation: u64,
            rgb: *const u8,
            color_count: usize,
            interval_ms: u32,
            width: u32,
            height: u32,
        ) -> i32;
        fn fandhe_apple_pip_start(generation: u64) -> i32;
        fn fandhe_apple_pip_stop(generation: u64) -> i32;
        fn fandhe_apple_pip_snapshot(generation: *mut u64, phase: *mut i32);
        fn fandhe_apple_pip_shutdown();
    }

    /// `.m` の戻りコードを契約へ写す純関数。OS の生値は見ず固定の種別へ畳む。
    pub(super) fn result_from_code(code: i32) -> Result<(), DeviceError> {
        match code {
            0 => Ok(()),
            1 => Err(DeviceError::Unsupported("picture_in_picture")),
            2 => Err(DeviceError::InvalidInput("picture_in_picture")),
            5 => Err(DeviceError::Disabled("picture_in_picture")),
            6 => Err(DeviceError::InvalidInput("picture_in_picture generation")),
            _ => Err(DeviceError::Internal("picture_in_picture")),
        }
    }

    /// `.m` の phase コードを契約の状態へ写す純関数。未知のコードは成功にせず `Internal` 失敗。
    pub(super) fn phase_from_code(code: i32) -> PipPhase {
        match code {
            0 => PipPhase::Idle,
            1 => PipPhase::Preparing,
            2 => PipPhase::Ready,
            3 => PipPhase::Starting,
            4 => PipPhase::Active,
            5 => PipPhase::Stopping,
            6 => PipPhase::Exited,
            7 => PipPhase::Failed(PipFailure::NotPossible),
            _ => PipPhase::Failed(PipFailure::Internal),
        }
    }

    impl PictureInPicture for super::ApplePictureInPicture {
        #[allow(unsafe_code)]
        fn prepare(&self) -> Result<u64, DeviceError> {
            let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            let rgb: Vec<u8> = FRAME_COLORS.iter().flatten().copied().collect();
            let interval_ms = FRAME_INTERVAL.as_millis() as u32;
            // SAFETY: `rgb` はこの呼び出しの間生きており、`.m` は同期でコピーしてポインタを保持しない。
            // 長さは `FRAME_COLORS.len() * 3` で `color_count` と一致する。
            let code = unsafe {
                fandhe_apple_pip_prepare(
                    generation,
                    rgb.as_ptr(),
                    FRAME_COLORS.len(),
                    interval_ms,
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                )
            };
            result_from_code(code).map(|()| generation)
        }

        #[allow(unsafe_code)]
        fn start(&self, generation: u64) -> Result<(), DeviceError> {
            // SAFETY: 値渡しの引数のみ。`.m` は 1 本のミューテックスの内側で処理する。
            result_from_code(unsafe { fandhe_apple_pip_start(generation) })
        }

        #[allow(unsafe_code)]
        fn stop(&self, generation: u64) -> Result<(), DeviceError> {
            // SAFETY: 同上。
            result_from_code(unsafe { fandhe_apple_pip_stop(generation) })
        }

        #[allow(unsafe_code)]
        fn snapshot(&self) -> PipSnapshot {
            let (mut generation, mut phase) = (0u64, 0i32);
            // SAFETY: 2 つの出力先はこの呼び出し中のみ有効なスタック上の値で、`.m` は必ず 1 回ずつ書く。
            unsafe { fandhe_apple_pip_snapshot(&mut generation, &mut phase) };
            PipSnapshot {
                generation,
                phase: phase_from_code(phase),
            }
        }

        #[allow(unsafe_code)]
        fn shutdown(&self) {
            // SAFETY: 引数なし。`.m` 側は冪等で、タイマー・controller・layer を解放する。
            unsafe { fandhe_apple_pip_shutdown() };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn codes_map_to_contract_errors() {
            assert!(result_from_code(0).is_ok());
            assert!(matches!(
                result_from_code(1),
                Err(DeviceError::Unsupported("picture_in_picture"))
            ));
            assert!(matches!(
                result_from_code(2),
                Err(DeviceError::InvalidInput("picture_in_picture"))
            ));
            assert!(matches!(
                result_from_code(5),
                Err(DeviceError::Disabled("picture_in_picture"))
            ));
            assert!(matches!(
                result_from_code(6),
                Err(DeviceError::InvalidInput("picture_in_picture generation"))
            ));
            for c in [3, 4, 99] {
                assert!(matches!(result_from_code(c), Err(DeviceError::Internal(_))));
            }
        }

        #[test]
        fn phase_codes_map_and_unknown_fails() {
            assert_eq!(phase_from_code(2), PipPhase::Ready);
            assert_eq!(phase_from_code(4), PipPhase::Active);
            assert_eq!(phase_from_code(6), PipPhase::Exited);
            assert_eq!(
                phase_from_code(7),
                PipPhase::Failed(PipFailure::NotPossible)
            );
            assert_eq!(phase_from_code(8), PipPhase::Failed(PipFailure::Internal));
            assert_eq!(phase_from_code(42), PipPhase::Failed(PipFailure::Internal));
        }
    }
}
