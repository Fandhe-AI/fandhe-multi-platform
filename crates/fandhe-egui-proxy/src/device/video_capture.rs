//! Phase 11 型サンプル「動画の撮影」（親 #169、Issue #821）の effect ドライバ。
//! `fandhe_core::screens::video_capture_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::video_capture::VideoCapturer` に撮影 UI の表示を依頼して、
//! OS 層から届く結果を `CompletionSender` 経由で画面へ返す（`crate::screens::registry` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約（結果の三値・数値表・守る範囲）の SSOT は `fandhe_device::video_capture`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch・bridge・本ファイルは #821 で最終形。後続の子はここを触らない）
//!
//! - iOS: `ios_video_capture`（#822。`UIImagePickerController`）
//! - Android: `android_video_capture`（#823。`ACTION_VIDEO_CAPTURE`）
//! - Web: `fandhe-web-dom` 側（#824）。wasm32 では本ファイルをビルドしない
//! - macOS: `macos_video_capture`（#1071。AppKit の撮影パネル + `AVCaptureMovieFileOutput`）
//! - Windows: 保留のため `UnsupportedVideoCapturer` 固定（ダミーの成功実装へは倒さない）。
//!   Linux は対象外
//!
//! # OS 層が呼ぶ受け口
//!
//! - [`try_begin_present`]: 撮影 UI を出す直前に呼ぶ。`false` なら出さない（古い要求・中断済み・
//!   提示済み）。`true` を返した要求だけが成功値（撮れた・取り消し）を返せる
//! - [`complete_from_os`]: 結果を 1 回だけ送る唯一の受け口。古い要求 id・重複は捨てられる
//!
//! # 期限を持たない理由
//!
//! 撮影は利用者の操作時間に依存するため、Rust 側で期限・タイマーを置かない（OS を正とする）。
//! OS から完了通知が届かない場合、画面は撮影中のままになる（`fandhe_device::video_capture` の
//! 守らない範囲）。
//!
//! # スレッド
//!
//! drive は UI スレッド（毎フレーム）、`try_begin_present` と `complete_from_os` は OS 層の
//! スレッドから呼ばれる。判定・`RequestSlot::deliver`・`active` の更新は同じロックの内側で行う。
//!
//! # 秘密値（契約表 8）
//!
//! 動画の中身・パス・OS のエラー文はログに出さない。失敗は理由の種別だけを持つ。

use std::sync::{Arc, Mutex, OnceLock};

use fandhe_core::screens::video_capture_show::VideoCaptureShowState;
use fandhe_core::screens::video_capture_show::{
    finish_capture, mark_running, take_request, ClipOrigin, CoreFailure, CoreOutcome, Phase,
    ShownClip,
};
use fandhe_device::video_capture::{
    VideoCaptureFailure, VideoCaptureOutcome, VideoCapturer, MAX_DURATION_SECS,
};

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

/// OS 層から届いて検査を終えた結果。
type Outcome = Result<VideoCaptureOutcome, VideoCaptureFailure>;

/// 現在のプラットフォームに応じた `VideoCapturer` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn VideoCapturer> {
    #[cfg(target_os = "ios")]
    {
        super::ios_video_capture::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_video_capture::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_video_capture::implementation()
    }
    // Windows は保留、Linux は対象外。Web は `fandhe-web-dom` 側。
    #[cfg(not(any(target_os = "ios", target_os = "android", target_os = "macos")))]
    {
        Arc::new(fandhe_device::video_capture::UnsupportedVideoCapturer)
    }
}

/// 進行中の要求。
struct Active {
    request_id: u64,
    /// OS が撮影 UI を出し始めたことを `try_begin_present` で記録済みか。
    presenting: bool,
}

/// 要求の追跡と表示開始の記録。テストが並列で干渉しないよう static から切り出して
/// 引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Bridge {
    slot: &'static RequestSlot<Outcome>,
    active: Mutex<Option<Active>>,
}

impl Bridge {
    fn new() -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            active: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 発送済み（OS 呼び出し中、または結果が画面へ未反映）の要求があるか。
    fn has_pending(&self) -> bool {
        self.lock().is_some() || self.slot.snapshot().is_some()
    }

