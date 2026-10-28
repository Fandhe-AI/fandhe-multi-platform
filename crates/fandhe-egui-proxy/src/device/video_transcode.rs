//! Phase 11（端末機能の拡充ループ、親 #340）型サンプル「動画の変換」（Issue #1014）の effect ドライバ。
//! `fandhe_core::screens::video_transcode`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::video_transcode::VideoTranscode` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・解放の失敗・守る範囲）の SSOT は
//! `fandhe_device::video_transcode`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は対象外
//! （`fandhe-web-dom` 側で扱う）。
//!
//! # OS 別の分担（dispatch は #1014 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_video_transcode::AppleVideoTranscode`]（#1014）
//! - iOS: `ios_video_transcode` 経由で `AppleVideoTranscode`（#1016 b-ios）
//! - Windows: `windows_video_transcode`（#1015。いまはスタブ）
//! - Android: `android_video_transcode`（#1017 b-android。いまはスタブ）
//! - それ以外（Linux）: `UnsupportedVideoTranscode` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中でセッションの世代が一致するとき `elapsed >= deadline` なら `Timeout` を確定して
//! セッションを空にし、まだなら `request_repaint_after` で次の判定を予約する。確定後に
//! ワーカーが返す結果は画面側の世代・状態照合（`finish_transcode`）で捨てられる。ワーカー自身も
//! 期限超過を `Timeout` にし、期限後に届いた成功を成功として扱わない。
//!
//! # 一時ファイルの追跡（契約表 9）
//!
//! 変換は一時ファイルを経由する実装（Apple・Windows）が使う [`Leftovers`] を本モジュールに置く。
//! 消せなかった一時ファイルは記録し、次の要求の開始前に [`Leftovers::ensure_released`] で再削除する。
//! 確かめられなければ OS を呼ばずに `RELEASE_PENDING` で失敗にする。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::video_transcode::{
    finish_transcode, mark_running, take_request, VideoContainer, VideoTranscodeFailure,
    VideoTranscodeOutput, VideoTranscodePhase, VideoTranscodeState, GENERIC_FAILED_MESSAGE,
    RELEASE_PENDING_MESSAGE,
};
use fandhe_device::video_transcode::{
    check_mp4_video, source_duration, VideoTranscode, EMPTY, MISMATCH, NOT_SMALLER,
    RELEASE_PENDING, SOURCE_HEIGHT, SOURCE_WIDTH, TIMEOUT, TRANSCODE_DEADLINE, UNREADABLE,
};
use fandhe_device::DeviceError;

use super::request_slot::RequestSlot;
use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `VideoTranscode` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn VideoTranscode> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_video_transcode::AppleVideoTranscode)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_video_transcode::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_video_transcode::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_video_transcode::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        Arc::new(fandhe_device::video_transcode::UnsupportedVideoTranscode)
    }
}

/// 消せなかった一時ファイルの追跡（Apple・Windows の実装が使う）。
///
/// 解放の失敗を握りつぶして「閉じた」扱いにしない（契約表 9）。記録は次の要求の開始前に
/// [`Self::ensure_released`] が再削除して確かめる。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) struct Leftovers(Mutex<Vec<PathBuf>>);

/// プロセス内で 1 つの追跡表（一時ファイルは `temp_dir` の共有領域に置かれるため）。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) static LEFTOVERS: Leftovers = Leftovers::new();

