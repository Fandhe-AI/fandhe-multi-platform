//! Phase 11（端末機能の拡充ループ、親 #182）型サンプル「ファイル・動画のプレビューとサムネイル」
//! （Issue #917）の effect ドライバ。`fandhe_core::screens::file_preview_show`（端末依存ゼロ）の
//! 要求を観測し、サムネイルはワーカースレッドで `crate::contract::FileThumbnailer` を、
//! プレビューは各 OS の `present` を呼ぶ（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・
//! 期限・守る範囲）の SSOT は `crate::contract`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web は対象外。
//!
//! # OS 別の分担（dispatch は #917 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: サムネイルは [`crate::apple::AppleFileThumbnailer`]、プレビューは
//!   [`crate::macos::present`]（#917）
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

use crate::contract::{
    FileKind, FileThumbnailer, PreviewResult, THUMBNAIL_DEADLINE, THUMBNAIL_MAX_SIDE, TIMEOUT,
    ThumbnailResult, UNKNOWN_BYTES, check_thumbnail,
};
use crate::error::FilePreviewError;
use fandhe_core::RgbaPixels;
use fandhe_core::screens::file_preview_show::{
    FilePreviewFailure, FilePreviewOp, FilePreviewOutcome, FilePreviewPhase, FilePreviewState,
    finish, mark_working, take_request,
};

use fandhe_runtime::CompletionSender;
use fandhe_runtime::request_slot::{RequestSlot, SlotOwner, poll_and_finish_owned};

type Outcome = Result<FilePreviewOutcome, FilePreviewFailure>;

/// `RequestSlot` に置く結果。サムネイルは画面用に畳んだ値、プレビューは契約の生の値で置く
/// （公開入口 [`poll_file_preview`] は生の値を返し、画面は反映時に写像する）。照合・first-wins・保持は slot の 1 つのロックで決まる。
#[derive(Clone)]
enum Raw {
    Screen(Outcome),
    Preview(Result<PreviewResult, FilePreviewError>),
}

/// プレビュー提示の関数型。`Ok(None)` は OS が受け付けた（結果は後で [`bridge::deliver`] に届く）、
/// `Ok(Some(r))` は同期で確定した、`Err` は失敗。
type Presenter = fn(u64, FileKind, Vec<u8>) -> Result<Option<PreviewResult>, FilePreviewError>;

