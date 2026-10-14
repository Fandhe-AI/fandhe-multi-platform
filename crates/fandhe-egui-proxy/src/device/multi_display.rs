//! Phase 11 の拡充ループ（親 #1134、Issue #1341）「複数ディスプレイ」の effect ドライバ。
//! `fandhe_device::multi_display::MultiDisplay` を一定間隔で読み、結果を
//! `fandhe_core::screens::multi_display_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。
//!
//! **間隔つき pull**（`network_status.rs` と同型）: 最終読取時刻から `READ_INTERVAL` 未満なら OS を
//! 読まず残り時間で再描画を予約する。読んだら次回の再描画を必ず予約し（契約表 4）、
//! 状態が変わらなければ送信を省略する。OS の結果は必ず `validate` を通す（空の一覧・サイズ 0 を
//! 成功にしない）。
//!
//! dispatch（[`platform_multi_display`]）は全対象 OS の最終形。Android は
//! `android_multi_display.rs`（`DisplayManager` を JNI で読む実装）、Web は `fandhe-web-dom` 側（#1344）、
//! iOS・Linux 等は `UnsupportedMultiDisplay` 固定でダミーの成功へ倒さない。

use std::sync::Mutex;
use std::time::Instant;

use fandhe_core::screens::multi_display_show::{
    apply_read, MultiDisplayShowState, ReadFailure, ScreenRow,
};
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
use fandhe_device::multi_display::UnsupportedMultiDisplay;
use fandhe_device::multi_display::{validate, MultiDisplay, ScreenInfo, READ_INTERVAL};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_multi_display() -> Box<dyn MultiDisplay> {
    #[cfg(target_os = "windows")]
    {
        super::windows_multi_display::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_multi_display::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_multi_display::implementation()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    {
        Box::new(UnsupportedMultiDisplay)
    }
}

fn to_row(s: ScreenInfo) -> ScreenRow {
    ScreenRow {
        width_px: s.width_px,
        height_px: s.height_px,
        scale: s.scale,
        is_primary: s.is_primary,
        name: s.name,
    }
}

/// 契約の結果を画面の入力へ写す。失敗は固定文言へ畳み、OS 固有値を画面へ漏らさない
/// （security.md A09）。
fn to_result(r: Result<Vec<ScreenInfo>, DeviceError>) -> Result<Vec<ScreenRow>, ReadFailure> {
    match r {
        Ok(v) => Ok(v.into_iter().map(to_row).collect()),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(ReadFailure::Failed("画面一覧の取得".to_string())),
    }
}

/// 最終読取時刻（プロセス全体で 1 つ）。
static LAST_READ: Mutex<Option<Instant>> = Mutex::new(None);

/// 間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &MultiDisplayShowState,
    sender: &CompletionSender<MultiDisplayShowState>,
    imp: &dyn MultiDisplay,
    now: Instant,
    last: &mut Option<Instant>,
) {
    if let Some(prev) = *last {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            sender.request_repaint_after(READ_INTERVAL - elapsed);
            return;
        }
    }
    *last = Some(now);
    // 送信を省略する分岐でも次回の読み取りを予約する。
    sender.request_repaint_after(READ_INTERVAL);

    let result = to_result(validate(imp.read()));
    let mut next = state.clone();
    apply_read(&mut next, result.clone());
    if next == *state {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut MultiDisplayShowState| {
        apply_read(s, result);
    }));
}