/// ファイルを消す。存在しなければ成功扱い。
fn remove_if_exists(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
impl Leftovers {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<PathBuf>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `paths` を消す。NotFound は成功扱い、それ以外の失敗は未解放として記録する。
    pub(crate) fn release(&self, paths: &[&Path]) {
        for p in paths {
            if !remove_if_exists(p) {
                self.mark_unreleased(p);
            }
        }
    }

    /// OS 操作がまだ終わっている保証がないなど、消す前に戻る経路で未解放として記録する。
    pub(crate) fn mark_unreleased(&self, path: &Path) {
        let mut g = self.lock();
        if !g.iter().any(|q| q == path) {
            g.push(path.to_path_buf());
        }
    }

    /// 記録を消し直す。消せないものが残れば `Internal(RELEASE_PENDING)`。
    pub(crate) fn ensure_released(&self) -> Result<(), DeviceError> {
        let mut g = self.lock();
        g.retain(|p| !remove_if_exists(p));
        if g.is_empty() {
            Ok(())
        } else {
            Err(DeviceError::Internal(RELEASE_PENDING))
        }
    }
}

/// 元動画と出力の一時ファイル `(src, out)` のパスを作る（内部生成のみ。外部入力を含めない）。
/// `temp_dir()/fandhe-video-transcode-<pid>-<seq>-{src,out}.mp4`。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) fn temp_pair() -> (PathBuf, PathBuf) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir();
    let stem = format!("fandhe-video-transcode-{}-{seq}", std::process::id());
    (
        dir.join(format!("{stem}-src.mp4")),
        dir.join(format!("{stem}-out.mp4")),
    )
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
///
/// 要求 1 件の追跡（採番・二重発送防止・破棄）は共通部品 [`RequestSlot`] に任せる。
/// `poll_and_finish` は通さない: 結果を置く側が OS コールバックではなく本ドライバ自身の
/// ワーカーで、完了は `CompletionSender` へ直接 push できるため。
/// 変換固有の期限管理（受付時刻と画面の世代）だけを `timing` に残す。
struct Session {
    slot: RequestSlot<()>,
    /// 進行中の要求の `(request_id, 画面の世代, 受付時刻)`。
    timing: Mutex<Option<(u64, u64, Instant)>>,
}

impl Session {
    const fn new() -> Self {
        Self {
            slot: RequestSlot::new(),
            timing: Mutex::new(None),
        }
    }

    fn reset(&self) {
        self.slot.reset();
        *self.timing.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

static SESSION: std::sync::OnceLock<Arc<Session>> = std::sync::OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、ドライバの処理済み記録を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、完了後も残る記録（世代 1）を残すと次の
/// 「変換する」が発送されず飲み込まれる。古いワーカーの結果は古いアプリのチャネルへ届くので
/// 新しい画面には適用されない。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.reset();
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コードは画面にもログにも出さず、
/// 上記以外は固定文言にする（security.md A09）。
pub(crate) fn to_failure(e: DeviceError) -> VideoTranscodeFailure {
    match e {
        DeviceError::Unsupported(_) => VideoTranscodeFailure::Unsupported,
        DeviceError::Internal(TIMEOUT) => VideoTranscodeFailure::Timeout,
        DeviceError::Internal(EMPTY) => VideoTranscodeFailure::Empty,
        DeviceError::Internal(UNREADABLE) => VideoTranscodeFailure::Unreadable,
        DeviceError::Internal(NOT_SMALLER) => VideoTranscodeFailure::NotSmaller,
        DeviceError::Internal(MISMATCH) => VideoTranscodeFailure::Mismatch,
        DeviceError::Internal(RELEASE_PENDING) => {
            VideoTranscodeFailure::Failed(RELEASE_PENDING_MESSAGE.to_string())
        }
        _ => VideoTranscodeFailure::Failed(GENERIC_FAILED_MESSAGE.to_string()),
    }
}

/// ワーカーの本体: 変換 → 期限の確認 → 出力の読み直し。
fn run_worker(
    imp: &dyn VideoTranscode,
    started: Instant,
    deadline: Duration,
) -> Result<VideoTranscodeOutput, VideoTranscodeFailure> {
    let result = imp.transcode(deadline.saturating_sub(started.elapsed()));
    // 期限後に届いた結果は成功にしない（中身は見ない）。
    if started.elapsed() > deadline {
        return Err(VideoTranscodeFailure::Timeout);
    }
    let bytes = result.map_err(to_failure)?;
    let info = check_mp4_video(&bytes).map_err(to_failure)?;
    Ok(VideoTranscodeOutput {
        container: VideoContainer::Mp4,
        // 画面へ出す文字列は英数字だけ通す（出力由来の値をそのまま表示しない）。
        codec: info
            .codec
            .iter()
            .filter(|b| b.is_ascii_alphanumeric())
            .map(|b| char::from(*b))
            .collect(),
        source_width: SOURCE_WIDTH,
        source_height: SOURCE_HEIGHT,
        source_ms: source_duration().as_millis() as u64,
        output_width: info.width,
        output_height: info.height,
        output_ms: info.length().as_millis() as u64,
        output_bytes: bytes.len() as u64,
    })
}

/// 1 フレームぶんの処理。要求の発送と期限判定を行う。
fn drive(
    state: &VideoTranscodeState,
    sender: &CompletionSender<VideoTranscodeState>,
    imp: &Arc<dyn VideoTranscode>,
    session: &Arc<Session>,
    deadline: Duration,
) {
    // 要求の観測と記録は同じロックの内側で行い、二重発送を防ぐ。
    let new_request = {
        let mut g = session.timing.lock().unwrap_or_else(|e| e.into_inner());
        match take_request(state) {
            Some(generation) if session.slot.snapshot().is_none() => {
                let started = Instant::now();
                let request_id = session.slot.begin();
                *g = Some((request_id, generation, started));
                Some((request_id, generation, started))
            }
            _ => {
                // 期限判定: 処理中でセッションの世代が一致するときだけ。
                if let (Some((request_id, gen, started)), VideoTranscodePhase::Running) =
                    (*g, &state.phase)
                {
                    if gen == state.generation {
                        let elapsed = started.elapsed();
                        if elapsed >= deadline {
                            *g = None;
                            session.slot.clear(request_id);
                            let _ = sender.send(Box::new(move |s: &mut VideoTranscodeState| {
                                finish_transcode(s, gen, Err(VideoTranscodeFailure::Timeout));
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
    let Some((request_id, generation, started)) = new_request else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    sender.request_repaint_after(deadline);

    let worker_sender = sender.clone();
    let worker_imp = imp.clone();
    let worker_session = session.clone();
    let spawned = std::thread::Builder::new()
        .name("video-transcode".into())
        .spawn(move || {
            let result = run_worker(&*worker_imp, started, deadline);
            let _ = worker_sender.send(Box::new(move |s: &mut VideoTranscodeState| {
                // 完了キューの適用が遅れた場合も期限を守る（UI スレッド停止後の適用）。
                let result = match result {
                    Ok(_) if started.elapsed() > deadline => Err(VideoTranscodeFailure::Timeout),
                    r => r,
                };
                finish_transcode(s, generation, result);
                worker_session.slot.clear(request_id);
            }));
        });
    if spawned.is_err() {
        let session = session.clone();
        let _ = sender.send(Box::new(move |s: &mut VideoTranscodeState| {
            // `mark_running` の後に適用されるよう同じキューへ積む。
            finish_transcode(
                s,
                generation,
                Err(VideoTranscodeFailure::Failed(
                    GENERIC_FAILED_MESSAGE.to_string(),
                )),
            );
            session.slot.clear(request_id);
        }));
    }
}

/// `registry.rs` の `effect` 区画から `video_transcode` 画面用に配線される effect ドライバ。
pub fn video_transcode_effect(
    state: &VideoTranscodeState,
    sender: &CompletionSender<VideoTranscodeState>,
) {
    static IMPL: std::sync::OnceLock<Arc<dyn VideoTranscode>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(|| Arc::new(Session::default())),
        TRANSCODE_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::video_transcode::{error_text, request_transcode};
    use fandhe_device::video_transcode::{minimal_mp4_video, UnsupportedVideoTranscode};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<VideoTranscodeState>>;

    fn channel() -> (CompletionSender<VideoTranscodeState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 元動画の半分の寸法・同じ長さの最小の MP4。
    fn good() -> Vec<u8> {
        minimal_mp4_video(b"avc1", 640, 360, 15_000, 30_000, 30, false)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Result<Vec<u8>, DeviceError>,
        sleep: Duration,
    }
    impl VideoTranscode for Fake {
        fn transcode(&self, _: Duration) -> Result<Vec<u8>, DeviceError> {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<Vec<u8>, DeviceError>) -> Arc<dyn VideoTranscode> {
        Arc::new(Fake {
            result,
            sleep: Duration::ZERO,
        })
    }

    /// 要求を立てて 1 フレーム回し、届いた完了を `n` 件適用した状態を返す。
    fn run(imp: Arc<dyn VideoTranscode>, n: usize) -> VideoTranscodeState {
        let (sender, rx) = channel();
        let session = Arc::new(Session::default());
        let mut state = VideoTranscodeState::default();
        request_transcode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        for _ in 0..n {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了が届く");
            c(&mut state);
        }
        state
    }

    fn failed(s: &VideoTranscodeState) -> &VideoTranscodeFailure {
        match &s.phase {
            VideoTranscodePhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        drive(
            &VideoTranscodeState::default(),
            &sender,
            &fake(|| Ok(good())),
            &Arc::new(Session::default()),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_fills_dimensions_and_durations() {
        let s = run(fake(|| Ok(good())), 2);
        match s.phase {
            VideoTranscodePhase::Done(o) => {
                assert_eq!(o.container, VideoContainer::Mp4);
                assert_eq!(o.codec, "avc1");
                assert_eq!((o.source_width, o.source_height), (1280, 720));
                assert_eq!((o.output_width, o.output_height), (640, 360));
                assert_eq!((o.source_ms, o.output_ms), (2000, 2000));
                assert_eq!(o.output_bytes, good().len() as u64);
            }
            p => panic!("成功のはず: {p:?}"),
        }
    }

    #[test]
    fn codec_text_keeps_only_alphanumerics() {
        let m = minimal_mp4_video(b"a\x00<b", 640, 360, 15_000, 30_000, 30, false);
        let r = run_worker(
            &Fake {
                result: || Ok(Vec::new()),
                sleep: Duration::ZERO,
            },
            Instant::now(),
            Duration::from_secs(30),
        );
        assert_eq!(r, Err(VideoTranscodeFailure::Empty));
        struct Once(Vec<u8>);
        impl VideoTranscode for Once {
            fn transcode(&self, _: Duration) -> Result<Vec<u8>, DeviceError> {
                Ok(self.0.clone())
            }
        }
        let o = run_worker(&Once(m), Instant::now(), Duration::from_secs(30)).unwrap();
        assert_eq!(o.codec, "ab");
    }

    #[test]
    fn failures_map_to_their_kinds() {
        let s = run(fake(|| Ok(Vec::new())), 2);
        assert_eq!(failed(&s), &VideoTranscodeFailure::Empty);
        let s = run(fake(|| Ok(b"garbage".to_vec())), 2);
        assert_eq!(failed(&s), &VideoTranscodeFailure::Unreadable);
        let s = run(
            fake(|| {
                Ok(minimal_mp4_video(
                    b"avc1", 1280, 720, 15_000, 30_000, 30, false,
                ))
            }),
            2,
        );
        assert_eq!(failed(&s), &VideoTranscodeFailure::NotSmaller);
        let s = run(
            fake(|| Ok(minimal_mp4_video(b"avc1", 640, 360, 1000, 500, 30, false))),
            2,
        );
        assert_eq!(failed(&s), &VideoTranscodeFailure::Mismatch);
        let s = run(Arc::new(UnsupportedVideoTranscode), 2);
        assert_eq!(failed(&s), &VideoTranscodeFailure::Unsupported);
        let s = run(fake(|| Err(DeviceError::Internal(TIMEOUT))), 2);
        assert_eq!(failed(&s), &VideoTranscodeFailure::Timeout);
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))), 2);
        let text = error_text(&s);
        assert!(!text.contains("secret"), "{text}");
        assert!(matches!(failed(&s), VideoTranscodeFailure::Failed(_)));
    }

    #[test]
    fn release_pending_is_a_fixed_message_failure() {
        let s = run(fake(|| Err(DeviceError::Internal(RELEASE_PENDING))), 2);
        let text = error_text(&s);
        assert!(text.contains(RELEASE_PENDING_MESSAGE), "{text}");
        assert!(!text.contains("video_transcode"), "{text}");
    }

    #[test]
    fn worker_past_deadline_is_timeout_even_with_good_output() {
        let imp: Arc<dyn VideoTranscode> = Arc::new(Fake {
            result: || Ok(good()),
            sleep: Duration::from_millis(60),
        });
        let (sender, rx) = channel();
        let mut state = VideoTranscodeState::default();
        request_transcode(&mut state);
        drive(
            &state,
            &sender,
            &imp,
            &Arc::new(Session::default()),
            Duration::from_millis(20),
        );
        for _ in 0..2 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        }
        assert_eq!(failed(&state), &VideoTranscodeFailure::Timeout);
    }

    #[test]
    fn late_apply_past_deadline_is_timeout() {
        let (sender, rx) = channel();
        let mut state = VideoTranscodeState::default();
        request_transcode(&mut state);
        drive(
            &state,
            &sender,
            &fake(|| Ok(good())),
            &Arc::new(Session::default()),
            Duration::from_millis(100),
        );
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        first(&mut state);
        second(&mut state);
        assert_eq!(failed(&state), &VideoTranscodeFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn VideoTranscode> = Arc::new(Fake {
            result: || Ok(good()),
            sleep: Duration::from_millis(300),
        });
        let (sender, rx) = channel();
        let session = Arc::new(Session::default());
        let mut state = VideoTranscodeState::default();
        request_transcode(&mut state);
        let deadline = Duration::from_millis(30);
        drive(&state, &sender, &imp, &session, deadline);
        // mark_running を適用
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(60));
        drive(&state, &sender, &imp, &session, deadline);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &VideoTranscodeFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failed(&state), &VideoTranscodeFailure::Timeout);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok(good()));
        let session = Arc::new(Session::default());
        let mut state = VideoTranscodeState::default();
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
        let session = Arc::new(Session::default());
        session.slot.begin();
        let mut state = VideoTranscodeState::default();
        request_transcode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.reset();
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn reset_then_stale_clear_does_not_consume_new_request() {
        let (sender, _rx) = channel();
        let imp = fake(|| Ok(good()));
        let session = Arc::new(Session::default());
        let old_id = session.slot.begin();
        session.reset();
        let mut state = VideoTranscodeState::default();
        request_transcode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        // 古い要求の後始末が、reset 後の新しい要求の記録を消さない。
        session.slot.clear(old_id);
        assert!(session.slot.snapshot().is_some());
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
            platform().transcode(Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }

    /// テスト専用の一時ディレクトリ（プロセス・連番で一意）。
    fn scratch(tag: &str) -> PathBuf {
        let (p, _) = temp_pair();
        let dir = p.with_file_name(format!("fandhe-vt-test-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn leftovers_missing_file_is_success_and_unremovable_is_tracked() {
        let dir = scratch("leftovers");
        let l = Leftovers::new();
        // 存在しないファイルは成功扱い
        l.release(&[&dir.join("none.mp4")]);
        assert!(l.ensure_released().is_ok());
        // 消せないもの（空でないディレクトリ）は記録され、ensure_released が Err を返す
        let stuck = dir.join("stuck.mp4");
        std::fs::create_dir_all(stuck.join("inner")).unwrap();
        l.release(&[&stuck]);
        assert!(matches!(
            l.ensure_released(),
            Err(DeviceError::Internal(RELEASE_PENDING))
        ));
        // 解消すれば通る
        std::fs::remove_dir_all(&stuck).unwrap();
        assert!(l.ensure_released().is_ok());
        // 実在するファイルは消える
        let f = dir.join("a.mp4");
        std::fs::write(&f, b"x").unwrap();
        l.release(&[&f]);
        assert!(!f.exists());
        assert!(l.ensure_released().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_pair_is_unique_and_internal_only() {
        let (a, b) = temp_pair();
        let (c, _) = temp_pair();
        assert_ne!(a, c);
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("fandhe-video-transcode-"), "{name}");
    }
}