/// OS へ渡す固定ファイルのバイト列を作る唯一の場所（固定ファイル以外を渡さない）。
pub(crate) fn sample_bytes(kind: FileKind) -> Vec<u8> {
    match kind {
        FileKind::Unknown => UNKNOWN_BYTES.to_vec(),
        FileKind::SampleImage => {
            let img = crate::contract::sample_image();
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
///
/// 画面の状態型に依存しない入口で、フレームワークの外のアプリも契約を直接呼べる。
pub fn platform_file_preview() -> Arc<dyn FileThumbnailer> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(crate::apple::AppleFileThumbnailer)
    }
    #[cfg(target_os = "ios")]
    {
        crate::ios::thumbnailer()
    }
    #[cfg(target_os = "windows")]
    {
        crate::windows::thumbnailer()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::thumbnailer()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        Arc::new(crate::contract::UnsupportedFileThumbnailer)
    }
}

/// 固定ファイルのプレビューを OS へ依頼し、結果を [`poll_file_preview`] で受け取るための要求 id を返す。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。画面の effect と同じ要求の枠
/// （1 プロセスに 1 要求）を共用するため、進行中の要求があるうちは `Internal`（busy）で断る。
/// OS が同期で結果を確定した場合も、[`poll_file_preview`] で受け取る。依頼自体の失敗は `Err`（結果は届かない）。
/// サムネイルは同期の [`platform_file_preview`] を使う。
pub fn begin_file_preview(kind: FileKind) -> Result<u64, FilePreviewError> {
    // 画面の未取得の結果も含め、届いて未取得の結果があるうちは置き換えない。
    let request_id = SESSION
        .slot
        .try_begin_unless(|_| true)
        .ok_or(FilePreviewError::Internal("file preview busy"))?;
    match present_preview(request_id, kind, sample_bytes(kind)) {
        Ok(None) => Ok(request_id),
        Ok(Some(r)) => {
            SESSION.deliver_preview(request_id, Ok(r));
            Ok(request_id)
        }
        Err(e) => {
            SESSION.slot.clear(request_id);
            Err(e)
        }
    }
}

/// [`begin_file_preview`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・取り出し済みは `None`。
pub fn poll_file_preview(request_id: u64) -> Option<Result<PreviewResult, FilePreviewError>> {
    match SESSION.slot.take_ready(request_id)? {
        Raw::Preview(r) => Some(r),
        Raw::Screen(_) => Some(Err(FilePreviewError::Internal("file preview result"))),
    }
}

/// 現在のプラットフォームのプレビュー提示を呼ぶ（dispatch の最終形）。
fn present_preview(
    request_id: u64,
    kind: FileKind,
    bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, FilePreviewError> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::present(request_id, kind, &bytes)
    }
    #[cfg(target_os = "ios")]
    {
        crate::ios::present_preview(request_id, kind, bytes)
    }
    #[cfg(target_os = "windows")]
    {
        crate::windows::present_preview(request_id, kind, bytes)
    }
    #[cfg(target_os = "android")]
    {
        crate::android::present_preview(request_id, kind, bytes)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        let _ = (request_id, kind, bytes);
        Err(FilePreviewError::Unsupported)
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
    slot: RequestSlot<Raw>,
    /// 画面の effect が登録した要求の印。公開入口の要求を置き換えたり、その結果を取り出したりしないために使う。
    screen: SlotOwner,
    timing: Mutex<Option<Timing>>,
}

impl Session {
    pub(crate) const fn new() -> Self {
        Self {
            slot: RequestSlot::new(),
            screen: SlotOwner::new(),
            timing: Mutex::new(None),
        }
    }

    /// プレビューの結果を生値のまま置く。旧 id・二重通知は slot が捨てて `false`（先に届いた結果は変わらない）。
    fn deliver_preview(
        &self,
        request_id: u64,
        result: Result<PreviewResult, FilePreviewError>,
    ) -> bool {
        self.slot.deliver(request_id, Raw::Preview(result))
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
pub fn reset_pending_tracking() {
    SESSION.reset();
}

/// OS コールバックの受け口（macOS のプレビュー窓が閉じた通知など）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) mod bridge {
    use crate::contract::PreviewResult;
    use crate::error::FilePreviewError;

    /// OS からのプレビュー結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（古い世代・二重通知が成功へ化けない）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<PreviewResult, FilePreviewError>,
    ) -> bool {
        super::SESSION.deliver_preview(request_id, result)
    }
}

/// `FilePreviewError` を画面の失敗種別へ写す。内部識別子・OS コードは画面にもログにも出さない
/// （security.md A09）。`Cancelled`・`PermissionDenied`・`InvalidInput` も成功にはしない。
pub(crate) fn to_failure(e: FilePreviewError) -> FilePreviewFailure {
    match e {
        FilePreviewError::Unsupported => FilePreviewFailure::Unsupported,
        FilePreviewError::Internal(TIMEOUT) => FilePreviewFailure::Timeout,
        _ => FilePreviewFailure::Failed,
    }
}

fn preview_outcome(r: Result<PreviewResult, FilePreviewError>) -> Outcome {
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
    r: Result<ThumbnailResult, FilePreviewError>,
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
                session.deliver_preview(request_id, Ok(r));
            }
            Err(e) => {
                session.deliver_preview(request_id, Err(e));
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
            session.slot.deliver(
                request_id,
                Raw::Screen(thumbnail_outcome(r, started, deadline)),
            );
            worker_sender.request_repaint_after(Duration::ZERO);
        });
    if spawned.is_err() {
        session
            .slot
            .deliver(request_id, Raw::Screen(Err(FilePreviewFailure::Failed)));
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
            // 発送済みの世代は `mark_working` の適用まで再発送しない（先の結果も消さない）。
            // 公開入口の要求・結果が残っているうちは置き換えず、画面へ busy の失敗を返す。
            Some((generation, op)) if g.is_none_or(|t| t.generation != generation) => {
                Some(match session.screen.begin_keeping_foreign(&session.slot) {
                    Some(request_id) => {
                        let started = Instant::now();
                        *g = Some(Timing {
                            request_id,
                            generation,
                            started,
                            has_deadline: op != FilePreviewOp::Preview,
                        });
                        Ok((request_id, op, started))
                    }
                    None => Err(generation),
                })
            }
            _ => None,
        }
    };
    match new_request {
        Some(Ok(req)) => {
            let _ = sender.send(Box::new(mark_working));
            start(session, sender, imp, present, req, deadline);
            sender.request_repaint_after(Duration::ZERO);
            return;
        }
        Some(Err(generation)) => {
            // 枠が埋まっていると画面は `Requested` のまま再描画されなくなるため、失敗を確定して戻す。
            let _ = sender.send(Box::new(move |s: &mut FilePreviewState| {
                finish(s, generation, Err(FilePreviewFailure::Failed));
            }));
            return;
        }
        None => {}
    }
    if !matches!(state.phase, FilePreviewPhase::Working(_)) {
        return;
    }
    let generation = state.generation;
    if let Some(t) = session.timing()
        && t.has_deadline
        && t.generation == generation
    {
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
    poll_and_finish_owned(
        &session.slot,
        &session.screen,
        sender,
        |_| {},
        move |s: &mut FilePreviewState, raw| {
            let outcome = match raw {
                Raw::Screen(o) => o,
                Raw::Preview(r) => preview_outcome(r),
            };
            // `finish` が `Idle`・世代違いを捨てる。結果は常に消費し、記録を残さない。
            finish(s, generation, outcome);
            session.set_timing(None);
            true
        },
        // 自分の要求が失われた（Activity 再生成の `reset` 等）。失敗を確定して画面を処理中のまま残さない。
        move |s: &mut FilePreviewState| {
            finish(s, generation, Err(FilePreviewFailure::Failed));
            session.set_timing(None);
        },
    );
}

