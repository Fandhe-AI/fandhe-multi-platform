//! macOS / iOS 向け動画変換実装（Issue #1014 macOS、iOS は #1016 で `ios_video_transcode` 経由）。
//! `device/video_transcode.rs` の macOS 経路から呼ばれ、`apple/fandhe_apple_video_transcode.m`
//! （`AVAssetWriter` で元動画を書き、`AVAssetExportSession` で小さい MP4 へ書き出す。`build.rs` が
//! feature `device` 有効時にコンパイル）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` は extern 宣言・
//! 呼び出し・コールバックに閉じる（coding-rust.md の FFI 例外規定準拠）。`build.rs` のコンパイル条件・
//! `device.rs` の mod cfg と同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::video_transcode`。出力の検証（`check_mp4_video`）はドライバが行う。
//! 一時ファイル（元動画・出力）は本モジュールが `video_transcode::temp_pair` で作り、全経路で消す。
//! 消せなかったものは `LEFTOVERS` に記録し、次の要求の前に再削除する（契約表 9）。
//! OS 操作が取り消しても終わらなかった場合（`.m` の戻りコード 6）は、完了を確認できるまで
//! OS を呼ばない（`settled`）。呼び出しはブロックするため、呼び出し元はワーカーで呼ぶ。

use std::ffi::{c_char, c_void, CString};
use std::path::Path;
use std::sync::{Mutex, TryLockError};
use std::time::Duration;

use fandhe_device::video_transcode::{
    fill_source_frame_bgra, VideoTranscode, RELEASE_PENDING, SOURCE_FPS, SOURCE_FRAMES,
    SOURCE_HEIGHT, SOURCE_WIDTH, TIMEOUT, UNREADABLE,
};
use fandhe_device::DeviceError;

use super::video_transcode::{temp_pair, LEFTOVERS};

type FillFrame = extern "C" fn(*mut c_void, u32, *mut u8, usize, usize) -> bool;

// SAFETY: `apple/fandhe_apple_video_transcode.m` が定義する C ABI（`build.rs` が macOS / iOS
// ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_video_transcode_settled() -> bool;
    fn fandhe_apple_video_transcode_run(
        src_path: *const c_char,
        out_path: *const c_char,
        width: u32,
        height: u32,
        fps: u32,
        frames: u32,
        remaining_ns: u64,
        ctx: *mut c_void,
        fill_frame: FillFrame,
    ) -> i32;
}

/// 元動画の寸法（コールバックの `ctx`）。
struct FillCtx {
    width: u32,
    height: u32,
}

/// `.m` から同期に呼ばれ、`base` へ `index` 枚目の絵を描く。panic しない処理だけにする
/// （unwind を FFI 越しに伝えない）。null・長さ不足は `false`。
#[allow(unsafe_code)]
extern "C" fn fill_frame(
    ctx: *mut c_void,
    index: u32,
    base: *mut u8,
    bytes_per_row: usize,
    len: usize,
) -> bool {
    if ctx.is_null() || base.is_null() {
        return false;
    }
    // SAFETY: `ctx` は `transcode` が渡した `&FillCtx` で、`.m` は呼び出しの間だけ同期に使い保持しない。
    let dims = unsafe { &*(ctx as *const FillCtx) };
    // SAFETY: `base` は `len` バイトの書き込み可能な領域（ロック中の `CVPixelBuffer`）で、
    // このコールバックの間だけ有効。`fill_source_frame_bgra` は範囲を検査して書く。
    let dst = unsafe { std::slice::from_raw_parts_mut(base, len) };
    fill_source_frame_bgra(index, dims.width, dims.height, bytes_per_row, dst)
}

/// 同時に走る呼び出しは 1 本だけ（前の呼び出しが猶予待ちで残っている間は新しく始めない）。
static CALL: Mutex<()> = Mutex::new(());

/// `AVAssetWriter` + `AVAssetExportSession`（`AVAssetExportPreset640x480`）で変換する実装。
pub(crate) struct AppleVideoTranscode;

