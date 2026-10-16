//! macOS / iOS 向け音声変換実装（Issue #311 macOS、iOS は b-ios の子 issue が `implementation` で返す）。
//! `device/media_transcode.rs` の macOS 経路から呼ばれ、`apple/fandhe_apple_media_transcode.m`
//! （`AVAudioFile` で AAC の m4a を書く。`build.rs` が feature `device` 有効時にコンパイル）を
//! `unsafe extern "C"` 1 関数で呼ぶ。出力は `.m` からコールバックで Rust の `Vec<u8>` へコピーし、
//! 所有権とアロケータは境界を越えない。`unsafe` はこの呼び出しとコールバックの 2 箇所に閉じる
//! （coding-rust.md の FFI 例外規定準拠）。`build.rs` のコンパイル条件・`device.rs` の mod cfg と
//! 同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::media_transcode`。出力の検証（`check_m4a`）はドライバが行う。
//! `AVAudioFile` の書き込みは中断できないため `remaining` は使わず走り切らせる。期限後に届いた
//! 結果はドライバが捨てる（契約の「守らない範囲」）。呼び出しはブロックするため、呼び出し元は
//! ワーカーで呼ぶ。

use std::ffi::c_void;
use std::time::Duration;

use fandhe_device::media_transcode::{MediaTranscode, EMPTY};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_media_transcode.m` が定義する C ABI（`build.rs` が macOS / iOS
// ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_media_transcode_run(
        pcm: *const i16,
        n: usize,
        sample_rate: f64,
        ctx: *mut c_void,
        on_output: extern "C" fn(*mut c_void, *const u8, usize),
    ) -> i32;
}

/// `.m` から同期に呼ばれ、出力を `ctx`（`&mut Vec<u8>`）へコピーする。panic しない処理だけにする
/// （unwind を FFI 越しに伝えない）。
#[allow(unsafe_code)]
extern "C" fn copy_output(ctx: *mut c_void, bytes: *const u8, len: usize) {
    if bytes.is_null() {
        return;
    }
    // SAFETY: `ctx` は `transcode` が渡した `&mut Vec<u8>` で、`.m` は呼び出しの間だけ同期に使い
    // 保持しない。`bytes` は `len` バイトの読み取り専用領域で、このコールバックの間だけ有効。
    let (out, src) = unsafe {
        (
            &mut *(ctx as *mut Vec<u8>),
            std::slice::from_raw_parts(bytes, len),
        )
    };
    out.extend_from_slice(src);
}

/// `AVAudioFile`（AAC-LC・モノラル）で変換する実装。
pub(crate) struct AppleMediaTranscode;

impl MediaTranscode for AppleMediaTranscode {
    #[allow(unsafe_code)]
    fn transcode(
        &self,
        pcm: &[i16],
        sample_rate: u32,
        _remaining: Duration,
    ) -> Result<Vec<u8>, DeviceError> {
        let mut out: Vec<u8> = Vec::new();
        // SAFETY: `pcm` は呼び出しの間生存する読み取り専用の領域で、`.m` は呼び出しの間だけ参照し
        // 保持しない。`out` への可変参照はこの呼び出しの間だけ `ctx` として渡し、`.m` は戻る前に
        // コールバックを高々 1 回呼ぶだけで保持しない。
        let code = unsafe {
            fandhe_apple_media_transcode_run(
                pcm.as_ptr(),
                pcm.len(),
                f64::from(sample_rate),
                &mut out as *mut Vec<u8> as *mut c_void,
                copy_output,
            )
        };
        match code {
            0 if out.is_empty() => Err(DeviceError::Internal(EMPTY)),
            0 => Ok(out),
            1 => Err(DeviceError::InvalidInput("media_transcode input")),
            _ => Err(DeviceError::Internal("media_transcode")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use fandhe_device::media_playback::demo_pcm;
    use fandhe_device::media_transcode::{check_m4a, input_duration, SAMPLE_RATE};

    const LONG: Duration = Duration::from_secs(30);

    fn leftovers() -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("fandhe-transcode-")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// 成功・失敗の両方の後に一時ファイルが残らないことまで、1 本の中で順に確かめる
    /// （並列テストと競合しないため）。
    #[test]
    fn transcodes_to_valid_m4a_and_leaves_no_temp_files() {
        let before = leftovers();
        let pcm = demo_pcm(SAMPLE_RATE);
        let bytes = AppleMediaTranscode
            .transcode(&pcm, SAMPLE_RATE, LONG)
            .expect("変換できる");
        check_m4a(&bytes, input_duration(pcm.len(), SAMPLE_RATE)).expect("m4a として検証できる");
        assert!(matches!(
            AppleMediaTranscode.transcode(&[], SAMPLE_RATE, LONG),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            AppleMediaTranscode.transcode(&pcm, 0, LONG),
            Err(DeviceError::InvalidInput(_))
        ));
        assert_eq!(leftovers(), before, "一時ファイルが残っている");
    }
}
