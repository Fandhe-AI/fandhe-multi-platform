//! Windows 向け動画の変換（Issue #1015）。`Windows.Media.Transcoding.MediaTranscoder` で
//! 実行時に作った元動画（1280×720・15 fps・30 フレーム）を小さい動画の MP4 へ 1 回変換する。
//!
//! 呼び出し元: `device/video_transcode.rs::platform` の Windows 経路がワーカースレッドから
//! `transcode` を同期に呼ぶ。期限・世代の照合、期限後に届いた結果の破棄、出力の読み直し
//! （`check_mp4_video`）、画面向けの失敗の写像は呼び出し側が担い、本ファイルは MP4 のバイト列か
//! `DeviceError` だけを返す。契約の SSOT は `fandhe_device::video_transcode`。
//!
//! 2 段で行う:
//! 1. `MediaStreamSource`（無圧縮 BGRA、絵は `fill_source_frame_bgra`）+
//!    `PrepareMediaStreamSourceTranscodeAsync` で元動画を一時ファイルへ書く
//! 2. その一時ファイルを `PrepareStreamTranscodeAsync` で小さい動画（`Vga` プロファイル）へ変換する。
//!    入力が一時ファイルになる理由は、メモリ上のストリームを受けないため
//!    （<https://learn.microsoft.com/en-us/uwp/api/windows.media.transcoding.mediatranscoder.preparestreamtranscodeasync>）。
//!    出力寸法は OS が決める（trait doc の契約）
//!
//! 一時ファイルは `video_transcode::temp_pair` で名前を作り、`create_new` で作る（既存のファイルや
//! リンクを上書きしない）。ストリームを閉じてから `LEFTOVERS` 経由で消す（`Close()` 失敗も `PENDING` に積み、閉じ直せるまで次の要求を止める。変換が成功していれば結果は返す）。OS の操作が `Cancel()` 後の
//! 猶予内に止まったと確かめられなければ、消さずに未解放として記録し、次の要求は OS を呼ばず失敗にする。
//!
//! 期限は `remaining` から求めた `Instant`（単調時計）。OS 側の操作も期限で `Cancel()` する。
//!
//! 失敗の写像（全経路で一貫）:
//! - クラスなし（`REGDB_E_CLASSNOTREG`）・statics なし（`E_NOINTERFACE`）・`CodecNotFound`
//!   （Media Feature Pack のない N エディション等）→ `Unsupported`
//! - 期限切れ → `Internal(TIMEOUT)`
//! - それ以外 → 段階別の固定ラベルの `Internal`（HRESULT・OS のエラー文は出さない）
//!
//! `RoInitialize` は呼ばず（MTA フォールバック）、`unsafe` は使わない。
//!
//! 既知の天井: unpackaged の exe で動くかは未確認（#310）。実機確認は #1006。型検査は
//! `x86_64-pc-windows-gnu` のみ。

use std::sync::Arc;

use fandhe_device::video_transcode::{VideoTranscode, SOURCE_FPS, SOURCE_FRAMES};

/// 100 ns 単位の 1 秒（`TimeSpan` の単位）。
const TICKS_PER_SEC: i64 = 10_000_000;

/// 元動画の `index` 枚目の開始時刻（100 ns 単位）。
fn sample_time(index: u32) -> i64 {
    i64::from(index) * TICKS_PER_SEC / i64::from(SOURCE_FPS)
}

/// 元動画の 1 フレームの長さ（100 ns 単位）。
fn frame_duration() -> i64 {
    TICKS_PER_SEC / i64::from(SOURCE_FPS)
}

/// 元動画全体の長さ（100 ns 単位）。
fn source_duration_ticks() -> i64 {
    sample_time(SOURCE_FRAMES)
}

