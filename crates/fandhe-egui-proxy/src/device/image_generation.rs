//! Phase 11 型サンプル「端末内での画像生成」（親 #191、Issue #975）の effect ドライバ。
//! `fandhe_core::screens::image_generation_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::image_generation::ImageGenerator` に生成画面の表示を依頼して、
//! OS 層から届く結果を `CompletionSender` 経由で画面へ返す（`crate::screens::registry` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約（結果の三値・期限・守る範囲）の SSOT は `fandhe_device::image_generation`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch・bridge・本ファイルは #975 で最終形。後続の子はここを触らない）
//!
//! - macOS: `apple_image_generation`（#975。Image Playground の `ImagePlaygroundViewController`）
//! - iOS: `ios_image_generation`（#976 が UIKit の実体を置く。#975 時点ではスタブ）
//! - Android・Windows・Linux: 対応表で `no` のため `UnsupportedImageGenerator` 固定
//!   （ダミーの成功実装へは倒さない）。Web は対象外
//!
//! # OS 層が呼ぶ受け口
//!
//! - [`try_begin_present`]: 生成画面を出す直前に呼ぶ。`false` なら出さない（期限切れ・古い要求・
//!   中断済み）。`true` を返した要求だけが成功値（生成した・キャンセル）を返せる
//! - [`complete_from_os`]: 結果を 1 回だけ送る唯一の受け口。古い要求 id・重複は捨てられる
//!
//! # 期限（契約表 3）
//!
//! 生成画面が出るまでの [`PRESENT_DEADLINE`] だけを持つ。起点は要求を観測した時の `Instant`
//! （単調時計）。期限切れの確定（`expire_if_due` と `try_begin_present` 内）は表示開始の記録と
//! 同じロックで行うため、期限切れを確定した後に OS が生成画面を出すことはない。
//!
//! # スレッド
//!
//! drive は UI スレッド（毎フレーム）、`try_begin_present` と `complete_from_os` は OS 層の
//! スレッド（main queue）から呼ばれる。判定・`RequestSlot::deliver`・`active` の更新は同じロックの
//! 内側で行う。
//!
//! # 秘密値（契約表 8）
//!
//! 画素・プロンプト・OS のエラー文はログに出さない。失敗は理由の種別だけを持つ。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::image_generation_show::ImageGenerationShowState;
use fandhe_core::screens::image_generation_show::{
    finish_generate, mark_running, take_request, CoreFailure, CoreOutcome, Phase,
};
use fandhe_core::RgbaPixels;
use fandhe_device::image_generation::{
    ImageGenerationFailure, ImageGenerationOutcome, ImageGenerator, MAX_SIDE, PRESENT_DEADLINE,
};

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

// 受け取る寸法の上限は描画側の上限を超えない（超えると `RgbaPixels::new` が `None` になる）。
const _: () = assert!(MAX_SIDE <= fandhe_core::RGBA_MAX_SIDE);

/// bridge が画面側へ渡す成功側の結果。`RequestSlot` が `Clone` を要るため画素は `Arc` で持つ。
#[derive(Clone)]
enum CoreGen {
    Generated(RgbaPixels),
    Cancelled,
}

/// OS 層から届いて検査を終えた結果。
type Outcome = Result<CoreGen, ImageGenerationFailure>;

/// 現在のプラットフォームに応じた `ImageGenerator` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn ImageGenerator> {
    #[cfg(target_os = "macos")]
    {
        super::apple_image_generation::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_image_generation::implementation()
    }
    // Android・Windows・Linux は対応表で no。
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        Arc::new(fandhe_device::image_generation::UnsupportedImageGenerator)
    }
}

/// 進行中の要求。
struct Active {
    request_id: u64,
    /// 要求を観測した時刻（単調時計）。期限の起点。
    started: Instant,
    /// OS が生成画面を出し始めたことを `try_begin_present` で記録済みか。
    presenting: bool,
}

/// 要求の追跡と期限・表示開始の排他。テストが並列で干渉しないよう static から切り出して
/// 引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Bridge {
    slot: &'static RequestSlot<Outcome>,
    active: Mutex<Option<Active>>,
    deadline: Duration,
}

