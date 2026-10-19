//! Phase 11 の拡充ループ（親 #104、Issue #438）「集中モードの状態」の effect ドライバ。
//! `fandhe_device::focus_mode_status::FocusStatusSource` を一定間隔で読み、結果を
//! `fandhe_core::screens::focus_mode_status_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。
//!
//! **間隔つき pull**（`system_accent_color.rs` と同型）: 最終読取から `READ_INTERVAL` 未満なら
//! OS を読まない。状態が変わらなければ送信を省くが、次の読み取りまでの再描画を必ず予約する
//! （入力が無いと egui が描画せずポーリングが止まる、契約表 4）。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。Windows は
//! `windows_focus_mode_status.rs`、Android は `android_focus_mode_status.rs`（JNI の同期 pull。
//! 現状スタブ）、それ以外（macOS / iOS / Linux を含む）は `UnsupportedFocusStatus`。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::focus_mode_status_show::{
    apply_read, FocusModeStatusShowState, ReadFailure,
};
#[cfg(not(any(target_os = "windows", target_os = "android")))]
use fandhe_device::focus_mode_status::UnsupportedFocusStatus;
use fandhe_device::focus_mode_status::{FocusStatusSource, READ_INTERVAL};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す。
fn implementation() -> Arc<dyn FocusStatusSource> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(super::windows_focus_mode_status::WindowsFocusStatus)
    }
    #[cfg(target_os = "android")]
    {
        super::android_focus_mode_status::implementation()
    }
    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    {
        Arc::new(UnsupportedFocusStatus)
    }
}

/// 契約の結果を画面の入力へ写す。失敗は固定文言へ畳み、OS 固有値を画面へ漏らさない
/// （security.md A09）。
fn to_result(r: Result<bool, DeviceError>) -> Result<bool, ReadFailure> {
    match r {
        Ok(v) => Ok(v),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(ReadFailure::Failed("集中モードの取得".to_string())),
    }
}

/// 最終読取の時刻（プロセス全体で 1 つ）。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing { last_read: None });

/// 間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &FocusModeStatusShowState,
    sender: &CompletionSender<FocusModeStatusShowState>,
    src: &dyn FocusStatusSource,
    now: Instant,
    timing: &mut Timing,
) {
    let due = timing
        .last_read
        .is_none_or(|prev| now.saturating_duration_since(prev) >= READ_INTERVAL);
    let result = if due {
        timing.last_read = Some(now);
        Some(to_result(src.read()))
    } else {
        None
    };
    // 送信を省く分岐でも次回の読み取りを予約する（契約表 4）。
    let since_read = timing
        .last_read
        .map_or(Duration::ZERO, |p| now.saturating_duration_since(p));
    sender.request_repaint_after(READ_INTERVAL.saturating_sub(since_read));

    let Some(result) = result else { return };
    let mut next = state.clone();
    apply_read(&mut next, result.clone());
    // 失敗状態のまま毎フレーム送らない。
    if next == *state {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut FocusModeStatusShowState| {
        apply_read(s, result.clone())
    }));
}

/// `registry.rs` の `effect` 区画から `focus_mode_status_show` 画面用に配線される effect ドライバ。
pub fn focus_mode_status_show_effect(
    state: &FocusModeStatusShowState,
    sender: &CompletionSender<FocusModeStatusShowState>,
) {
    let mut timing = TIMING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        implementation().as_ref(),
        Instant::now(),
        &mut timing,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::focus_mode_status_show::FocusView;
    use fandhe_device::focus_mode_status::UnsupportedFocusStatus as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<FocusModeStatusShowState>>;

    fn sender_with_channel() -> (CompletionSender<FocusModeStatusShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<bool, DeviceError>,
        reads: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<bool, DeviceError>) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
            }
        }
    }

    impl FocusStatusSource for Fake {
        fn read(&self) -> Result<bool, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &self.result {
                Ok(v) => Ok(*v),
                Err(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Err(_) => Err(DeviceError::Internal("x")),
            }
        }
    }

    fn apply_all(rx: &Rx, s: &mut FocusModeStatusShowState) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(s);
            n += 1;
        }
        n
    }

    #[test]
    fn skips_os_read_within_interval() {
        let (tx, _rx) = sender_with_channel();
        let fake = Fake::new(Ok(true));
        let state = FocusModeStatusShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &fake, t0, &mut timing);
        drive(
            &state,
            &tx,
            &fake,
            t0 + Duration::from_millis(500),
            &mut timing,
        );
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        drive(&state, &tx, &fake, t0 + READ_INTERVAL, &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn same_value_is_not_sent_but_change_is() {
        let (tx, rx) = sender_with_channel();
        let mut state = FocusModeStatusShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &Fake::new(Ok(true)), t0, &mut timing);
        assert_eq!(apply_all(&rx, &mut state), 1);
        drive(
            &state,
            &tx,
            &Fake::new(Ok(true)),
            t0 + READ_INTERVAL,
            &mut timing,
        );
        assert!(rx.try_recv().is_err());
        drive(
            &state,
            &tx,
            &Fake::new(Ok(false)),
            t0 + READ_INTERVAL * 2,
            &mut timing,
        );
        assert_eq!(apply_all(&rx, &mut state), 1);
        assert_eq!(state.view, FocusView::Off);
    }

    #[test]
    fn failures_never_become_read() {
        for r in [
            Err(DeviceError::Unsupported("focus_mode_status")),
            Err(DeviceError::Internal("x")),
        ] {
            let (tx, rx) = sender_with_channel();
            let mut state = FocusModeStatusShowState::default();
            drive(
                &state,
                &tx,
                &Fake::new(r),
                Instant::now(),
                &mut Timing::default(),
            );
            apply_all(&rx, &mut state);
            assert!(matches!(
                state.view,
                FocusView::Unsupported | FocusView::Failed(_)
            ));
        }
    }

    #[test]
    fn previous_value_is_not_kept_after_failure() {
        let (tx, rx) = sender_with_channel();
        let mut state = FocusModeStatusShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &Fake::new(Ok(true)), t0, &mut timing);
        apply_all(&rx, &mut state);
        assert_eq!(state.view, FocusView::On);
        let failing = Fake::new(Err(DeviceError::Internal("x")));
        drive(&state, &tx, &failing, t0 + READ_INTERVAL, &mut timing);
        apply_all(&rx, &mut state);
        assert!(!matches!(state.view, FocusView::On | FocusView::Off));
    }

    #[test]
    fn unsupported_impl_reports_unsupported() {
        let (tx, rx) = sender_with_channel();
        let mut state = FocusModeStatusShowState::default();
        drive(
            &state,
            &tx,
            &Unsupported,
            Instant::now(),
            &mut Timing::default(),
        );
        apply_all(&rx, &mut state);
        assert_eq!(state.view, FocusView::Unsupported);
    }
}