/// `registry.rs` の `effect` 区画から `file_preview` 画面用に配線される effect ドライバ。
pub fn file_preview_effect(state: &FilePreviewState, sender: &CompletionSender<FilePreviewState>) {
    static IMPL: std::sync::OnceLock<Arc<dyn FileThumbnailer>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform_file_preview);
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
    use crate::contract::UnsupportedFileThumbnailer;
    use fandhe_core::screens::file_preview_show::{error_text, request};
    use fandhe_device::Image;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<fandhe_runtime::Completion<FilePreviewState>>;

    fn channel() -> (CompletionSender<FilePreviewState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    /// テストごとに独立した Session（static は共有されるので leak して使う）。
    fn session() -> &'static Session {
        Box::leak(Box::new(Session::new()))
    }

    fn thumb(w: u32, h: u32, len: usize) -> Result<ThumbnailResult, FilePreviewError> {
        Ok(ThumbnailResult::Thumbnail(Image {
            width: w,
            height: h,
            rgba: vec![255; len],
        }))
    }

    /// 起床しないだけの口（受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    struct Fake {
        result: fn() -> Result<ThumbnailResult, FilePreviewError>,
        sleep: Duration,
    }
    impl FileThumbnailer for Fake {
        fn thumbnail(
            &self,
            _: FileKind,
            _: &[u8],
            _: u32,
            _: Duration,
        ) -> Result<ThumbnailResult, FilePreviewError> {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<ThumbnailResult, FilePreviewError>) -> Arc<dyn FileThumbnailer> {
        Arc::new(Fake {
            result,
            sleep: Duration::ZERO,
        })
    }

    fn no_preview(
        _: u64,
        _: FileKind,
        _: Vec<u8>,
    ) -> Result<Option<PreviewResult>, FilePreviewError> {
        Err(FilePreviewError::Unsupported)
    }

    fn target_unavailable(
        _: u64,
        _: FileKind,
        _: Vec<u8>,
    ) -> Result<Option<PreviewResult>, FilePreviewError> {
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

    /// `mark_working` の適用前に `drive` が再び呼ばれても、同じ世代を再発送せず先の結果も消さない。
    #[test]
    fn same_generation_is_not_redispatched_before_mark_working() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn counting(
            _: u64,
            _: FileKind,
            _: Vec<u8>,
        ) -> Result<Option<PreviewResult>, FilePreviewError> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Some(PreviewResult::TargetUnavailable))
        }
        let (sender, rx) = channel();
        let session = session();
        let imp = fake(|| thumb(2, 3, 24));
        let mut state = FilePreviewState::default();
        request(&mut state, FilePreviewOp::Preview);
        let d = Duration::from_secs(30);
        drive(&state, &sender, session, &imp, counting, d);
        drive(&state, &sender, session, &imp, counting, d);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        drive(&state, &sender, session, &imp, counting, d);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(
            state.outcome,
            Some(Ok(FilePreviewOutcome::TargetUnavailable))
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    /// 公開入口が枠を使用中に画面から要求しても、`Requested` のまま止まらず busy の失敗が画面へ届く。
    #[test]
    fn busy_slot_returns_failure_to_screen() {
        let (sender, rx) = channel();
        let session = session();
        let imp = fake(|| thumb(2, 3, 24));
        let busy = session.slot.try_begin_unless(|_| true).expect("枠を占有");
        let mut state = FilePreviewState::default();
        request(&mut state, FilePreviewOp::Preview);
        drive(
            &state,
            &sender,
            session,
            &imp,
            no_preview,
            Duration::from_secs(30),
        );
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(failure(&state), FilePreviewFailure::Failed);
        assert!(session.timing().is_none());
        session.slot.clear(busy);
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
            || FilePreviewError::Cancelled("x"),
            || FilePreviewError::PermissionDenied("x"),
            || FilePreviewError::InvalidInput("x"),
            || FilePreviewError::Internal("secret os detail"),
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
            fake(|| Err(FilePreviewError::Internal(TIMEOUT))),
            no_preview,
            FilePreviewOp::Thumbnail,
        );
        assert_eq!(failure(&s), FilePreviewFailure::Timeout);
    }

    struct ErrFake(fn() -> FilePreviewError);
    impl FileThumbnailer for ErrFake {
        fn thumbnail(
            &self,
            _: FileKind,
            _: &[u8],
            _: u32,
            _: Duration,
        ) -> Result<ThumbnailResult, FilePreviewError> {
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
        assert!(!s.slot.deliver(
            id.wrapping_add(1),
            Raw::Screen(Ok(FilePreviewOutcome::Closed))
        ));
        assert!(
            s.slot
                .deliver(id, Raw::Screen(Ok(FilePreviewOutcome::Closed)))
        );
        assert!(
            !s.slot
                .deliver(id, Raw::Screen(Ok(FilePreviewOutcome::HandedOff)))
        );
        let old = s.slot.begin();
        let _new = s.slot.begin();
        assert!(
            !s.slot
                .deliver(old, Raw::Screen(Ok(FilePreviewOutcome::Closed)))
        );
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
        // 記録が残っていると発送されず、busy の失敗が画面へ返る。
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(failure(&state), FilePreviewFailure::Failed);
        session.reset();
        request(&mut state, FilePreviewOp::Thumbnail);
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
                platform_file_preview().thumbnail(
                    FileKind::SampleImage,
                    &[1],
                    THUMBNAIL_MAX_SIDE,
                    THUMBNAIL_DEADLINE
                ),
                Err(FilePreviewError::Unsupported)
            ));
            assert!(matches!(
                present_preview(1, FileKind::SampleImage, vec![1]),
                Err(FilePreviewError::Unsupported)
            ));
        }
    }

    /// サムネイルの長辺の上限が `RgbaPixels` の上限に収まる（崩れたらコンパイルが通らない）。
    const _: () = assert!(THUMBNAIL_MAX_SIDE <= fandhe_core::RGBA_MAX_SIDE);

    #[test]
    fn public_poll_returns_raw_result_once() {
        let id = SESSION.slot.begin();
        assert_eq!(poll_file_preview(id), None);
        assert!(SESSION.deliver_preview(id, Err(FilePreviewError::PermissionDenied("x"))));
        assert_eq!(
            poll_file_preview(id),
            Some(Err(FilePreviewError::PermissionDenied("x")))
        );
        assert_eq!(poll_file_preview(id), None);
    }
}
