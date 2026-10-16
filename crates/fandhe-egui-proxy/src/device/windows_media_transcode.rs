//! Windows 向け動画・音声の変換（Issue #312）。`Windows.Media.Transcoding.MediaTranscoder` で
//! 実行時に作った PCM を AAC-LC の m4a へ 1 回変換する。
//!
//! 呼び出し元: `device/media_transcode.rs` の Windows 経路がワーカースレッドから `transcode` を
//! 同期に呼ぶ。期限・世代の照合、期限後に届いた結果の破棄、出力の読み直し（`check_m4a`）、
//! 画面向けの失敗の写像は呼び出し側が担い、本ファイルは m4a のバイト列か `DeviceError` だけを返す。
//! 契約の SSOT は `fandhe_device::media_transcode`。
//!
//! 入力が一時ファイルになる理由: `PrepareStreamTranscodeAsync` はメモリ上のストリームを受けない
//! （<https://learn.microsoft.com/en-us/uwp/api/windows.media.transcoding.mediatranscoder.preparestreamtranscodeasync>）。
//! WAV を `temp_dir` に書き、`FileRandomAccessStream` で開いて渡す。出力はメモリ上のストリーム。
//! 一時ファイルとストリームは成功・失敗・期限切れのどの経路でも閉じて消す（契約表 9）。
//!
//! 期限は `remaining` から求めた `Instant`（単調時計）。OS 側の操作も期限で `Cancel()` する。
//!
//! 失敗の写像（全経路で一貫）:
//! - クラスなし（`REGDB_E_CLASSNOTREG`）・statics なし（`E_NOINTERFACE`、1709 未満）・
//!   `CodecNotFound`（Media Feature Pack のない N エディション等）→ `Unsupported`
//! - 期限切れ → `Internal(TIMEOUT)`、出力 0 バイト → `Internal(EMPTY)`
//! - それ以外 → 段階別の固定ラベルの `Internal`（HRESULT・OS のエラー文は出さない）
//!
//! `RoInitialize` は呼ばず（MTA フォールバック）、`unsafe` は使わない。
//!
//! 既知の天井: unpackaged の exe で動くかは未確認（#310）。出力が fragmented MP4 になる環境では
//! 呼び出し側の `check_m4a` が `UNREADABLE` で落とす（嘘の成功にはならない）。
//! 型検査は `x86_64-pc-windows-gnu` のみ。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use fandhe_device::media_playback::wav_bytes;
use fandhe_device::media_transcode::{validate_input, MediaTranscode, EMPTY, TIMEOUT};
use fandhe_device::DeviceError;
use windows::core::HSTRING;
use windows::Media::MediaProperties::{AudioEncodingQuality, MediaEncodingProfile};
use windows::Media::Transcoding::{MediaTranscoder, TranscodeFailureReason};
use windows::Storage::FileAccessMode;
use windows::Storage::Streams::{
    DataReader, FileRandomAccessStream, IRandomAccessStream, InMemoryRandomAccessStream,
};
use windows::Win32::Foundation::{E_NOINTERFACE, REGDB_E_CLASSNOTREG};
use windows_future::{
    AsyncActionWithProgressCompletedHandler, AsyncOperationCompletedHandler, AsyncStatus,
    IAsyncOperation,
};

/// 出力の上限。5 秒の AAC は 100 KB 前後。`u32` への切り詰めと過大な確保を防ぐ。
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

/// Windows の動画・音声変換（MediaTranscoder）。
pub(crate) struct WindowsMediaTranscode;

fn internal(label: &'static str) -> DeviceError {
    DeviceError::Internal(label)
}

/// クラスなし・statics なしは非対応、それ以外は段階別ラベルの `Internal`。
fn map_err(e: &windows::core::Error, label: &'static str) -> DeviceError {
    if e.code() == REGDB_E_CLASSNOTREG || e.code() == E_NOINTERFACE {
        DeviceError::Unsupported("media_transcode")
    } else {
        internal(label)
    }
}

/// 入力用の一時 WAV。Drop で必ず消す（全経路）。
struct TempWav(PathBuf);

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 一時ファイルの名前に使う連番（同一プロセス内の衝突を防ぐ）。
static SEQ: AtomicU64 = AtomicU64::new(0);

/// `deadline` まで完了を待つ。期限切れ・登録失敗は `cancel()` してから返す。
fn wait(
    deadline: Instant,
    label: &'static str,
    register: impl FnOnce(mpsc::Sender<AsyncStatus>) -> windows::core::Result<()>,
    cancel: impl Fn(),
) -> Result<(), DeviceError> {
    let (tx, rx) = mpsc::channel();
    if register(tx).is_err() {
        cancel();
        return Err(internal(label));
    }
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(AsyncStatus::Completed) => Ok(()),
        Ok(_) => Err(internal(label)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            cancel();
            Err(internal(TIMEOUT))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(internal(label)),
    }
}

/// `IAsyncOperation` を期限つきで待って結果を取る。
fn wait_op<T: windows::core::RuntimeType + 'static>(
    deadline: Instant,
    label: &'static str,
    op: &IAsyncOperation<T>,
) -> Result<T, DeviceError> {
    wait(
        deadline,
        label,
        |tx| {
            op.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, status| {
                let _ = tx.send(status);
                Ok(())
            }))
        },
        || {
            let _ = op.Cancel();
        },
    )?;
    op.GetResults().map_err(|e| map_err(&e, label))
}

