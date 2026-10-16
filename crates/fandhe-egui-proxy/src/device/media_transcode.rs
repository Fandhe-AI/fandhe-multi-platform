//! Phase 11（端末機能の拡充ループ、親 #91）型サンプル「動画・音声の変換」（Issue #311）の effect ドライバ。
//! `fandhe_core::screens::media_transcode`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::media_transcode::MediaTranscode` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::media_transcode`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は対象外
//! （`fandhe-web-dom` 側で扱う）。
//!
//! # OS 別の分担（dispatch は #311 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_media_transcode::AppleMediaTranscode`]（#311）
//! - iOS: `ios_media_transcode`（b-ios）
//! - Windows: `windows_media_transcode`（windows の子。MediaTranscoder）
//! - Android: `android_media_transcode`（b-android。MediaCodec + MediaMuxer）
//! - それ以外（Linux）: `UnsupportedMediaTranscode` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中でセッションの世代が一致するとき `elapsed >= deadline` なら `Timeout` を確定して
//! セッションを空にし、まだなら `request_repaint_after` で次の判定を予約する。確定後に
//! ワーカーが返す結果は画面側の世代・状態照合（`finish_transcode`）で捨てられる。ワーカー自身も
//! 期限超過を `Timeout` にし、期限後に届いた成功を成功として扱わない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::media_transcode::{
    finish_transcode, mark_running, take_request, TranscodeFailure, TranscodeFormat,
    TranscodeOutput, TranscodePhase, TranscodeState,
};
use fandhe_device::media_playback::demo_pcm;
use fandhe_device::media_transcode::{
    check_m4a, input_duration, validate_input, MediaTranscode, EMPTY, MISMATCH, SAMPLE_RATE,
    TIMEOUT, TRANSCODE_DEADLINE, UNREADABLE,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// WAV のヘッダ長（入力の大きさの表示用）。
const WAV_HEADER_BYTES: u64 = 44;

/// 現在のプラットフォームに応じた `MediaTranscode` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn MediaTranscode> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_media_transcode::AppleMediaTranscode)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_media_transcode::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_media_transcode::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_media_transcode::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        Arc::new(fandhe_device::media_transcode::UnsupportedMediaTranscode)
    }
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
/// 進行中の要求の `(世代, 受付時刻)`。
#[derive(Default)]
struct Session {
    inflight: Mutex<Option<(u64, Instant)>>,
}

static SESSION: std::sync::OnceLock<Session> = std::sync::OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、ドライバの処理済み記録を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、完了後も残る記録（世代 1）を残すと次の
/// 「変換する」が発送されず飲み込まれる。古いワーカーの結果は古いアプリのチャネルへ届くので
/// 新しい画面には適用されない。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        *s.inflight.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コードは画面にもログにも出さず、
/// 上記以外は固定文言にする（security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> TranscodeFailure {
    match e {
        DeviceError::Unsupported(_) => TranscodeFailure::Unsupported,
        DeviceError::Internal(TIMEOUT) => TranscodeFailure::Timeout,
        DeviceError::Internal(UNREADABLE) => TranscodeFailure::Unreadable,
        DeviceError::Internal(MISMATCH) => TranscodeFailure::Mismatch,
        DeviceError::Internal(EMPTY) => TranscodeFailure::Empty,
        _ => TranscodeFailure::Failed("変換できませんでした".to_string()),
    }
}

/// ワーカーの本体: 入力の生成と検査 → 変換 → 期限の確認 → 出力の読み直し。
fn run_worker(
    imp: &dyn MediaTranscode,
    started: Instant,
    deadline: Duration,
) -> Result<TranscodeOutput, TranscodeFailure> {
    let pcm = demo_pcm(SAMPLE_RATE);
    validate_input(&pcm, SAMPLE_RATE).map_err(to_failure)?;
    let result = imp.transcode(
        &pcm,
        SAMPLE_RATE,
        deadline.saturating_sub(started.elapsed()),
    );
    // 期限後に届いた結果は成功にしない（中身は見ない）。
    if started.elapsed() > deadline {
        return Err(TranscodeFailure::Timeout);
    }
    let bytes = result.map_err(to_failure)?;
    let input = input_duration(pcm.len(), SAMPLE_RATE);
    let output = check_m4a(&bytes, input).map_err(to_failure)?;
    Ok(TranscodeOutput {
        format: TranscodeFormat::AacM4a,
        input_bytes: WAV_HEADER_BYTES + pcm.len() as u64 * 2,
        input_ms: input.as_millis() as u64,
        output_bytes: bytes.len() as u64,
        output_ms: output.as_millis() as u64,
    })
}

