//! Phase 11（端末機能の拡充ループ、親 #182）型サンプル「ファイル・動画のプレビューとサムネイル」
//! （Issue #917）の effect ドライバ。`fandhe_core::screens::file_preview_show`（端末依存ゼロ）の
//! 要求を観測し、サムネイルはワーカースレッドで `fandhe_device::file_preview::FileThumbnailer` を、
//! プレビューは各 OS の `present` を呼ぶ（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・
//! 期限・守る範囲）の SSOT は `fandhe_device::file_preview`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は対象外。
//!
//! # OS 別の分担（dispatch は #917 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: サムネイルは [`super::apple_file_preview::AppleFileThumbnailer`]、プレビューは
//!   [`super::macos_file_preview::present`]（#917）
//! - iOS: `ios_file_preview`（b-ios。サムネイルは macOS と共用の `AppleFileThumbnailer`）
//! - Windows: `windows_file_preview`（windows の子。サムネイルのみ）
//! - Android: `android_file_preview`（b-android）
//! - それ以外（Linux）: `UnsupportedFileThumbnailer` / `Unsupported` 固定（ダミーの成功実装へ倒さない）
//!
//! # 要求の追跡と期限（契約表 2・3）
//!
//! サムネイルとプレビューは同じ [`Session`]（`RequestSlot` 1 本）を共用する（画面は 1 回に 1 操作）。
//! 結果はワーカー（サムネイル）か OS コールバック（プレビュー、[`bridge::deliver`]）が `slot` に置き、
//! 毎フレーム `poll_and_finish` が画面へ反映する。期限はサムネイルだけで、起点は要求を観測した
//! `Instant`（単調時計）。期限が来て結果が未着なら `clear_if_pending` が真のときだけ `Timeout` を
//! 確定し、遅れて届いた結果は `slot` の `request_id` 照合で捨てられる。プレビューに期限は無い。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::file_preview_show::{
    finish, mark_working, take_request, FilePreviewFailure, FilePreviewOp, FilePreviewOutcome,
    FilePreviewPhase, FilePreviewState,
};
use fandhe_core::RgbaPixels;
use fandhe_device::file_preview::{
    check_thumbnail, FileKind, FileThumbnailer, PreviewResult, ThumbnailResult, THUMBNAIL_DEADLINE,
    THUMBNAIL_MAX_SIDE, TIMEOUT, UNKNOWN_BYTES,
};
use fandhe_device::DeviceError;

use super::request_slot::{poll_and_finish, RequestSlot};
use crate::app::CompletionSender;

type Outcome = Result<FilePreviewOutcome, FilePreviewFailure>;

/// プレビュー提示の関数型。`Ok(None)` は OS が受け付けた（結果は後で [`bridge::deliver`] に届く）、
/// `Ok(Some(r))` は同期で確定した、`Err` は失敗。
type Presenter = fn(u64, FileKind, Vec<u8>) -> Result<Option<PreviewResult>, DeviceError>;

/// OS へ渡す固定ファイルのバイト列を作る唯一の場所（固定ファイル以外を渡さない）。
pub(crate) fn sample_bytes(kind: FileKind) -> Vec<u8> {
    match kind {
        FileKind::Unknown => UNKNOWN_BYTES.to_vec(),
        FileKind::SampleImage => {
            let img = fandhe_device::file_preview::sample_image();
            let mut out = Vec::new();
            // 固定サイズの RGBA を PNG にするだけで失敗しない。万一失敗しても空のバイト列が
            // OS へ渡り、サムネイルなし・失敗になるだけで成功には化けない。
            if let Some(buf) = image::RgbaImage::from_raw(img.width, img.height, img.rgba) {
                let _ = buf.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
            }
            out
        }
    }
}