/// Windows の `VideoTranscode` 実装を返す（Windows 以外のホストテストでは非対応）。
pub(crate) fn implementation() -> Arc<dyn VideoTranscode> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(imp::WindowsVideoTranscode)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(fandhe_device::video_transcode::UnsupportedVideoTranscode)
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::sync::{Mutex, TryLockError};
    use std::time::{Duration, Instant};

    use fandhe_device::video_transcode::{
        fill_source_frame_bgra, VideoTranscode, RELEASE_PENDING, SOURCE_FPS, SOURCE_HEIGHT,
        SOURCE_WIDTH, TIMEOUT,
    };
    use fandhe_device::DeviceError;
    use windows::core::HSTRING;
    use windows::Foundation::{TimeSpan, TypedEventHandler};
    use windows::Media::Core::{
        MediaStreamSample, MediaStreamSource, MediaStreamSourceSampleRequestedEventArgs,
        VideoStreamDescriptor,
    };
    use windows::Media::MediaProperties::{
        AudioEncodingProperties, MediaEncodingProfile, MediaEncodingSubtypes,
        VideoEncodingProperties, VideoEncodingQuality,
    };
    use windows::Media::Transcoding::{
        MediaTranscoder, PrepareTranscodeResult, TranscodeFailureReason,
    };
    use windows::Storage::FileAccessMode;
    use windows::Storage::Streams::{DataWriter, FileRandomAccessStream, IRandomAccessStream};
    use windows::Win32::Foundation::{E_NOINTERFACE, REGDB_E_CLASSNOTREG};
    use windows_future::{
        AsyncActionWithProgressCompletedHandler, AsyncOperationCompletedHandler, AsyncStatus,
        IAsyncOperation,
    };

    use super::{frame_duration, sample_time, source_duration_ticks};
    use crate::device::video_transcode::{temp_pair, LEFTOVERS};

    /// 出力の上限。`u32` への切り詰めと過大な確保を防ぐ。
    const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

    /// `Cancel()` 後に OS 側の完了通知を待つ上限（契約表 3・9）。
    const CANCEL_GRACE: Duration = Duration::from_millis(500);

    /// 同時に 1 本しか OS 操作を走らせない。
    static CALL: Mutex<()> = Mutex::new(());

    /// 完了を確かめられないまま戻った OS 操作の「まだ動いている」判定。すべて false になるまで
    /// 次の要求を受けない。
    static PENDING: Mutex<Vec<Box<dyn Fn() -> bool + Send>>> = Mutex::new(Vec::new());

    /// 終わっていない OS 操作が残っているか（終わったものは外す）。
    fn pending_alive() -> bool {
        let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|alive| alive());
        !g.is_empty()
    }

    /// ストリームを閉じる。`Close()` が失敗したら握りつぶさず `PENDING` に積み、閉じ直せるまで
    /// 次の要求を止める（一時ファイルも消さない）。変換自体が成功していれば結果は返す。
    fn close_stream(s: &IRandomAccessStream) {
        if s.Close().is_err() {
            let s = s.clone();
            PENDING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Box::new(move || s.Close().is_err()));
        }
    }

    fn internal(label: &'static str) -> DeviceError {
        DeviceError::Internal(label)
    }

    /// クラスなし・statics なしは非対応、それ以外は段階別ラベルの `Internal`。
    fn map_err(e: &windows::core::Error, label: &'static str) -> DeviceError {
        if e.code() == REGDB_E_CLASSNOTREG || e.code() == E_NOINTERFACE {
            DeviceError::Unsupported("video_transcode")
        } else {
            internal(label)
        }
    }

    /// サンプル生成側（`SampleRequested`）で起きたエラーをワーカーへ渡す枠。最初の 1 件だけ残す。
    type Fault = std::sync::Arc<Mutex<Option<DeviceError>>>;

    fn take_fault(fault: &Fault) -> Option<DeviceError> {
        fault.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// 完了通知の送り先。チャネル（待機側）と旗（待機側が去った後も立つ）の両方へ知らせる。
    struct Notify {
        tx: mpsc::Sender<AsyncStatus>,
        done: Arc<AtomicBool>,
    }

    impl Notify {
        fn send(&self, status: AsyncStatus) {
            self.done.store(true, Ordering::SeqCst);
            let _ = self.tx.send(status);
        }
    }

    /// `deadline` まで完了を待ち、届いた終了状態（Completed / Error / Canceled）を返す。
    /// 期限切れは `cancel()` して猶予だけ待ち、完了が届かなければ `PENDING` に積む。
    /// 通知の登録に失敗したときも `cancel()` し、`status` を猶予だけ見て止まらなければ
    /// 操作自体を `PENDING` に積む（`status` が `Started` 以外になるまで次の要求を止める）。
    fn wait(
        deadline: Instant,
        label: &'static str,
        register: impl FnOnce(Notify) -> windows::core::Result<()>,
        cancel: impl Fn(),
        status: impl Fn() -> windows::core::Result<AsyncStatus> + Send + 'static,
    ) -> Result<AsyncStatus, DeviceError> {
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        if register(Notify {
            tx,
            done: done.clone(),
        })
        .is_err()
        {
            cancel();
            let running = move || !matches!(status(), Ok(s) if s != AsyncStatus::Started);
            let grace_end = Instant::now() + CANCEL_GRACE;
            while running() && Instant::now() < grace_end {
                std::thread::sleep(Duration::from_millis(10));
            }
            if running() {
                PENDING
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(Box::new(running));
            }
            return Err(internal(label));
        }
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(st) => Ok(st),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancel();
                // OS 側の操作が止まるまで待つ。届かなければ旗を残して次の要求を止める。
                if rx.recv_timeout(CANCEL_GRACE).is_err() {
                    PENDING
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(Box::new(move || !done.load(Ordering::SeqCst)));
                }
                Err(internal(TIMEOUT))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(internal(label)),
        }
    }

    /// 終了状態から結果を決める。Error は `result`（`GetResults()`）が持つ元の HRESULT を
    /// 同期失敗と同じ `map_err` で写像する。
    fn settle<T>(
        st: AsyncStatus,
        result: windows::core::Result<T>,
        label: &'static str,
    ) -> Result<T, DeviceError> {
        match (st, result) {
            (AsyncStatus::Completed, Ok(v)) => Ok(v),
            (AsyncStatus::Completed | AsyncStatus::Error, Err(e)) => Err(map_err(&e, label)),
            _ => Err(internal(label)),
        }
    }

    /// `IAsyncOperation` を期限つきで待って結果を取る。
    fn wait_op<T: windows::core::RuntimeType + 'static>(
        deadline: Instant,
        label: &'static str,
        op: &IAsyncOperation<T>,
    ) -> Result<T, DeviceError> {
        let status_op = op.clone();
        let st = wait(
            deadline,
            label,
            |n| {
                op.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, status| {
                    n.send(status);
                    Ok(())
                }))
            },
            || {
                let _ = op.Cancel();
            },
            move || status_op.Status(),
        )?;
        if matches!(st, AsyncStatus::Completed | AsyncStatus::Error) {
            settle(st, op.GetResults(), label)
        } else {
            Err(internal(label))
        }
    }

    /// 既存パスをファイルストリームとして開く。期限切れ直前に開けていたら閉じる。
    fn open_stream(
        path: &Path,
        mode: FileAccessMode,
        deadline: Instant,
        label: &'static str,
    ) -> Result<IRandomAccessStream, DeviceError> {
        let open = FileRandomAccessStream::OpenAsync(&HSTRING::from(path.as_os_str()), mode)
            .map_err(|e| map_err(&e, label))?;
        wait_op(deadline, label, &open).inspect_err(|_| {
            if let Ok(s) = open.GetResults() {
                close_stream(&s);
            }
        })
    }

    /// 準備の結果から変換を実行して待つ。`CanTranscode` が false なら理由で写像する。
    fn run_prepared(
        prep: IAsyncOperation<PrepareTranscodeResult>,
        deadline: Instant,
        label: &'static str,
    ) -> Result<(), DeviceError> {
        let prepared = wait_op(deadline, "video_transcode prepare", &prep)?;
        if !prepared.CanTranscode().unwrap_or(false) {
            return Err(match prepared.FailureReason() {
                Ok(TranscodeFailureReason::CodecNotFound) => {
                    DeviceError::Unsupported("video_transcode")
                }
                _ => internal("video_transcode prepare"),
            });
        }
        let action = prepared.TranscodeAsync().map_err(|e| map_err(&e, label))?;
        let status_action = action.clone();
        let st = wait(
            deadline,
            label,
            |n| {
                action.SetCompleted(&AsyncActionWithProgressCompletedHandler::new(
                    move |_, status| {
                        n.send(status);
                        Ok(())
                    },
                ))
            },
            || {
                let _ = action.Cancel();
            },
            move || status_action.Status(),
        )?;
        if matches!(st, AsyncStatus::Completed | AsyncStatus::Error) {
            settle(st, action.GetResults(), label)
        } else {
            Err(internal(label))
        }
    }

    /// 元動画のフレームを順に渡す `MediaStreamSource`。`SOURCE_FRAMES` 枚で EOS にする。
    /// 失敗は `fault` に残して EOS にし、呼び出し側が成功にしない。
    fn build_source(fault: &Fault) -> Result<MediaStreamSource, DeviceError> {
        let err = |e: windows::core::Error| map_err(&e, "video_transcode source");
        let subtype = MediaEncodingSubtypes::Bgra8().map_err(err)?;
        let props =
            VideoEncodingProperties::CreateUncompressed(&subtype, SOURCE_WIDTH, SOURCE_HEIGHT)
                .map_err(err)?;
        props
            .FrameRate()
            .and_then(|r| {
                r.SetNumerator(SOURCE_FPS)?;
                r.SetDenominator(1)
            })
            .map_err(err)?;
        let descriptor = VideoStreamDescriptor::Create(&props).map_err(err)?;
        let source = MediaStreamSource::CreateFromDescriptor(&descriptor).map_err(err)?;
        source
            .SetBufferTime(TimeSpan { Duration: 0 })
            .and_then(|()| {
                source.SetDuration(TimeSpan {
                    Duration: source_duration_ticks(),
                })
            })
            .map_err(err)?;

        let fault = fault.clone();
        let stride = SOURCE_WIDTH as usize * 4;
        let mut buf = vec![0u8; stride * SOURCE_HEIGHT as usize];
        let mut index = 0u32;
        source
            .SampleRequested(&TypedEventHandler::<
                MediaStreamSource,
                MediaStreamSourceSampleRequestedEventArgs,
            >::new(move |_, args| {
                let request = args.ok()?.Request()?;
                if index >= fandhe_device::video_transcode::SOURCE_FRAMES {
                    return Ok(()); // サンプル無し = EOS
                }
                let mut produce = || -> Result<(), DeviceError> {
                    let e = |e: windows::core::Error| map_err(&e, "video_transcode source");
                    if !fill_source_frame_bgra(index, SOURCE_WIDTH, SOURCE_HEIGHT, stride, &mut buf)
                    {
                        return Err(internal("video_transcode source"));
                    }
                    let writer = DataWriter::new().map_err(e)?;
                    writer.WriteBytes(&buf).map_err(e)?;
                    let buffer = writer.DetachBuffer().map_err(e)?;
                    let sample = MediaStreamSample::CreateFromBuffer(
                        &buffer,
                        TimeSpan {
                            Duration: sample_time(index),
                        },
                    )
                    .map_err(e)?;
                    sample
                        .SetDuration(TimeSpan {
                            Duration: frame_duration(),
                        })
                        .map_err(e)?;
                    request.SetSample(&sample).map_err(e)
                };
                match produce() {
                    Ok(()) => index += 1,
                    Err(e) => {
                        // 握りつぶさずワーカーへ伝える: 失敗を残して EOS にする。
                        fault
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .get_or_insert(e);
                        index = fandhe_device::video_transcode::SOURCE_FRAMES;
                    }
                }
                Ok(())
            }))
            .map_err(err)?;
        Ok(source)
    }

    /// 1 段目: 元動画を `src` へ書く。
    fn stage1(src: &Path, deadline: Instant) -> Result<(), DeviceError> {
        let err = |e: windows::core::Error| map_err(&e, "video_transcode prepare");
        let fault: Fault = std::sync::Arc::new(Mutex::new(None));
        let source = build_source(&fault)?;
        let profile = MediaEncodingProfile::CreateMp4(VideoEncodingQuality::HD720p).map_err(err)?;
        // ソースは映像のみ。既定の AAC 音声を残すと `CanTranscode` が false になる。
        profile
            .SetAudio(None::<&AudioEncodingProperties>)
            .map_err(err)?;
        let video = profile.Video().map_err(err)?;
        video
            .SetWidth(SOURCE_WIDTH)
            .and_then(|()| video.SetHeight(SOURCE_HEIGHT))
            .and_then(|()| {
                let r = video.FrameRate()?;
                r.SetNumerator(SOURCE_FPS)?;
                r.SetDenominator(1)
            })
            .map_err(err)?;
        let transcoder = MediaTranscoder::new().map_err(err)?;
        let output = open_stream(
            src,
            FileAccessMode::ReadWrite,
            deadline,
            "video_transcode source",
        )?;
        let result = transcoder
            .PrepareMediaStreamSourceTranscodeAsync(&source, &output, &profile)
            .map_err(err)
            .and_then(|prep| run_prepared(prep, deadline, "video_transcode transcode"))
            .and_then(|()| take_fault(&fault).map_or(Ok(()), Err));
        close_stream(&output);
        result
    }

    /// 2 段目: `src` を小さい動画にして `out` へ書く。寸法は指定せず OS に任せる。
    fn stage2(src: &Path, out: &Path, deadline: Instant) -> Result<(), DeviceError> {
        let err = |e: windows::core::Error| map_err(&e, "video_transcode prepare");
        let profile = MediaEncodingProfile::CreateMp4(VideoEncodingQuality::Vga).map_err(err)?;
        profile
            .SetAudio(None::<&AudioEncodingProperties>)
            .map_err(err)?;
        let transcoder = MediaTranscoder::new().map_err(err)?;
        let input = open_stream(
            src,
            FileAccessMode::Read,
            deadline,
            "video_transcode source",
        )?;
        let output = match open_stream(
            out,
            FileAccessMode::ReadWrite,
            deadline,
            "video_transcode output",
        ) {
            Ok(o) => o,
            Err(e) => {
                close_stream(&input);
                return Err(e);
            }
        };
        let result = transcoder
            .PrepareStreamTranscodeAsync(&input, &output, &profile)
            .map_err(err)
            .and_then(|prep| run_prepared(prep, deadline, "video_transcode transcode"));
        close_stream(&output);
        close_stream(&input);
        result
    }

    /// `create_new` で空ファイルを作る（既存のファイルやリンクを上書きしない）。
    fn create_new(path: &Path) -> Result<(), DeviceError> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(drop)
            .map_err(|_| internal("video_transcode path"))
    }

    fn read_output(out: &Path) -> Result<Vec<u8>, DeviceError> {
        let len = std::fs::metadata(out)
            .map_err(|_| internal("video_transcode read"))?
            .len();
        if len > MAX_OUTPUT_BYTES {
            return Err(internal("video_transcode read"));
        }
        std::fs::read(out).map_err(|_| internal("video_transcode read"))
    }

    /// Windows の動画の変換（MediaTranscoder）。
    pub(crate) struct WindowsVideoTranscode;

    impl VideoTranscode for WindowsVideoTranscode {
        fn transcode(&self, remaining: Duration) -> Result<Vec<u8>, DeviceError> {
            let _call = match CALL.try_lock() {
                Ok(g) => g,
                Err(TryLockError::Poisoned(p)) => p.into_inner(),
                Err(TryLockError::WouldBlock) => return Err(internal(RELEASE_PENDING)),
            };
            if pending_alive() {
                return Err(internal(RELEASE_PENDING));
            }
            LEFTOVERS.ensure_released()?;
            let deadline = Instant::now() + remaining;
            let (src, out) = temp_pair();
            create_new(&src)?;
            if let Err(e) = create_new(&out) {
                LEFTOVERS.release(&[&src]);
                return Err(e);
            }
            let result = stage1(&src, deadline)
                .and_then(|()| stage2(&src, &out, deadline))
                .and_then(|()| read_output(&out));
            if pending_alive() {
                // OS の操作が止まったと確かめられない間は消さない（次の要求の前に確かめる）。
                LEFTOVERS.mark_unreleased(&src);
                LEFTOVERS.mark_unreleased(&out);
            } else {
                LEFTOVERS.release(&[&src, &out]);
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_times_span_the_source_duration() {
        assert_eq!(sample_time(0), 0);
        assert_eq!(sample_time(SOURCE_FRAMES), TICKS_PER_SEC * 2);
        assert!(sample_time(1) > 0 && sample_time(1) <= frame_duration() + 1);
        assert_eq!(source_duration_ticks(), TICKS_PER_SEC * 2);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn host_implementation_is_unsupported() {
        use std::time::Duration;
        assert!(matches!(
            implementation().transcode(Duration::from_secs(1)),
            Err(fandhe_device::DeviceError::Unsupported(_))
        ));
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::imp::WindowsVideoTranscode;
    use fandhe_device::video_transcode::{
        check_mp4_video, VideoTranscode, RELEASE_PENDING, SOURCE_HEIGHT, SOURCE_WIDTH, TIMEOUT,
    };
    use fandhe_device::DeviceError;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// 実装はプロセス共有の状態（CALL / PENDING / LEFTOVERS）を持つため、テストは直列に走らせる。
    static SERIAL: Mutex<()> = Mutex::new(());

    fn leftovers() -> usize {
        let prefix = format!("fandhe-video-transcode-{}-", std::process::id());
        std::fs::read_dir(std::env::temp_dir())
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                    .count()
            })
            .unwrap_or(0)
    }

    /// 前回の解放待ち（`RELEASE_PENDING`）が解けるまで、次の要求として呼び直す（最大 10 秒）。
    /// 期限切れ直後は未解放のまま残りうる（遅延解放の契約）。解放は次の要求で確認される。
    fn transcode_after_release(remaining: Duration) -> Result<Vec<u8>, DeviceError> {
        let give_up = Instant::now() + Duration::from_secs(10);
        loop {
            match WindowsVideoTranscode.transcode(remaining) {
                Err(DeviceError::Internal(RELEASE_PENDING)) if Instant::now() < give_up => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                r => return r,
            }
        }
    }

    /// 成功（検証を通り、元より小さい）か非対応のどちらか。一時ファイルは残らない。
    #[test]
    fn transcodes_or_reports_unsupported() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        match transcode_after_release(Duration::from_secs(30)) {
            Ok(bytes) => {
                let info = check_mp4_video(&bytes).expect("mp4 検証");
                assert!(info.width < SOURCE_WIDTH && info.height < SOURCE_HEIGHT);
            }
            Err(DeviceError::Unsupported(_)) => {}
            Err(e) => panic!("想定外の失敗: {e:?}"),
        }
        assert_eq!(leftovers(), 0, "一時ファイルが残っている");
    }

    /// 期限 0 は成功にならない。期限切れ直後は未解放で残りうるため、次の要求で再削除されてから
    /// 一時ファイルが残らないことを確かめる。
    #[test]
    fn zero_deadline_is_not_success() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let r = transcode_after_release(Duration::ZERO);
        assert!(matches!(
            r,
            Err(DeviceError::Internal(TIMEOUT)) | Err(DeviceError::Unsupported(_))
        ));
        // 次の要求（解放待ちが解けるまで）で再削除される。
        let _ = transcode_after_release(Duration::from_secs(30));
        assert_eq!(leftovers(), 0, "次の要求の後も一時ファイルが残っている");
    }
}
