//! Phase 11 の拡充ループ（親 #1132、Issue #1308）「アクセシビリティ設定の追従」の effect ドライバ。
//! `fandhe_device::accessibility_prefs::AccessibilityPrefsSource` を一定間隔で読み、結果を
//! `fandhe_core::screens::accessibility_prefs_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。
//!
//! **間隔つき pull**（`network_status.rs` と同型）: 最終読取から `READ_INTERVAL` 未満なら OS を
//! 読まない。丸を動かしているあいだは毎フレーム送信して連続描画にする。止めているとき
//! （視差軽減オン）は状態が変わらなければ送信を省くが、次の読み取りまでの再描画を必ず予約する
//! （入力が無いと egui が描画せずポーリングが止まる、契約表 4）。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS は `ios_accessibility_prefs.rs`
//! （#1309）、Android は `android_accessibility_prefs.rs`（#1310）が返す。Web は
//! `fandhe-web-dom` 側（#1311）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::accessibility_prefs_show::{
    advance_phase, animates, apply_read, AccessibilityPrefsShowState, Prefs, ReadFailure,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::accessibility_prefs::UnsupportedAccessibilityPrefs;
use fandhe_device::accessibility_prefs::{
    AccessibilityPrefs, AccessibilityPrefsSource, READ_INTERVAL,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 1 フレームで進める時間の上限（バックグラウンド復帰などの飛び対策）。
const MAX_DT: f32 = 0.1;
/// 初回フレームの時間幅。
const DEFAULT_DT: f32 = 1.0 / 60.0;

/// 現在のプラットフォームに応じた実装を返す。
fn implementation() -> Arc<dyn AccessibilityPrefsSource> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(super::windows_accessibility_prefs::WindowsAccessibilityPrefs)
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_accessibility_prefs::AppleAccessibilityPrefs)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_accessibility_prefs::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_accessibility_prefs::implementation()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Arc::new(UnsupportedAccessibilityPrefs)
    }
}

/// 契約の結果を画面の入力へ写す。失敗は固定文言へ畳み、OS 固有値を画面へ漏らさない
/// （security.md A09）。
fn to_result(r: Result<AccessibilityPrefs, DeviceError>) -> Result<Prefs, ReadFailure> {
    match r {
        Ok(p) => Ok(Prefs {
            reduce_motion: p.reduce_motion,
            increase_contrast: p.increase_contrast,
        }),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(ReadFailure::Failed("設定の取得".to_string())),
    }
}

/// 最終読取・最終フレームの時刻（プロセス全体で 1 つ）。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
    last_tick: Option<Instant>,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing {
    last_read: None,
    last_tick: None,
});

/// 間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &AccessibilityPrefsShowState,
    sender: &CompletionSender<AccessibilityPrefsShowState>,
    src: &dyn AccessibilityPrefsSource,
    now: Instant,
    timing: &mut Timing,
) {
    let dt = match timing.last_tick {
        Some(prev) => now
            .saturating_duration_since(prev)
            .as_secs_f32()
            .clamp(0.0, MAX_DT),
        None => DEFAULT_DT,
    };
    timing.last_tick = Some(now);

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

    let step = move |s: &mut AccessibilityPrefsShowState| {
        if let Some(r) = result.clone() {
            apply_read(s, r);
        }
        if animates(s) {
            advance_phase(s, dt);
        }
    };
    let mut next = state.clone();
    step(&mut next);
    // 動かしているあいだは連続描画のため常に送る。止めているときだけ同値の送信を省く。
    if !animates(&next) && next == *state {
        return;
    }
    let _ = sender.send(Box::new(step));
}