/// 現在のプラットフォームのサムネイル実装を返す（dispatch の最終形）。
fn thumbnailer() -> Arc<dyn FileThumbnailer> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_file_preview::AppleFileThumbnailer)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_file_preview::thumbnailer()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_file_preview::thumbnailer()
    }
    #[cfg(target_os = "android")]
    {
        super::android_file_preview::thumbnailer()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        Arc::new(fandhe_device::file_preview::UnsupportedFileThumbnailer)
    }
}

/// 現在のプラットフォームのプレビュー提示を呼ぶ（dispatch の最終形）。
fn present_preview(
    request_id: u64,
    kind: FileKind,
    bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, DeviceError> {
    #[cfg(target_os = "macos")]
    {
        super::macos_file_preview::present(request_id, kind, &bytes)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_file_preview::present_preview(request_id, kind, bytes)
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_file_preview::present_preview(request_id, kind, bytes)
    }
    #[cfg(target_os = "android")]
    {
        super::android_file_preview::present_preview(request_id, kind, bytes)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        let _ = (request_id, kind, bytes);
        Err(DeviceError::Unsupported("file_preview"))
    }
}

/// 進行中の要求の期限管理用の記録。
#[derive(Clone, Copy)]
struct Timing {
    request_id: u64,
    generation: u64,
    started: Instant,
    /// サムネイルのときだけ真（プレビューに期限は無い）。
    has_deadline: bool,
}

/// ドライバの可変状態。テストが干渉しないよう static から切り出し、引数で渡す。
pub(crate) struct Session {
    slot: RequestSlot<Outcome>,
    timing: Mutex<Option<Timing>>,
}

impl Session {
    pub(crate) const fn new() -> Self {
        Self {
            slot: RequestSlot::new(),
            timing: Mutex::new(None),
        }
    }

    fn timing(&self) -> Option<Timing> {
        *self.timing.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_timing(&self, t: Option<Timing>) {
        *self.timing.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }

    fn reset(&self) {
        self.slot.reset();
        self.set_timing(None);
    }
}

static SESSION: Session = Session::new();

/// Activity 再生成時（`android_main` の再実行）に、ドライバの処理済み記録を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、完了後も残る記録を残すと次の操作が発送されず
/// 飲み込まれる。古いワーカーの結果は `request_id` 照合で捨てられる。走っている OS 呼び出しは
/// 止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    SESSION.reset();
}

/// OS コールバックの受け口（macOS のプレビュー窓が閉じた通知など）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) mod bridge {
    use fandhe_device::file_preview::PreviewResult;
    use fandhe_device::DeviceError;

    /// OS からのプレビュー結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（古い世代・二重通知が成功へ化けない）。
    pub(crate) fn deliver(request_id: u64, result: Result<PreviewResult, DeviceError>) -> bool {
        super::SESSION
            .slot
            .deliver(request_id, super::preview_outcome(result))
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コードは画面にもログにも出さない
/// （security.md A09）。`Cancelled`・`PermissionDenied`・`InvalidInput` も成功にはしない。
pub(crate) fn to_failure(e: DeviceError) -> FilePreviewFailure {
    match e {
        DeviceError::Unsupported(_) => FilePreviewFailure::Unsupported,
        DeviceError::Internal(TIMEOUT) => FilePreviewFailure::Timeout,
        _ => FilePreviewFailure::Failed,
    }
}

fn preview_outcome(r: Result<PreviewResult, DeviceError>) -> Outcome {
    r.map(|p| match p {
        PreviewResult::Closed => FilePreviewOutcome::Closed,
        PreviewResult::HandedOff => FilePreviewOutcome::HandedOff,
        PreviewResult::NoHandler => FilePreviewOutcome::NoHandler,
        PreviewResult::TargetUnavailable => FilePreviewOutcome::TargetUnavailable,
    })
    .map_err(to_failure)
}

/// サムネイルの結果を画面用へ畳む。期限後に届いた結果は中身を見ずに `Timeout` にし、成功は
/// [`check_thumbnail`] で再検査する。
fn thumbnail_outcome(
    r: Result<ThumbnailResult, DeviceError>,
    started: Instant,
    deadline: Duration,
) -> Outcome {
    if started.elapsed() > deadline {
        return Err(FilePreviewFailure::Timeout);
    }
    match r.map_err(to_failure)? {
        ThumbnailResult::NoThumbnail => Ok(FilePreviewOutcome::NoThumbnail),
        ThumbnailResult::Thumbnail(i) => {
            match check_thumbnail(i.width, i.height, i.rgba).map_err(to_failure)? {
                ThumbnailResult::Thumbnail(i) => RgbaPixels::new(i.width, i.height, i.rgba)
                    .map(FilePreviewOutcome::Thumbnail)
                    .ok_or(FilePreviewFailure::Failed),
                ThumbnailResult::NoThumbnail => Err(FilePreviewFailure::Failed),
            }
        }
    }
}

fn kind_of(op: FilePreviewOp) -> FileKind {
    match op {
        FilePreviewOp::ThumbnailUnknown => FileKind::Unknown,
        _ => FileKind::SampleImage,
    }
}

/// 要求を OS へ発送する。サムネイルはワーカー、プレビューはその場で提示を依頼する。
fn start(
    session: &'static Session,
    sender: &CompletionSender<FilePreviewState>,
    imp: &Arc<dyn FileThumbnailer>,
    present: Presenter,
    (request_id, op, started): (u64, FilePreviewOp, Instant),
    deadline: Duration,
) {
    let kind = kind_of(op);
    let bytes = sample_bytes(kind);
    if op == FilePreviewOp::Preview {
        match present(request_id, kind, bytes) {
            Ok(None) => {}
            Ok(Some(r)) => {
                session.slot.deliver(request_id, preview_outcome(Ok(r)));
            }
            Err(e) => {
                session.slot.deliver(request_id, Err(to_failure(e)));
            }
        }
        return;
    }
    let worker_imp = imp.clone();
    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("file-preview".into())
        .spawn(move || {
            let remaining = deadline.saturating_sub(started.elapsed());
            let r = worker_imp.thumbnail(kind, &bytes, THUMBNAIL_MAX_SIDE, remaining);
            session
                .slot
                .deliver(request_id, thumbnail_outcome(r, started, deadline));
            worker_sender.request_repaint_after(Duration::ZERO);
        });
    if spawned.is_err() {
        session
            .slot
            .deliver(request_id, Err(FilePreviewFailure::Failed));
    }
}

/// 1 フレームぶんの処理。要求の発送・期限判定・結果の反映を行う。
fn drive(
    state: &FilePreviewState,
    sender: &CompletionSender<FilePreviewState>,
    session: &'static Session,
    imp: &Arc<dyn FileThumbnailer>,
    present: Presenter,
    deadline: Duration,
) {
    // 要求の観測と記録は同じロックの内側で行い、二重発送を防ぐ。
    let new_request = {
        let mut g = session.timing.lock().unwrap_or_else(|e| e.into_inner());
        match take_request(state) {
            Some((generation, op)) if session.slot.snapshot().is_none() => {
                let started = Instant::now();
                let request_id = session.slot.begin();
                *g = Some(Timing {
                    request_id,
                    generation,
                    started,
                    has_deadline: op != FilePreviewOp::Preview,
                });
                Some((request_id, op, started))
            }
            _ => None,
        }
    };
    if let Some(req) = new_request {
        let _ = sender.send(Box::new(mark_working));
        start(session, sender, imp, present, req, deadline);
        sender.request_repaint_after(Duration::ZERO);
        return;
    }
    if !matches!(state.phase, FilePreviewPhase::Working(_)) {
        return;
    }
    let generation = state.generation;
    if let Some(t) = session.timing() {
        if t.has_deadline && t.generation == generation {
            let elapsed = t.started.elapsed();
            if elapsed >= deadline {
                // 結果が未着のときだけ Timeout を確定する。着地済みなら後続の poll に任せる。
                if session.slot.clear_if_pending(t.request_id) {
                    session.set_timing(None);
                    let _ = sender.send(Box::new(move |s: &mut FilePreviewState| {
                        finish(s, generation, Err(FilePreviewFailure::Timeout));
                    }));
                    return;
                }
            } else {
                sender.request_repaint_after(deadline - elapsed);
            }
        }
    }
    poll_and_finish(
        &session.slot,
        sender,
        |_| {},
        move |s: &mut FilePreviewState, outcome| {
            // `finish` が `Idle`・世代違いを捨てる。結果は常に消費し、記録を残さない。
            finish(s, generation, outcome);
            session.set_timing(None);
            true
        },
    );
}

/// `registry.rs` の `effect` 区画から `file_preview` 画面用に配線される effect ドライバ。
pub fn file_preview_effect(state: &FilePreviewState, sender: &CompletionSender<FilePreviewState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn FileThumbnailer>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(thumbnailer);
    drive(
        state,
        sender,
        &SESSION,
        imp,
        present_preview,
        THUMBNAIL_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::file_preview_show::{error_text, request};
    use fandhe_device::file_preview::UnsupportedFileThumbnailer;
    use fandhe_device::Image;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<FilePreviewState>>;

    fn channel() -> (CompletionSender<FilePreviewState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// テストごとに独立した Session（static は共有されるので leak して使う）。
    fn session() -> &'static Session {
        Box::leak(Box::new(Session::new()))
    }

    fn thumb(w: u32, h: u32, len: usize) -> Result<ThumbnailResult, DeviceError> {
        Ok(ThumbnailResult::Thumbnail(Image {
            width: w,
            height: h,
            rgba: vec![255; len],
        }))
    }

    struct Fake {
        result: fn() -> Result<ThumbnailResult, DeviceError>,
        sleep: Duration,
    }
    impl FileThumbnailer for Fake {
        fn thumbnail(
            &self,
            _: FileKind,
            _: &[u8],
            _: u32,
            _: Duration,
        ) -> Result<ThumbnailResult, DeviceError> {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<ThumbnailResult, DeviceError>) -> Arc<dyn FileThumbnailer> {
        Arc::new(Fake {
            result,
            sleep: Duration::ZERO,
        })
    }

    fn no_preview(_: u64, _: FileKind, _: Vec<u8>) -> Result<Option<PreviewResult>, DeviceError> {
        Err(DeviceError::Unsupported("file_preview"))
    }

    fn target_unavailable(
        _: u64,
        _: FileKind,
        _: Vec<u8>,
    ) -> Result<Option<PreviewResult>, DeviceError> {
        Ok(Some(PreviewResult::TargetUnavailable))
    }

    /// 要求を立てて 1 フレーム回し、`mark_working` を適用してから、結果が反映されるまで回す。
    fn run(
        imp: Arc<dyn FileThumbnailer>,
        present: Presenter,
        op: FilePreviewOp,
    ) -> FilePreviewState {
        let (sender, rx) = channel();
        let session = session();
        let mut state = FilePreviewState::default();
        request(&mut state, op);
        drive(
            &state,
            &sender,
            session,
            &imp,
            present,
            Duration::from_secs(30),
        );
        for _ in 0..200 {
            while let Ok(c) = rx.try_recv() {
                c(&mut state);
            }
            if state.outcome.is_some() {
                return state;
            }
            std::thread::sleep(Duration::from_millis(10));
            drive(
                &state,
                &sender,
                session,
                &imp,
                present,
                Duration::from_secs(30),
            );
        }
        panic!("結果が反映されない: {state:?}");
    }

    fn failure(s: &FilePreviewState) -> FilePreviewFailure {
        match &s.outcome {
            Some(Err(f)) => *f,
            o => panic!("失敗のはず: {o:?}"),
        }
    }

    #[test]
    fn thumbnail_success_fills_image() {
        let s = run(
            fake(|| thumb(2, 3, 24)),
            no_preview,
            FilePreviewOp::Thumbnail,
        );
        assert!(matches!(
            s.outcome,
            Some(Ok(FilePreviewOutcome::Thumbnail(_)))
        ));
    }

    #[test]
    fn invalid_dimensions_never_succeed() {
        for r in [
            (|| thumb(0, 3, 0)) as fn() -> _,
            || thumb(2, 3, 23),
            || thumb(257, 1, 257 * 4),
        ] {
            let s = run(fake(r), no_preview, FilePreviewOp::Thumbnail);
            assert_eq!(failure(&s), FilePreviewFailure::Failed);
        }
    }

    #[test]
    fn no_thumbnail_is_not_a_failure_nor_a_thumbnail() {
        let s = run(
            fake(|| Ok(ThumbnailResult::NoThumbnail)),
            no_preview,
            FilePreviewOp::ThumbnailUnknown,
        );
        assert_eq!(s.outcome, Some(Ok(FilePreviewOutcome::NoThumbnail)));
    }

    #[test]
    fn os_errors_never_become_success() {
        for e in [
            || DeviceError::Cancelled("x"),
            || DeviceError::PermissionDenied("x"),
            || DeviceError::InvalidInput("x"),
            || DeviceError::Internal("secret os detail"),
        ] {
            let imp: Arc<dyn FileThumbnailer> = Arc::new(ErrFake(e));
            let s = run(imp, no_preview, FilePreviewOp::Thumbnail);
            assert_eq!(failure(&s), FilePreviewFailure::Failed);
            assert!(!error_text(&s).contains("secret"));
        }
        let s = run(
            Arc::new(UnsupportedFileThumbnailer),
            no_preview,
            FilePreviewOp::Thumbnail,
        );
        assert_eq!(failure(&s), FilePreviewFailure::Unsupported);
        let s = run(
            fake(|| Err(DeviceError::Internal(TIMEOUT))),
            no_preview,
            FilePreviewOp::Thumbnail,
        );
        assert_eq!(failure(&s), FilePreviewFailure::Timeout);
    }

    struct ErrFake(fn() -> DeviceError);
    impl FileThumbnailer for ErrFake {
        fn thumbnail(
            &self,
            _: FileKind,
            _: &[u8],
            _: u32,
            _: Duration,
        ) -> Result<ThumbnailResult, DeviceError> {
            Err((self.0)())
        }
    }

    #[test]
    fn worker_past_deadline_is_timeout_even_with_good_output() {
        let imp: Arc<dyn FileThumbnailer> = Arc::new(Fake {
            result: || thumb(2, 3, 24),
            sleep: Duration::from_millis(80),
        });
        let (sender, rx) = channel();
        let session = session();
        let mut state = FilePreviewState::default();
        request(&mut state, FilePreviewOp::Thumbnail);
        let d = Duration::from_millis(20);
        drive(&state, &sender, session, &imp, no_preview, d);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(200));
        // フレームの期限判定が先でも、ワーカーの遅れた成功が先でも成功にはならない。
        drive(&state, &sender, session, &imp, no_preview, d);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        for _ in 0..5 {
            if state.outcome.is_some() {
                break;
            }
            drive(&state, &sender, session, &imp, no_preview, d);
            while let Ok(c) = rx.try_recv() {
                c(&mut state);
            }
        }
        assert_eq!(failure(&state), FilePreviewFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn FileThumbnailer> = Arc::new(Fake {
            result: || thumb(2, 3, 24),
            sleep: Duration::from_millis(300),
        });
        let (sender, rx) = channel();
        let session = session();
        let mut state = FilePreviewState::default();
        request(&mut state, FilePreviewOp::Thumbnail);
        let d = Duration::from_millis(30);
        drive(&state, &sender, session, &imp, no_preview, d);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        std::thread::sleep(Duration::from_millis(60));
        drive(&state, &sender, session, &imp, no_preview, d);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(failure(&state), FilePreviewFailure::Timeout);
        // 遅れて届いたワーカーの結果は slot の照合で捨てられ、結果は変わらない。
        std::thread::sleep(Duration::from_millis(400));
        drive(&state, &sender, session, &imp, no_preview, d);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(failure(&state), FilePreviewFailure::Timeout);
    }

    #[test]
    fn stale_and_duplicate_delivery_are_discarded() {
        let s = session();
        let id = s.slot.begin();
        assert!(!s
            .slot
            .deliver(id.wrapping_add(1), Ok(FilePreviewOutcome::Closed)));
        assert!(s.slot.deliver(id, Ok(FilePreviewOutcome::Closed)));
        assert!(!s.slot.deliver(id, Ok(FilePreviewOutcome::HandedOff)));
        let old = s.slot.begin();
        let _new = s.slot.begin();
        assert!(!s.slot.deliver(old, Ok(FilePreviewOutcome::Closed)));
    }

    #[test]
    fn double_drive_before_mark_applied_dispatches_once() {
        let (sender, rx) = channel();
        let imp = fake(|| thumb(2, 3, 24));
        let session = session();
        let mut state = FilePreviewState::default();
        request(&mut state, FilePreviewOp::Thumbnail);
        let d = Duration::from_secs(30);
        drive(&state, &sender, session, &imp, no_preview, d);
        drive(&state, &sender, session, &imp, no_preview, d);
        std::thread::sleep(Duration::from_millis(200));
        let mut marks = 0;
        while rx.try_recv().is_ok() {
            marks += 1;
        }
        // mark_working 1 件と、ワーカー終了時の repaint は完了キューを使わない。
        assert_eq!(marks, 1);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(|| thumb(2, 3, 24));
        let session = session();
        session.slot.begin();
        let mut state = FilePreviewState::default();
        request(&mut state, FilePreviewOp::Thumbnail);
        let d = Duration::from_secs(30);
        drive(&state, &sender, session, &imp, no_preview, d);
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.reset();
        drive(&state, &sender, session, &imp, no_preview, d);
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn synchronous_preview_result_is_reflected() {
        let s = run(
            Arc::new(UnsupportedFileThumbnailer),
            target_unavailable,
            FilePreviewOp::Preview,
        );
        assert_eq!(s.outcome, Some(Ok(FilePreviewOutcome::TargetUnavailable)));
    }

    #[test]
    fn preview_error_is_a_failure() {
        let s = run(
            Arc::new(UnsupportedFileThumbnailer),
            no_preview,
            FilePreviewOp::Preview,
        );
        assert_eq!(failure(&s), FilePreviewFailure::Unsupported);
    }

    #[test]
    fn sample_bytes_are_png_and_deterministic() {
        let a = sample_bytes(FileKind::SampleImage);
        assert!(a.starts_with(b"\x89PNG"));
        assert_eq!(a, sample_bytes(FileKind::SampleImage));
        assert_eq!(sample_bytes(FileKind::Unknown), UNKNOWN_BYTES.to_vec());
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "android"
        )))]
        {
            assert!(matches!(
                thumbnailer().thumbnail(
                    FileKind::SampleImage,
                    &[1],
                    THUMBNAIL_MAX_SIDE,
                    THUMBNAIL_DEADLINE
                ),
                Err(DeviceError::Unsupported(_))
            ));
            assert!(matches!(
                present_preview(1, FileKind::SampleImage, vec![1]),
                Err(DeviceError::Unsupported(_))
            ));
        }
    }

    /// サムネイルの長辺の上限が `RgbaPixels` の上限に収まる（崩れたらコンパイルが通らない）。
    const _: () = assert!(THUMBNAIL_MAX_SIDE <= fandhe_core::RGBA_MAX_SIDE);
}