impl Bridge {
    fn new(deadline: Duration) -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            active: Mutex::new(None),
            deadline,
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
            started: Instant::now(),
            presenting: false,
        });
        request_id
    }

    /// 表示開始の記録。現在の要求で、未確定で、期限前のときだけ `true`。
    /// 期限を過ぎていれば `TimedOut` を確定して `false`。
    fn try_begin_present(&self, request_id: u64) -> bool {
        let mut active = self.lock();
        let Some(a) = active.as_mut().filter(|a| a.request_id == request_id) else {
            return false;
        };
        if a.presenting {
            return false;
        }
        if a.started.elapsed() >= self.deadline {
            self.slot
                .deliver(request_id, Err(ImageGenerationFailure::TimedOut));
            *active = None;
            return false;
        }
        a.presenting = true;
        true
    }

    /// OS 層からの唯一の結果の受け口。
    ///
    /// 先に id を照合して古い・確定済みの完了を画素の変換前に捨てる。変換はロックの外で行い、
    /// 届ける前にもう一度照合する（変換中に期限切れ・差し替えが起きうるため）。
    fn complete_from_os(
        &self,
        request_id: u64,
        result: Result<ImageGenerationOutcome, ImageGenerationFailure>,
    ) {
        if !self.is_current(request_id) {
            return;
        }
        let result: Outcome = match result {
            Ok(ImageGenerationOutcome::Cancelled) => Ok(CoreGen::Cancelled),
            Ok(ImageGenerationOutcome::Generated(img)) => {
                let (w, h) = (img.width(), img.height());
                // 描画側の検証（`RgbaPixels::new`）を通らない画素は成功にしない。
                RgbaPixels::new(w, h, img.into_rgba())
                    .map(CoreGen::Generated)
                    .ok_or(ImageGenerationFailure::Undecodable)
            }
            Err(f) => Err(f),
        };
        let mut active = self.lock();
        let Some(a) = active.as_ref().filter(|a| a.request_id == request_id) else {
            return;
        };
        // 表示開始を記録していない要求の成功値は嘘の成功になるため成功にしない。
        let result = match result {
            Ok(_) if !a.presenting => Err(ImageGenerationFailure::Internal),
            r => r,
        };
        self.slot.deliver(request_id, result);
        *active = None;
    }

    /// `request_id` が現在の未確定の要求か。
    fn is_current(&self, request_id: u64) -> bool {
        self.lock()
            .as_ref()
            .is_some_and(|a| a.request_id == request_id)
    }

    /// 表示開始前に期限を過ぎていれば `TimedOut` を確定する。
    fn expire_if_due(&self) {
        let mut active = self.lock();
        let due = active
            .as_ref()
            .is_some_and(|a| !a.presenting && a.started.elapsed() >= self.deadline);
        if due {
            if let Some(a) = active.take() {
                self.slot
                    .deliver(a.request_id, Err(ImageGenerationFailure::TimedOut));
            }
        }
    }
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

fn bridge() -> &'static Bridge {
    BRIDGE.get_or_init(|| Bridge::new(PRESENT_DEADLINE))
}

/// OS 層が生成画面を出す直前に呼ぶ。`false` なら出さない（モジュール doc 参照）。
// macOS・iOS 以外のビルドでは未使用（OS 層の受け口が呼ぶ）。
#[allow(dead_code)]
pub(crate) fn try_begin_present(request_id: u64) -> bool {
    bridge().try_begin_present(request_id)
}

/// OS 層が結果を 1 回だけ送る受け口（モジュール doc 参照）。
// macOS・iOS 以外のビルドでは未使用（OS 層の受け口が呼ぶ）。
#[allow(dead_code)]
pub(crate) fn complete_from_os(
    request_id: u64,
    result: Result<ImageGenerationOutcome, ImageGenerationFailure>,
) {
    bridge().complete_from_os(request_id, result);
}

/// OS 層の失敗を画面の失敗種別へ写す。画素・OS の文言は渡さない。
fn to_core_failure(f: ImageGenerationFailure) -> CoreFailure {
    match f {
        ImageGenerationFailure::Unsupported => CoreFailure::Unsupported,
        ImageGenerationFailure::NoWindow => CoreFailure::NoWindow,
        ImageGenerationFailure::Undecodable => CoreFailure::Undecodable,
        ImageGenerationFailure::TooLarge => CoreFailure::TooLarge,
        ImageGenerationFailure::NotForeground => CoreFailure::NotForeground,
        ImageGenerationFailure::TimedOut => CoreFailure::TimedOut,
        ImageGenerationFailure::Internal => CoreFailure::Internal,
    }
}