/// 1 フレームぶんの処理。要求の発送と期限判定を行う。
fn drive(
    state: &TranscodeState,
    sender: &CompletionSender<TranscodeState>,
    imp: &Arc<dyn MediaTranscode>,
    session: &Session,
    deadline: Duration,
) {
    // 要求の観測と記録は同じロックの内側で行い、二重発送を防ぐ。
    let new_request = {
        let mut g = session.inflight.lock().unwrap_or_else(|e| e.into_inner());
        match take_request(state) {
            Some(generation) if g.map(|(gen, _)| gen) != Some(generation) => {
                let started = Instant::now();
                *g = Some((generation, started));
                Some((generation, started))
            }
            _ => {
                // 期限判定: 処理中でセッションの世代が一致するときだけ。
                if let (Some((gen, started)), TranscodePhase::Running) = (*g, &state.phase) {
                    if gen == state.generation {
                        let elapsed = started.elapsed();
                        if elapsed >= deadline {
                            *g = None;
                            let _ = sender.send(Box::new(move |s: &mut TranscodeState| {
                                finish_transcode(s, gen, Err(TranscodeFailure::Timeout));
                            }));
                        } else {
                            sender.request_repaint_after(deadline - elapsed);
                        }
                    }
                }
                None
            }
        }
    };
    let Some((generation, started)) = new_request else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    sender.request_repaint_after(deadline);

    let worker_sender = sender.clone();
    let worker_imp = imp.clone();
    let spawned = std::thread::Builder::new()
        .name("media-transcode".into())
        .spawn(move || {
            let result = run_worker(&*worker_imp, started, deadline);
            let _ = worker_sender.send(Box::new(move |s: &mut TranscodeState| {
                finish_transcode(s, generation, result);
            }));
        });
    if spawned.is_err() {
        let _ = sender.send(Box::new(move |s: &mut TranscodeState| {
            // `mark_running` の後に適用されるよう同じキューへ積む。
            finish_transcode(
                s,
                generation,
                Err(TranscodeFailure::Failed("変換できませんでした".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `media_transcode` 画面用に配線される effect ドライバ。
pub fn media_transcode_effect(state: &TranscodeState, sender: &CompletionSender<TranscodeState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn MediaTranscode>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        TRANSCODE_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::media_transcode::request_transcode;
    use fandhe_device::media_transcode::{minimal_m4a, UnsupportedMediaTranscode};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<TranscodeState>>;

    fn channel() -> (CompletionSender<TranscodeState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 入力と同じ長さ（5 秒）の最小の m4a。
    fn good() -> Vec<u8> {
        minimal_m4a(b"mp4a", 48_000, 240_000, 235, false)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Result<Vec<u8>, DeviceError>,
        sleep: Duration,
    }
    impl MediaTranscode for Fake {
        fn transcode(&self, _: &[i16], _: u32, _: Duration) -> Result<Vec<u8>, DeviceError> {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<Vec<u8>, DeviceError>) -> Arc<dyn MediaTranscode> {
        Arc::new(Fake {
            result,
            sleep: Duration::ZERO,
        })
    }

    /// 要求を立てて 1 フレーム回し、届いた完了を `n` 件適用した状態を返す。
    fn run(imp: Arc<dyn MediaTranscode>, n: usize) -> TranscodeState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = TranscodeState::default();
        request_transcode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        for _ in 0..n {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了が届く");
            c(&mut state);
        }
        state
    }

    fn failed(s: &TranscodeState) -> &TranscodeFailure {
        match &s.phase {
            TranscodePhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        drive(
            &TranscodeState::default(),
            &sender,
            &fake(|| Ok(good())),
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_fills_sizes_and_durations() {
        let s = run(fake(|| Ok(good())), 2);
        match s.phase {
            TranscodePhase::Done(o) => {
                assert_eq!(o.format, TranscodeFormat::AacM4a);
                assert_eq!(o.input_bytes, 44 + 240_000 * 2);
                assert_eq!((o.input_ms, o.output_ms), (5000, 5000));
                assert_eq!(o.output_bytes, good().len() as u64);
            }
            p => panic!("成功のはず: {p:?}"),
        }
    }

    #[test]
    fn empty_garbage_mismatch_unsupported_map_to_their_failures() {
        let s = run(fake(|| Ok(Vec::new())), 2);
        assert_eq!(failed(&s), &TranscodeFailure::Empty);
        let s = run(fake(|| Ok(b"garbage".to_vec())), 2);
        assert_eq!(failed(&s), &TranscodeFailure::Unreadable);
        let s = run(
            fake(|| Ok(minimal_m4a(b"mp4a", 48_000, 48_000, 10, false))),
            2,
        );
        assert_eq!(failed(&s), &TranscodeFailure::Mismatch);
        let s = run(Arc::new(UnsupportedMediaTranscode), 2);
        assert_eq!(failed(&s), &TranscodeFailure::Unsupported);
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))), 2);
        let text = fandhe_core::screens::media_transcode::error_text(&s);
        assert!(!text.contains("secret"), "{text}");
        assert!(matches!(failed(&s), TranscodeFailure::Failed(_)));
    }

    #[test]
    fn worker_past_deadline_is_timeout_even_with_good_output() {
        let imp: Arc<dyn MediaTranscode> = Arc::new(Fake {
            result: || Ok(good()),
            sleep: Duration::from_millis(60),
        });
        let (sender, rx) = channel();
        let mut state = TranscodeState::default();
        request_transcode(&mut state);
        drive(
            &state,
            &sender,
            &imp,
            &Session::default(),
            Duration::from_millis(20),
        );
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(failed(&state), &TranscodeFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn MediaTranscode> = Arc::new(Fake {
            result: || Ok(good()),
            sleep: Duration::from_millis(300),
        });
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = TranscodeState::default();
        request_transcode(&mut state);
        let deadline = Duration::from_millis(30);
        drive(&state, &sender, &imp, &session, deadline);
        // mark_running を適用
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(60));
        drive(&state, &sender, &imp, &session, deadline);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &TranscodeFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &TranscodeFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = TranscodeState::default();
        request_transcode(&mut state);
        mark_running(&mut state);
        finish_transcode(
            &mut state,
            99,
            Err(TranscodeFailure::Failed("x".to_string())),
        );
        assert_eq!(state.phase, TranscodePhase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok(good()));
        let session = Session::default();
        let mut state = TranscodeState::default();
        request_transcode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(200));
        // mark_running 1 + finish 1
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 2);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok(good()));
        let session = Session::default();
        *session.inflight.lock().unwrap() = Some((1, Instant::now()));
        let mut state = TranscodeState::default();
        request_transcode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        *session.inflight.lock().unwrap() = None;
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "android"
        )))]
        assert!(matches!(
            platform().transcode(&[1], SAMPLE_RATE, Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