impl MediaTranscode for WindowsMediaTranscode {
    fn transcode(
        &self,
        pcm: &[i16],
        sample_rate: u32,
        remaining: Duration,
    ) -> Result<Vec<u8>, DeviceError> {
        let deadline = Instant::now() + remaining;
        validate_input(pcm, sample_rate)?;

        // `create_new` で既存のファイルやリンクを上書きしない。名前は外部入力を含まない。
        let path = std::env::temp_dir().join(format!(
            "fandhe-media-transcode-{}-{}.wav",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let wrote = {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut f| f.write_all(&wav_bytes(pcm, sample_rate)))
        };
        let _guard = TempWav(path.clone());
        wrote.map_err(|_| internal("media_transcode input"))?;

        let open = FileRandomAccessStream::OpenAsync(
            &HSTRING::from(path.as_os_str()),
            FileAccessMode::Read,
        )
        .map_err(|e| map_err(&e, "media_transcode input"))?;
        let input = wait_op(deadline, "media_transcode input", &open)?;
        // 入力を閉じてからでないと Windows ではファイルを消せない（ガードの Drop より先）。
        let result = run(&input, deadline);
        let _ = input.Close();
        result
    }
}

/// 準備 → 変換 → 読み出し。出力ストリームは戻る前に閉じる。
fn run(input: &IRandomAccessStream, deadline: Instant) -> Result<Vec<u8>, DeviceError> {
    let transcoder = MediaTranscoder::new().map_err(|e| map_err(&e, "media_transcode prepare"))?;
    let profile = MediaEncodingProfile::CreateM4a(AudioEncodingQuality::Auto)
        .map_err(|_| internal("media_transcode prepare"))?;
    let output =
        InMemoryRandomAccessStream::new().map_err(|_| internal("media_transcode prepare"))?;
    let result = transcode_into(&transcoder, input, &output, &profile, deadline)
        .and_then(|()| read_all(&output));
    let _ = output.Close();
    result
}

fn transcode_into(
    transcoder: &MediaTranscoder,
    input: &IRandomAccessStream,
    output: &InMemoryRandomAccessStream,
    profile: &MediaEncodingProfile,
    deadline: Instant,
) -> Result<(), DeviceError> {
    let prep = transcoder
        .PrepareStreamTranscodeAsync(input, output, profile)
        .map_err(|e| map_err(&e, "media_transcode prepare"))?;
    let prepared = wait_op(deadline, "media_transcode prepare", &prep)?;
    if !prepared.CanTranscode().unwrap_or(false) {
        return Err(match prepared.FailureReason() {
            Ok(TranscodeFailureReason::CodecNotFound) => {
                DeviceError::Unsupported("media_transcode")
            }
            _ => internal("media_transcode prepare"),
        });
    }
    let action = prepared
        .TranscodeAsync()
        .map_err(|e| map_err(&e, "media_transcode transcode"))?;
    wait(
        deadline,
        "media_transcode transcode",
        |tx| {
            action.SetCompleted(&AsyncActionWithProgressCompletedHandler::new(
                move |_, status| {
                    let _ = tx.send(status);
                    Ok(())
                },
            ))
        },
        || {
            let _ = action.Cancel();
        },
    )
}

/// 出力ストリームの全バイトを読む。0 バイト・上限超えは失敗。
fn read_all(output: &InMemoryRandomAccessStream) -> Result<Vec<u8>, DeviceError> {
    let size = output
        .Size()
        .map_err(|_| internal("media_transcode size"))?;
    if size == 0 {
        return Err(internal(EMPTY));
    }
    if size > MAX_OUTPUT_BYTES {
        return Err(internal("media_transcode size"));
    }
    let e = |_| internal("media_transcode read");
    let input = output.GetInputStreamAt(0).map_err(e)?;
    let reader = DataReader::CreateDataReader(&input).map_err(e)?;
    reader.LoadAsync(size as u32).map_err(e)?.get().map_err(e)?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).map_err(e)?;
    Ok(bytes)
}

/// Windows の `MediaTranscode` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn MediaTranscode> {
    Arc::new(WindowsMediaTranscode)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use fandhe_device::media_playback::demo_pcm;
    use fandhe_device::media_transcode::{check_m4a, input_duration, SAMPLE_RATE};

    fn leftovers() -> usize {
        let prefix = format!("fandhe-media-transcode-{}-", std::process::id());
        std::fs::read_dir(std::env::temp_dir())
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                    .count()
            })
            .unwrap_or(0)
    }

    /// 成功（検証を通る m4a）か非対応のどちらか。一時ファイルは残らない。
    #[test]
    fn transcodes_or_reports_unsupported() {
        let pcm = demo_pcm(SAMPLE_RATE);
        match WindowsMediaTranscode.transcode(&pcm, SAMPLE_RATE, Duration::from_secs(30)) {
            Ok(bytes) => {
                check_m4a(&bytes, input_duration(pcm.len(), SAMPLE_RATE)).expect("m4a 検証");
            }
            Err(DeviceError::Unsupported(_)) => {}
            Err(e) => panic!("想定外の失敗: {e:?}"),
        }
        assert_eq!(leftovers(), 0, "一時ファイルが残っている");
    }

    /// 期限 0 は成功にならず、一時ファイルも残らない。
    #[test]
    fn zero_deadline_is_not_success() {
        let pcm = demo_pcm(SAMPLE_RATE);
        let r = WindowsMediaTranscode.transcode(&pcm, SAMPLE_RATE, Duration::ZERO);
        assert!(matches!(
            r,
            Err(DeviceError::Internal(TIMEOUT)) | Err(DeviceError::Unsupported(_))
        ));
        assert_eq!(leftovers(), 0, "一時ファイルが残っている");
    }
}