/// `registry.rs` の `effect` 区画から `multi_display_show` 画面用に配線される effect ドライバ。
pub fn multi_display_show_effect(
    state: &MultiDisplayShowState,
    sender: &CompletionSender<MultiDisplayShowState>,
) {
    let mut last = LAST_READ
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        platform_multi_display().as_ref(),
        Instant::now(),
        &mut last,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::multi_display_show::{list_text, DisplayView};
    use fandhe_device::multi_display::UnsupportedMultiDisplay as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<MultiDisplayShowState>>;

    fn sender_with_channel() -> (CompletionSender<MultiDisplayShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<Vec<ScreenInfo>, ()>,
        reads: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<Vec<ScreenInfo>, ()>) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
            }
        }
    }

    impl MultiDisplay for Fake {
        fn read(&self) -> Result<Vec<ScreenInfo>, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.result.clone().map_err(|_| DeviceError::Internal("x"))
        }
    }

    fn scr(w: u32, primary: bool) -> ScreenInfo {
        ScreenInfo {
            width_px: w,
            height_px: 100,
            scale: Some(2.0),
            is_primary: primary,
            name: None,
        }
    }

    fn apply_all(rx: &Rx, s: &mut MultiDisplayShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    #[test]
    fn skips_os_read_within_interval() {
        let (tx, _rx) = sender_with_channel();
        let fake = Fake::new(Ok(vec![scr(10, true)]));
        let state = MultiDisplayShowState::default();
        let t0 = Instant::now();
        let mut last = None;
        drive(&state, &tx, &fake, t0, &mut last);
        drive(
            &state,
            &tx,
            &fake,
            t0 + Duration::from_millis(500),
            &mut last,
        );
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        drive(&state, &tx, &fake, t0 + READ_INTERVAL, &mut last);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn same_value_is_not_sent_and_change_is() {
        let (tx, rx) = sender_with_channel();
        let mut state = MultiDisplayShowState::default();
        let t0 = Instant::now();
        let mut last = None;
        drive(
            &state,
            &tx,
            &Fake::new(Ok(vec![scr(10, true)])),
            t0,
            &mut last,
        );
        apply_all(&rx, &mut state);
        assert!(matches!(state.view, DisplayView::Listed(_)));
        // 同値: 送信されない。
        drive(
            &state,
            &tx,
            &Fake::new(Ok(vec![scr(10, true)])),
            t0 + READ_INTERVAL,
            &mut last,
        );
        assert!(rx.try_recv().is_err());
        // 変化: 送信される。
        drive(
            &state,
            &tx,
            &Fake::new(Ok(vec![scr(10, true), scr(20, false)])),
            t0 + READ_INTERVAL * 2,
            &mut last,
        );
        apply_all(&rx, &mut state);
        assert_eq!(list_text(&state).lines().count(), 2);
    }

    #[test]
    fn list_is_not_kept_after_failure() {
        let (tx, rx) = sender_with_channel();
        let mut state = MultiDisplayShowState::default();
        let t0 = Instant::now();
        let mut last = None;
        drive(
            &state,
            &tx,
            &Fake::new(Ok(vec![scr(10, true)])),
            t0,
            &mut last,
        );
        apply_all(&rx, &mut state);
        drive(
            &state,
            &tx,
            &Fake::new(Err(())),
            t0 + READ_INTERVAL,
            &mut last,
        );
        apply_all(&rx, &mut state);
        assert!(matches!(state.view, DisplayView::Failed(_)));
        assert_eq!(list_text(&state), "");
    }

    #[test]
    fn unsupported_impl_reports_unsupported() {
        let (tx, rx) = sender_with_channel();
        let mut state = MultiDisplayShowState::default();
        drive(&state, &tx, &Unsupported, Instant::now(), &mut None);
        apply_all(&rx, &mut state);
        assert_eq!(state.view, DisplayView::Unsupported);
    }

    #[test]
    fn empty_list_and_zero_size_become_failed() {
        for v in [vec![], vec![scr(0, true)]] {
            let (tx, rx) = sender_with_channel();
            let mut state = MultiDisplayShowState::default();
            drive(&state, &tx, &Fake::new(Ok(v)), Instant::now(), &mut None);
            apply_all(&rx, &mut state);
            assert!(matches!(state.view, DisplayView::Failed(_)));
        }
    }

    #[test]
    fn two_primaries_show_none_as_primary() {
        let (tx, rx) = sender_with_channel();
        let mut state = MultiDisplayShowState::default();
        let fake = Fake::new(Ok(vec![scr(10, true), scr(20, true)]));
        drive(&state, &tx, &fake, Instant::now(), &mut None);
        apply_all(&rx, &mut state);
        assert!(!list_text(&state).contains("主画面"));
    }
}