/// `registry.rs` の `effect` 区画から `accessibility_prefs_show` 画面用に配線される effect ドライバ。
pub fn accessibility_prefs_show_effect(
    state: &AccessibilityPrefsShowState,
    sender: &CompletionSender<AccessibilityPrefsShowState>,
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
    use fandhe_core::screens::accessibility_prefs_show::{status_text, PrefsView};
    use fandhe_device::accessibility_prefs::UnsupportedAccessibilityPrefs as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<AccessibilityPrefsShowState>>;

    fn sender_with_channel() -> (CompletionSender<AccessibilityPrefsShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<AccessibilityPrefs, DeviceError>,
        reads: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<AccessibilityPrefs, DeviceError>) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
            }
        }
        fn prefs(rm: Option<bool>, ic: Option<bool>) -> Self {
            Self::new(Ok(AccessibilityPrefs {
                reduce_motion: rm,
                increase_contrast: ic,
            }))
        }
    }

    impl AccessibilityPrefsSource for Fake {
        fn read(&self) -> Result<AccessibilityPrefs, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &self.result {
                Ok(p) => Ok(*p),
                Err(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Err(_) => Err(DeviceError::Internal("x")),
            }
        }
    }

    fn apply_all(rx: &Rx, s: &mut AccessibilityPrefsShowState) -> usize {
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
        let fake = Fake::prefs(Some(false), Some(false));
        let state = AccessibilityPrefsShowState::default();
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
    fn stopped_ball_with_same_value_is_not_sent() {
        let (tx, rx) = sender_with_channel();
        let fake = Fake::prefs(Some(true), Some(false));
        let mut state = AccessibilityPrefsShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &fake, t0, &mut timing);
        assert_eq!(apply_all(&rx, &mut state), 1);
        let phase = state.phase;
        // 読まない間隔内・同値の読み取り直後のどちらも送信しない。
        drive(
            &state,
            &tx,
            &fake,
            t0 + Duration::from_millis(300),
            &mut timing,
        );
        drive(&state, &tx, &fake, t0 + READ_INTERVAL, &mut timing);
        assert!(rx.try_recv().is_err());
        assert_eq!(state.phase, phase);
    }

    #[test]
    fn moving_ball_is_sent_every_frame() {
        let (tx, rx) = sender_with_channel();
        let fake = Fake::prefs(Some(false), Some(false));
        let mut state = AccessibilityPrefsShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        for i in 0..3u32 {
            drive(
                &state,
                &tx,
                &fake,
                t0 + Duration::from_millis(16 * i as u64),
                &mut timing,
            );
            assert_eq!(apply_all(&rx, &mut state), 1);
        }
        assert!(state.phase > 0.0);
    }

    #[test]
    fn ball_moves_again_after_reduce_motion_is_turned_off() {
        let (tx, rx) = sender_with_channel();
        let mut state = AccessibilityPrefsShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &Fake::prefs(Some(true), None), t0, &mut timing);
        apply_all(&rx, &mut state);
        assert!(!animates(&state));
        drive(
            &state,
            &tx,
            &Fake::prefs(Some(false), None),
            t0 + READ_INTERVAL,
            &mut timing,
        );
        apply_all(&rx, &mut state);
        assert!(animates(&state));
        let phase = state.phase;
        drive(
            &state,
            &tx,
            &Fake::prefs(Some(false), None),
            t0 + READ_INTERVAL + Duration::from_millis(16),
            &mut timing,
        );
        apply_all(&rx, &mut state);
        assert!(state.phase != phase);
    }

    #[test]
    fn failures_never_become_on() {
        for r in [
            Err(DeviceError::Unsupported("accessibility_prefs")),
            Err(DeviceError::Internal("x")),
        ] {
            let (tx, rx) = sender_with_channel();
            let mut state = AccessibilityPrefsShowState::default();
            drive(
                &state,
                &tx,
                &Fake::new(r),
                Instant::now(),
                &mut Timing::default(),
            );
            apply_all(&rx, &mut state);
            assert!(!status_text(&state).contains("オン"));
            assert!(matches!(
                state.view,
                PrefsView::Unsupported | PrefsView::Failed(_)
            ));
            assert!(animates(&state));
        }
    }

    #[test]
    fn previous_value_is_not_kept_after_failure() {
        let (tx, rx) = sender_with_channel();
        let mut state = AccessibilityPrefsShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(
            &state,
            &tx,
            &Fake::prefs(Some(true), Some(true)),
            t0,
            &mut timing,
        );
        apply_all(&rx, &mut state);
        let failing = Fake::new(Err(DeviceError::Internal("x")));
        drive(&state, &tx, &failing, t0 + READ_INTERVAL, &mut timing);
        apply_all(&rx, &mut state);
        assert!(!matches!(state.view, PrefsView::Read(_)));
    }

    #[test]
    fn unsupported_impl_reports_unsupported() {
        let (tx, rx) = sender_with_channel();
        let mut state = AccessibilityPrefsShowState::default();
        drive(
            &state,
            &tx,
            &Unsupported,
            Instant::now(),
            &mut Timing::default(),
        );
        apply_all(&rx, &mut state);
        assert_eq!(state.view, PrefsView::Unsupported);
    }
}