impl VideoTranscode for AppleVideoTranscode {
    #[allow(unsafe_code)]
    fn transcode(&self, remaining: Duration) -> Result<Vec<u8>, DeviceError> {
        let _guard = match CALL.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(DeviceError::Internal(RELEASE_PENDING)),
        };
        // SAFETY: 引数なしの純粋な状態読み出し。
        if !unsafe { fandhe_apple_video_transcode_settled() } {
            return Err(DeviceError::Internal(RELEASE_PENDING));
        }
        LEFTOVERS.ensure_released()?;
        let (src, out) = temp_pair();
        let to_c = |p: &Path| {
            p.to_str()
                .and_then(|s| CString::new(s).ok())
                .ok_or(DeviceError::Internal("video_transcode path"))
        };
        let (c_src, c_out) = (to_c(&src)?, to_c(&out)?);
        let ctx = FillCtx {
            width: SOURCE_WIDTH,
            height: SOURCE_HEIGHT,
        };
        let remaining_ns = u64::try_from(remaining.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: パス文字列と `ctx` は呼び出しの間生存し、`.m` は呼び出しの間だけ参照して保持しない。
        // `fill_frame` は panic しない `extern "C"` 関数。
        let code = unsafe {
            fandhe_apple_video_transcode_run(
                c_src.as_ptr(),
                c_out.as_ptr(),
                SOURCE_WIDTH,
                SOURCE_HEIGHT,
                SOURCE_FPS,
                SOURCE_FRAMES,
                remaining_ns,
                &ctx as *const FillCtx as *mut c_void,
                fill_frame,
            )
        };
        let result = match code {
            0 => std::fs::read(&out).map_err(|_| DeviceError::Internal(UNREADABLE)),
            1 => Err(DeviceError::InvalidInput("video_transcode input")),
            2 => Err(DeviceError::Internal("video_transcode source")),
            3 => Err(DeviceError::Internal("video_transcode export")),
            4 => Err(DeviceError::Unsupported("video_transcode")),
            5 | 6 => Err(DeviceError::Internal(TIMEOUT)),
            7 => Err(DeviceError::Internal(RELEASE_PENDING)),
            _ => Err(DeviceError::Internal("video_transcode")),
        };
        if code == 6 {
            // OS の操作が残っている可能性がある間は、消さずに未解放として記録する
            // （次の要求の前に `settled` と再削除で確かめる）。
            LEFTOVERS.mark_unreleased(&src);
            LEFTOVERS.mark_unreleased(&out);
        } else {
            LEFTOVERS.release(&[&src, &out]);
        }
        result
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use fandhe_device::video_transcode::{check_mp4_video, SOURCE_HEIGHT, SOURCE_WIDTH};

    fn leftovers() -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("fandhe-video-transcode-")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// 実変換・期限切れ・復帰まで、1 本の中で順に確かめる（並列テストと競合しないため）。
    #[test]
    #[allow(unsafe_code)]
    fn transcodes_times_out_and_leaves_no_temp_files() {
        let before = leftovers();
        let bytes = AppleVideoTranscode
            .transcode(Duration::from_secs(60))
            .expect("変換できる");
        let info = check_mp4_video(&bytes).expect("MP4 として検証できる");
        assert!(info.width < SOURCE_WIDTH && info.height < SOURCE_HEIGHT);
        assert_eq!(leftovers(), before, "成功後に一時ファイルが残っている");

        // 極小の期限は TIMEOUT。OS の操作が残る場合は settled になるまで待つ。
        let r = AppleVideoTranscode.transcode(Duration::from_millis(1));
        assert!(
            matches!(r, Err(DeviceError::Internal(TIMEOUT))),
            "{:?}",
            r.map(|b| b.len())
        );
        let started = std::time::Instant::now();
        // SAFETY: 引数なしの純粋な状態読み出し。
        while !unsafe { fandhe_apple_video_transcode_settled() }
            && started.elapsed() < Duration::from_secs(30)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        LEFTOVERS
            .ensure_released()
            .expect("settled 後は再削除できる");
        assert_eq!(leftovers(), before, "期限切れ後に一時ファイルが残っている");

        // 復帰: 次の要求が成功する。
        let again = AppleVideoTranscode
            .transcode(Duration::from_secs(60))
            .expect("復帰後も変換できる");
        check_mp4_video(&again).expect("MP4 として検証できる");
        assert_eq!(leftovers(), before);
    }
}