    /// 新しい要求を登録して id を返す。OS へ渡す前に呼び、即時の完了も照合できるようにする。
    fn begin(&self) -> u64 {
        let mut active = self.lock();
        let request_id = self.slot.begin();
        *active = Some(Active {
            request_id,
            presenting: false,
        });
        request_id
    }

    /// 表示開始の記録。現在の要求で、未提示のときだけ `true`。
    #[allow(dead_code)] // iOS・Android 以外のビルドでは単体テストだけが呼ぶ。
    fn try_begin_present(&self, request_id: u64) -> bool {
        let mut active = self.lock();
        let Some(a) = active.as_mut().filter(|a| a.request_id == request_id) else {
            return false;
        };
        if a.presenting {
            return false;
        }
        a.presenting = true;
        true
    }

    /// OS 層からの唯一の結果の受け口。古い id・確定済みの完了は捨てる。
    fn complete_from_os(&self, request_id: u64, result: Outcome) {
        let mut active = self.lock();
        let Some(a) = active.as_ref().filter(|a| a.request_id == request_id) else {
            return;
        };
        // 表示開始を記録していない要求の成功値は嘘の成功になるため成功にしない。
        let result = match result {
            Ok(_) if !a.presenting => Err(VideoCaptureFailure::Internal),
            r => r,
        };
        self.slot.deliver(request_id, result);
        *active = None;
    }

    fn reset(&self) {
        let mut active = self.lock();
        *active = None;
        self.slot.reset();
    }
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

fn bridge() -> &'static Bridge {
    BRIDGE.get_or_init(Bridge::new)
}

/// OS 層が撮影 UI を出す直前に呼ぶ。`false` なら出さない（モジュール doc 参照）。
// iOS・Android 以外のビルドでは未使用（スタブ段階では #822・#823 が呼ぶ）。
#[allow(dead_code)]
pub(crate) fn try_begin_present(request_id: u64) -> bool {
    bridge().try_begin_present(request_id)
}

/// OS 層が結果を 1 回だけ送る受け口（モジュール doc 参照）。
// iOS・Android 以外のビルドでは未使用（スタブ段階では #822・#823 が呼ぶ）。
#[allow(dead_code)]
pub(crate) fn complete_from_os(request_id: u64, result: Outcome) {
    bridge().complete_from_os(request_id, result);
}

/// 画面の作り直し時に保留中の要求を破棄する。画面状態は `default()`（世代 0）に戻るため、
/// 保留が残ると次の要求が発送されず飲み込まれる。旧要求の遅延完了は id 不一致で捨てられる。
// Android の再生成経路（`android.rs`）から呼ばれる。Android 以外では未使用。
#[allow(dead_code)]
pub(crate) fn reset_pending_tracking() {
    if let Some(b) = BRIDGE.get() {
        b.reset();
    }
}

/// OS 層の失敗を画面の失敗種別へ写す。動画・OS の文言は渡さない。
fn to_core_failure(f: VideoCaptureFailure) -> CoreFailure {
    match f {
        VideoCaptureFailure::Unsupported => CoreFailure::Unsupported,
        VideoCaptureFailure::PermissionDenied => CoreFailure::PermissionDenied,
        VideoCaptureFailure::NoCameraApp => CoreFailure::NoCameraApp,
        VideoCaptureFailure::NotForeground => CoreFailure::NotForeground,
        VideoCaptureFailure::Interrupted => CoreFailure::Interrupted,
        VideoCaptureFailure::Empty => CoreFailure::Empty,
        VideoCaptureFailure::Unreadable => CoreFailure::Unreadable,
        VideoCaptureFailure::Internal => CoreFailure::Internal,
    }
}

/// 要求を登録して OS へ渡す。同期失敗はその場で確定する。
fn start_request(bridge: &Bridge, imp: &dyn VideoCapturer) {
    let request_id = bridge.begin();
    if let Err(f) = imp.capture(request_id, MAX_DURATION_SECS) {
        bridge.complete_from_os(request_id, Err(f));
    }
}