/// 要求を登録して OS へ渡す。同期失敗はその場で確定する。
fn start_request(bridge: &Bridge, imp: &dyn ImageGenerator) {
    let request_id = bridge.begin();
    if let Err(f) = imp.generate(request_id) {
        bridge.complete_from_os(request_id, Err(f));
    }
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &ImageGenerationShowState,
    sender: &CompletionSender<ImageGenerationShowState>,
    imp: &Arc<dyn ImageGenerator>,
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
    bridge.expire_if_due();
    let slot = bridge.slot;
    let Some((pending_id, _)) = slot.snapshot() else {
        return;
    };
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut ImageGenerationShowState, outcome: Outcome| {
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
fn apply_outcome(s: &mut ImageGenerationShowState, generation: u64, outcome: Outcome) -> bool {
    let result = match outcome {
        Ok(CoreGen::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(CoreGen::Generated(p)) => Ok(CoreOutcome::Generated(p)),
        Err(f) => Err(to_core_failure(f)),
    };
    finish_generate(s, generation, result)
}

/// `registry.rs` の `effect` 区画から `image_generation_show` 画面用に配線される effect ドライバ。
pub fn image_generation_show_effect(
    state: &ImageGenerationShowState,
    sender: &CompletionSender<ImageGenerationShowState>,
) {
    static IMPL: OnceLock<Arc<dyn ImageGenerator>> = OnceLock::new();
    drive(state, sender, IMPL.get_or_init(platform), bridge());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::image_generation_show::{error_text, request_generate};
    use fandhe_device::image_generation::{GeneratedImage, UnsupportedImageGenerator};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<ImageGenerationShowState>>;

    fn channel() -> (CompletionSender<ImageGenerationShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn new_bridge(deadline: Duration) -> &'static Bridge {
        Box::leak(Box::new(Bridge::new(deadline)))
    }

    fn image() -> GeneratedImage {
        GeneratedImage::from_os_rgba(vec![9; 8], 2, 1).unwrap()
    }

    #[derive(Clone)]
    enum Mode {
        /// 同期で失敗を返す。
        Fail(ImageGenerationFailure),
        /// 何も起きない（OS が応答しない）。
        Silent,
        /// 表示を始め、画像を送る。
        PresentThenGenerate,
        /// 表示を始め、`ms` 待ってからキャンセルを送る。
        PresentThenCancel(u64),
        /// 表示を始めずに画像を送る（契約違反の OS）。
        GenerateWithoutPresent,
        /// 表示を始めずにキャンセルを送る（契約違反の OS）。
        CancelWithoutPresent,
    }

    struct Fake {
        mode: Mode,
        calls: AtomicUsize,
        bridge: &'static Bridge,
    }

    impl ImageGenerator for Fake {
        fn generate(&self, request_id: u64) -> Result<(), ImageGenerationFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let b = self.bridge;
            match self.mode.clone() {
                Mode::Fail(f) => return Err(f),
                Mode::Silent => {}
                Mode::PresentThenGenerate => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            b.complete_from_os(
                                request_id,
                                Ok(ImageGenerationOutcome::Generated(image())),
                            );
                        }
                    });
                }
                Mode::PresentThenCancel(ms) => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            std::thread::sleep(Duration::from_millis(ms));
                            b.complete_from_os(request_id, Ok(ImageGenerationOutcome::Cancelled));
                        }
                    });
                }
                Mode::GenerateWithoutPresent => {
                    b.complete_from_os(request_id, Ok(ImageGenerationOutcome::Generated(image())));
                }
                Mode::CancelWithoutPresent => {
                    b.complete_from_os(request_id, Ok(ImageGenerationOutcome::Cancelled));
                }
            }
            Ok(())
        }
    }

    fn fake(mode: Mode, bridge: &'static Bridge) -> Arc<Fake> {
        Arc::new(Fake {
            mode,
            calls: AtomicUsize::new(0),
            bridge,
        })
    }

    /// フレームを回して届いた完了を適用する。`Idle` に戻るか 5 秒で打ち切る。
    fn pump(
        state: &mut ImageGenerationShowState,
        sender: &CompletionSender<ImageGenerationShowState>,
        rx: &Rx,
        imp: &Arc<dyn ImageGenerator>,
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

    fn run(mode: Mode, deadline: Duration) -> (ImageGenerationShowState, Arc<Fake>) {
        let bridge = new_bridge(deadline);
        let f = fake(mode, bridge);
        let imp: Arc<dyn ImageGenerator> = f.clone();
        let (sender, rx) = channel();
        let mut state = ImageGenerationShowState::default();
        request_generate(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        (state, f)
    }

    const LONG: Duration = Duration::from_secs(30);

    #[test]
    fn unsupported_impl_is_unsupported_not_success() {
        let bridge = new_bridge(LONG);
        let imp: Arc<dyn ImageGenerator> = Arc::new(UnsupportedImageGenerator);
        let (sender, rx) = channel();
        let mut state = ImageGenerationShowState::default();
        request_generate(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::Unsupported)));
    }

    #[test]
    fn fallback_platform_is_unsupported_off_apple() {
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        assert_eq!(
            platform().generate(1),
            Err(ImageGenerationFailure::Unsupported)
        );
    }

    #[test]
    fn cancel_is_not_a_failure() {
        let (s, _) = run(Mode::PresentThenCancel(0), LONG);
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn generated_pixels_reach_the_screen() {
        let (s, _) = run(Mode::PresentThenGenerate, LONG);
        let Some(Ok(CoreOutcome::Generated(p))) = &s.result else {
            panic!("{:?}", s.result);
        };
        assert_eq!((p.width(), p.height(), p.rgba().len()), (2, 1, 8));
    }

    #[test]
    fn sync_failures_are_reported() {
        for (f, expect) in [
            (ImageGenerationFailure::NoWindow, CoreFailure::NoWindow),
            (
                ImageGenerationFailure::NotForeground,
                CoreFailure::NotForeground,
            ),
            (ImageGenerationFailure::TooLarge, CoreFailure::TooLarge),
            (ImageGenerationFailure::Internal, CoreFailure::Internal),
        ] {
            let (s, _) = run(Mode::Fail(f), LONG);
            assert_eq!(s.result, Some(Err(expect)));
        }
    }

    #[test]
    fn success_values_without_present_are_internal() {
        for mode in [Mode::GenerateWithoutPresent, Mode::CancelWithoutPresent] {
            let (s, _) = run(mode, LONG);
            assert_eq!(s.result, Some(Err(CoreFailure::Internal)));
        }
    }

    #[test]
    fn deadline_before_present_times_out_and_blocks_later_present() {
        let (s, f) = run(Mode::Silent, Duration::from_millis(20));
        assert_eq!(s.result, Some(Err(CoreFailure::TimedOut)));
        assert!(!f.bridge.try_begin_present(1));
    }

    #[test]
    fn late_success_after_timeout_is_dropped() {
        let bridge = new_bridge(Duration::from_millis(10));
        let id = bridge.begin();
        std::thread::sleep(Duration::from_millis(30));
        assert!(!bridge.try_begin_present(id));
        bridge.complete_from_os(id, Ok(ImageGenerationOutcome::Generated(image())));
        assert!(matches!(
            bridge.slot.snapshot(),
            Some((i, Some(Err(ImageGenerationFailure::TimedOut)))) if i == id
        ));
    }

    #[test]
    fn after_present_deadline_no_longer_applies() {
        let (s, _) = run(Mode::PresentThenCancel(120), Duration::from_millis(30));
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
    }

    #[test]
    fn stale_request_id_is_dropped_and_duplicates_first_wins() {
        let bridge = new_bridge(LONG);
        let id = bridge.begin();
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id + 1, Err(ImageGenerationFailure::NoWindow));
        assert!(matches!(bridge.slot.snapshot(), Some((i, None)) if i == id));
        bridge.complete_from_os(id, Ok(ImageGenerationOutcome::Cancelled));
        bridge.complete_from_os(id, Err(ImageGenerationFailure::Internal));
        assert!(matches!(
            bridge.slot.snapshot(),
            Some((i, Some(Ok(CoreGen::Cancelled)))) if i == id
        ));
        assert!(!bridge.try_begin_present(id));
    }

    #[test]
    fn mismatched_generation_completion_clears_slot_and_unblocks_dispatch() {
        let bridge = new_bridge(LONG);
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn ImageGenerator> = f.clone();
        let (sender, rx) = channel();
        let mut state = ImageGenerationShowState::default();
        request_generate(&mut state);
        drive(&state, &sender, &imp, bridge);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        let id = bridge.slot.snapshot().unwrap().0;
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id, Ok(ImageGenerationOutcome::Cancelled));
        // 世代がずれた画面（再生成など）へ完了が届く。
        state.generation += 1;
        drive(&state, &sender, &imp, bridge);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert!(bridge.slot.snapshot().is_none());
        assert!(!bridge.has_pending());
    }

    #[test]
    fn double_drive_before_mark_applied_generates_once() {
        let bridge = new_bridge(LONG);
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn ImageGenerator> = f.clone();
        let (sender, rx) = channel();
        let mut state = ImageGenerationShowState::default();
        request_generate(&mut state);
        drive(&state, &sender, &imp, bridge);
        drive(&state, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1, "mark_running は 1 件のみ");
    }
}