/// 1 フレームぶんの処理。要求の発送・完了の画面反映を行う。
fn drive(
    state: &VideoCaptureShowState,
    sender: &CompletionSender<VideoCaptureShowState>,
    imp: &Arc<dyn VideoCapturer>,
    bridge: &Bridge,
) {
    if take_request(state).is_some() {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if bridge.has_pending() {
            return;
        }
        let _ = sender.send(Box::new(mark_running));
        start_request(bridge, &**imp);
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }
    if state.phase != Phase::Running {
        return;
    }
    let generation = state.generation;
    let slot = bridge.slot;
    let Some((pending_id, _)) = slot.snapshot() else {
        return;
    };
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut VideoCaptureShowState, outcome: Outcome| {
            let applied = apply_outcome(s, generation, outcome);
            // 反映できない完了（世代違い）は捨てる。残すとスロットが埋まったままになり、
            // `has_pending` が真のまま以降の要求が発送されない。
            if !applied {
                slot.clear(pending_id);
            }
            applied
        },
    );
}

/// 完了を画面状態へ反映する。反映できなければ `false`（呼び出し側がスロットを片付ける）。
fn apply_outcome(s: &mut VideoCaptureShowState, generation: u64, outcome: Outcome) -> bool {
    let result = match outcome {
        Ok(VideoCaptureOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(VideoCaptureOutcome::Captured(c)) => Ok(CoreOutcome::Captured(ShownClip {
            size_bytes: c.size_bytes(),
            duration_ms: c.duration_ms(),
            width: c.width(),
            height: c.height(),
            // iOS・Android は OS の撮影 UI 経由。Web は `fandhe-web-dom` 側で `Received` を使う。
            origin: ClipOrigin::Recorded,
        })),
        Err(f) => Err(to_core_failure(f)),
    };
    finish_capture(s, generation, result)
}

/// `registry.rs` の `effect` 区画から `video_capture_show` 画面用に配線される effect ドライバ。
pub fn video_capture_show_effect(
    state: &VideoCaptureShowState,
    sender: &CompletionSender<VideoCaptureShowState>,
) {
    static IMPL: OnceLock<Arc<dyn VideoCapturer>> = OnceLock::new();
    drive(state, sender, IMPL.get_or_init(platform), bridge());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::video_capture_show::{
        error_text, request_capture, status_text, MAX_DURATION_SECS as CORE_MAX_SECS,
    };
    use fandhe_device::video_capture::{UnsupportedVideoCapturer, VideoClip};
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    type Rx = mpsc::Receiver<crate::app::Completion<VideoCaptureShowState>>;

    fn channel() -> (CompletionSender<VideoCaptureShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn new_bridge() -> &'static Bridge {
        Box::leak(Box::new(Bridge::new()))
    }

    fn clip() -> VideoClip {
        VideoClip::from_os(4096, Some(2500), 1920, 1080, 90).unwrap()
    }

    #[derive(Clone)]
    enum Mode {
        /// 同期で失敗を返す。
        Fail(VideoCaptureFailure),
        /// 何も起きない（OS が応答しない）。
        Silent,
        /// 表示を始め、動画を送る。
        PresentThenCapture,
        /// 表示を始め、取り消しを送る。
        PresentThenCancel,
        /// 表示を始めずに動画を送る（契約違反の OS）。
        CaptureWithoutPresent,
        /// 表示を始めずに取り消しを送る（契約違反の OS）。
        CancelWithoutPresent,
        /// 表示を始めて権限拒否を送る。
        PermissionDenied,
        /// 表示を始めずに中断を送る。
        Interrupt,
    }

    struct Fake {
        mode: Mode,
        calls: AtomicUsize,
        last_max: AtomicU32,
        bridge: &'static Bridge,
    }

    impl VideoCapturer for Fake {
        fn capture(
            &self,
            request_id: u64,
            max_duration_secs: u32,
        ) -> Result<(), VideoCaptureFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.last_max.store(max_duration_secs, Ordering::SeqCst);
            let b = self.bridge;
            match self.mode.clone() {
                Mode::Fail(f) => return Err(f),
                Mode::Silent => {}
                Mode::PresentThenCapture => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            b.complete_from_os(
                                request_id,
                                Ok(VideoCaptureOutcome::Captured(clip())),
                            );
                        }
                    });
                }
                Mode::PresentThenCancel => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            b.complete_from_os(request_id, Ok(VideoCaptureOutcome::Cancelled));
                        }
                    });
                }
                Mode::CaptureWithoutPresent => {
                    b.complete_from_os(request_id, Ok(VideoCaptureOutcome::Captured(clip())));
                }
                Mode::CancelWithoutPresent => {
                    b.complete_from_os(request_id, Ok(VideoCaptureOutcome::Cancelled));
                }
                Mode::PermissionDenied => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            b.complete_from_os(
                                request_id,
                                Err(VideoCaptureFailure::PermissionDenied),
                            );
                        }
                    });
                }
                Mode::Interrupt => {
                    b.complete_from_os(request_id, Err(VideoCaptureFailure::Interrupted));
                }
            }
            Ok(())
        }
    }

    fn fake(mode: Mode, bridge: &'static Bridge) -> Arc<Fake> {
        Arc::new(Fake {
            mode,
            calls: AtomicUsize::new(0),
            last_max: AtomicU32::new(0),
            bridge,
        })
    }

    /// フレームを回して届いた完了を適用する。`Idle` に戻るか 5 秒で打ち切る。
    fn pump(
        state: &mut VideoCaptureShowState,
        sender: &CompletionSender<VideoCaptureShowState>,
        rx: &Rx,
        imp: &Arc<dyn VideoCapturer>,
        bridge: &Bridge,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, bridge);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if state.phase == Phase::Idle || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn run(mode: Mode) -> (VideoCaptureShowState, Arc<Fake>) {
        let bridge = new_bridge();
        let f = fake(mode, bridge);
        let imp: Arc<dyn VideoCapturer> = f.clone();
        let (sender, rx) = channel();
        let mut state = VideoCaptureShowState::default();
        request_capture(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        (state, f)
    }

    #[test]
    fn no_request_does_nothing() {
        let bridge = new_bridge();
        let imp: Arc<dyn VideoCapturer> = fake(Mode::Silent, bridge);
        let (sender, rx) = channel();
        drive(&VideoCaptureShowState::default(), &sender, &imp, bridge);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unsupported_impl_is_unsupported_not_success() {
        let bridge = new_bridge();
        let imp: Arc<dyn VideoCapturer> = Arc::new(UnsupportedVideoCapturer);
        let (sender, rx) = channel();
        let mut state = VideoCaptureShowState::default();
        request_capture(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::Unsupported)));
    }

    #[test]
    fn fallback_platform_is_unsupported_off_ios_android_macos() {
        #[cfg(not(any(target_os = "ios", target_os = "android", target_os = "macos")))]
        assert_eq!(
            platform().capture(1, MAX_DURATION_SECS),
            Err(VideoCaptureFailure::Unsupported)
        );
    }

    #[test]
    fn captured_is_reported_with_metadata() {
        let (s, f) = run(Mode::PresentThenCapture);
        let Some(Ok(CoreOutcome::Captured(c))) = &s.result else {
            panic!("{:?}", s.result);
        };
        assert_eq!(
            (c.size_bytes, c.duration_ms, c.width, c.height, c.origin),
            (4096, Some(2500), 1080, 1920, ClipOrigin::Recorded)
        );
        assert!(status_text(&s).starts_with("撮った動画"));
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn driver_passes_the_shared_max_duration() {
        let (_, f) = run(Mode::PresentThenCancel);
        assert_eq!(f.last_max.load(Ordering::SeqCst), MAX_DURATION_SECS);
        assert_eq!(MAX_DURATION_SECS, CORE_MAX_SECS);
    }

    #[test]
    fn permission_denied_is_a_failure() {
        let (s, _) = run(Mode::PermissionDenied);
        assert_eq!(s.result, Some(Err(CoreFailure::PermissionDenied)));
        assert_ne!(error_text(&s), "");
    }

    #[test]
    fn cancel_is_not_a_failure() {
        let (s, _) = run(Mode::PresentThenCancel);
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn sync_failures_are_reported() {
        for (f, expect) in [
            (
                VideoCaptureFailure::NotForeground,
                CoreFailure::NotForeground,
            ),
            (VideoCaptureFailure::NoCameraApp, CoreFailure::NoCameraApp),
            (VideoCaptureFailure::Unsupported, CoreFailure::Unsupported),
            (VideoCaptureFailure::Internal, CoreFailure::Internal),
        ] {
            let (s, _) = run(Mode::Fail(f));
            assert_eq!(s.result, Some(Err(expect)));
        }
    }

    #[test]
    fn interrupted_is_a_failure() {
        let (s, _) = run(Mode::Interrupt);
        assert_eq!(s.result, Some(Err(CoreFailure::Interrupted)));
    }

    #[test]
    fn success_values_without_present_are_internal() {
        for mode in [Mode::CaptureWithoutPresent, Mode::CancelWithoutPresent] {
            let (s, _) = run(mode);
            assert_eq!(s.result, Some(Err(CoreFailure::Internal)));
        }
    }

    #[test]
    fn stale_request_id_is_dropped_and_duplicates_first_wins() {
        let bridge = new_bridge();
        let id = bridge.begin();
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id + 1, Err(VideoCaptureFailure::Empty));
        assert!(matches!(bridge.slot.snapshot(), Some((i, None)) if i == id));
        bridge.complete_from_os(id, Ok(VideoCaptureOutcome::Cancelled));
        bridge.complete_from_os(id, Err(VideoCaptureFailure::Internal));
        assert!(matches!(
            bridge.slot.snapshot(),
            Some((i, Some(Ok(VideoCaptureOutcome::Cancelled)))) if i == id
        ));
        bridge.reset();
        let id2 = bridge.begin();
        assert!(bridge.try_begin_present(id2));
        assert!(!bridge.try_begin_present(id2));
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = VideoCaptureShowState::default();
        request_capture(&mut state);
        mark_running(&mut state);
        assert!(!finish_capture(&mut state, 99, Ok(CoreOutcome::Cancelled)));
        assert_eq!(state.phase, Phase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_captures_once() {
        let bridge = new_bridge();
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn VideoCapturer> = f.clone();
        let (sender, rx) = channel();
        let mut state = VideoCaptureShowState::default();
        request_capture(&mut state);
        drive(&state, &sender, &imp, bridge);
        drive(&state, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1, "mark_running は 1 件のみ");
    }

    #[test]
    fn mismatched_generation_completion_clears_slot_and_unblocks_dispatch() {
        let bridge = new_bridge();
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn VideoCapturer> = f.clone();
        let (sender, rx) = channel();
        let mut state = VideoCaptureShowState::default();
        request_capture(&mut state);
        drive(&state, &sender, &imp, bridge);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        let id = bridge.slot.snapshot().unwrap().0;
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id, Ok(VideoCaptureOutcome::Cancelled));
        // 世代がずれた画面（再生成など）へ完了が届く。
        state.generation += 1;
        drive(&state, &sender, &imp, bridge);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert!(
            bridge.slot.snapshot().is_none(),
            "不一致でもスロットを片付ける"
        );
        assert!(!bridge.has_pending());
        // 次の要求が発送される。
        let mut fresh = VideoCaptureShowState::default();
        request_capture(&mut fresh);
        drive(&fresh, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reset_drops_old_completion_and_allows_next_dispatch() {
        let bridge = new_bridge();
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn VideoCapturer> = f.clone();
        let (sender, rx) = channel();
        let mut state = VideoCaptureShowState::default();
        request_capture(&mut state);
        drive(&state, &sender, &imp, bridge);
        let old_id = bridge.slot.snapshot().unwrap().0;
        bridge.reset();
        bridge.complete_from_os(old_id, Ok(VideoCaptureOutcome::Cancelled));
        assert!(bridge.slot.snapshot().is_none());
        let mut fresh = VideoCaptureShowState::default();
        request_capture(&mut fresh);
        while rx.try_recv().is_ok() {}
        drive(&fresh, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn interrupted_before_present_is_reported_and_blocks_later_present() {
        let (s, _) = run(Mode::Interrupt);
        assert_eq!(s.result, Some(Err(CoreFailure::Interrupted)));
        // 確定済みの要求は提示できない。
        assert!(!bridge_is_unusable_after_complete());
    }

    fn bridge_is_unusable_after_complete() -> bool {
        let bridge = new_bridge();
        let id = bridge.begin();
        bridge.complete_from_os(id, Err(VideoCaptureFailure::Interrupted));
        bridge.try_begin_present(id)
    }
}
