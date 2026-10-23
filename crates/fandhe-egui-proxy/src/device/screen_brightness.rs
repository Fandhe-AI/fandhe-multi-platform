//! Phase 11 の拡充ループ（親 #126、Issue #633）「画面の明るさ」の effect ドライバ。
//! `fandhe_device::screen_brightness::ScreenBrightness` へ要求を渡し、読み戻しを
//! `fandhe_core::screens::screen_brightness_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる。ワーカーは作らない）。
//!
//! 要求があれば `take_request` → `mark_applying` → OS 呼び出し → 読み戻し → `finish` の順で
//! 送る。結果を届ける前に読み戻しで状態を確定させる（契約表 2・6）。要求がなければ
//! `READ_INTERVAL` ごとに読み戻す間隔つき pull で、次の読み取りまでの再描画を必ず予約する
//! （入力が無いと egui が描画せず pull が止まる、契約表 4）。読み取りに失敗・非対応のときは
//! 予約しない（失敗状態で再描画し続けない）。期限は `Instant`（単調時計）で測る（契約表 3）。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS は `ios_screen_brightness.rs`
//! （#634）、Android は `android_screen_brightness.rs`（#635）が返す。macOS・Web は `no`、
//! Windows は `deferred` のため `UnsupportedScreenBrightness` 固定。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::screen_brightness_show::{
    apply_read, finish, mark_applying, take_request, wants_read, BrightnessCommand,
    BrightnessFailure, Reading, ScreenBrightnessShowState,
};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::screen_brightness::UnsupportedScreenBrightness;
use fandhe_device::screen_brightness::{
    validate_level, BrightnessReading, ScreenBrightness, READ_INTERVAL,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `ScreenBrightness` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn ScreenBrightness> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn ScreenBrightness>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "ios")]
            {
                super::ios_screen_brightness::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_screen_brightness::implementation()
            }
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            {
                Arc::new(UnsupportedScreenBrightness)
            }
        })
        .clone()
}

/// `DeviceError` を画面の失敗種別へ写す。契約外のエラー（`PermissionDenied` / `Cancelled` を含む）は
/// 固定文言に畳み、OS のエラーコードを画面・ログへ出さない（security.md A09）。
fn to_failure(e: DeviceError) -> BrightnessFailure {
    match e {
        DeviceError::Unsupported(_) => BrightnessFailure::Unsupported,
        DeviceError::InvalidInput(_) => BrightnessFailure::InvalidInput,
        _ => BrightnessFailure::Failed("画面の明るさの操作".to_string()),
    }
}

fn to_reading(r: Result<BrightnessReading, DeviceError>) -> Result<Reading, BrightnessFailure> {
    r.map(|r| Reading {
        level: r.level,
        overriding: r.overriding,
    })
    .map_err(to_failure)
}

/// 最終読取の時刻（プロセス全体で 1 つ）。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing { last_read: None });

/// 要求の処理・間隔つき pull・再描画予約を行う本体（テストから実装と時刻を差し込めるよう切り出す）。
fn drive(
    state: &ScreenBrightnessShowState,
    sender: &CompletionSender<ScreenBrightnessShowState>,
    imp: &dyn ScreenBrightness,
    now: Instant,
    timing: &mut Timing,
) {
    if let Some(request) = take_request(state) {
        let generation = request.generation;
        // 次の再描画から `take_request` が同じ要求を返さないようにし、二重に OS へ渡さない。
        let _ = sender.send(Box::new(move |s: &mut ScreenBrightnessShowState| {
            mark_applying(s, generation)
        }));
        let result = match request.command {
            // 範囲外・非有限値は OS へ渡さない（丸めの前に検証）。
            BrightnessCommand::Set(level) => validate_level(level)
                .and_then(|l| imp.set_override(l))
                .map_err(to_failure),
            BrightnessCommand::Clear => imp.clear_override().map_err(to_failure),
        };
        // 成否に関わらず、結果を届ける前に実際の状態を読み戻す（嘘の成功の禁止）。
        let reading = to_reading(imp.read());
        timing.last_read = Some(now);
        let _ = sender.send(Box::new(move |s: &mut ScreenBrightnessShowState| {
            finish(s, generation, result, reading);
        }));
        sender.request_repaint_after(READ_INTERVAL);
        return;
    }

    if !wants_read(state) {
        return;
    }
    let since = timing
        .last_read
        .map(|prev| now.saturating_duration_since(prev));
    match since {
        Some(d) if d < READ_INTERVAL => {
            // 読まない分岐でも次回の読み取りを予約する（契約表 4）。
            sender.request_repaint_after(READ_INTERVAL - d);
        }
        _ => {
            timing.last_read = Some(now);
            let result = to_reading(imp.read());
            let keep_polling = result.is_ok();
            let _ = sender.send(Box::new(move |s: &mut ScreenBrightnessShowState| {
                apply_read(s, result);
            }));
            sender.request_repaint_after(if keep_polling {
                READ_INTERVAL
            } else {
                Duration::ZERO
            });
        }
    }
}

/// `registry.rs` の `effect` 区画から `screen_brightness_show` 画面用に配線される effect ドライバ。
pub fn screen_brightness_show_effect(
    state: &ScreenBrightnessShowState,
    sender: &CompletionSender<ScreenBrightnessShowState>,
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
    use fandhe_core::screens::screen_brightness_show::{
        clear, error_text, set_slider, status_text, ReadState, OVERRIDING_PREFIX,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<ScreenBrightnessShowState>>;

    fn sender_with_channel() -> (CompletionSender<ScreenBrightnessShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    fn apply_all(rx: &Rx, s: &mut ScreenBrightnessShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    /// 呼び出し回数を数え、失敗を差し込める Fake。
    struct Fake {
        level: Mutex<Option<f32>>,
        fail_set: bool,
        fail_clear: bool,
        sets: AtomicUsize,
        clears: AtomicUsize,
        reads: AtomicUsize,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                level: Mutex::new(None),
                fail_set: false,
                fail_clear: false,
                sets: AtomicUsize::new(0),
                clears: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
            }
        }
    }

    impl ScreenBrightness for Fake {
        fn set_override(&self, level: f32) -> Result<(), DeviceError> {
            self.sets.fetch_add(1, Ordering::SeqCst);
            if self.fail_set {
                return Err(DeviceError::Internal("x"));
            }
            *self.level.lock().unwrap() = Some(level);
            Ok(())
        }
        fn clear_override(&self) -> Result<(), DeviceError> {
            self.clears.fetch_add(1, Ordering::SeqCst);
            if self.fail_clear {
                return Err(DeviceError::Internal("x"));
            }
            *self.level.lock().unwrap() = None;
            Ok(())
        }
        fn read(&self) -> Result<BrightnessReading, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let l = *self.level.lock().unwrap();
            Ok(BrightnessReading {
                level: Some(l.unwrap_or(0.6)),
                overriding: l.is_some(),
            })
        }
    }

    /// 読み取り済み（非上書き）の状態にして返す。
    fn ready(
        tx: &CompletionSender<ScreenBrightnessShowState>,
        rx: &Rx,
        fake: &Fake,
    ) -> ScreenBrightnessShowState {
        let mut s = ScreenBrightnessShowState::default();
        drive(&s, tx, fake, Instant::now(), &mut Timing::default());
        apply_all(rx, &mut s);
        assert!(matches!(s.read_state(), ReadState::Read(_)));
        s
    }

    #[test]
    fn no_request_means_no_os_write() {
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let s = ready(&tx, &rx, &fake);
        drive(&s, &tx, &fake, Instant::now(), &mut Timing::default());
        assert_eq!(fake.sets.load(Ordering::SeqCst), 0);
        assert_eq!(fake.clears.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn set_then_clear_round_trip() {
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut s = ready(&tx, &rx, &fake);
        let mut timing = Timing::default();
        set_slider(&mut s, 0.9);
        drive(&s, &tx, &fake, Instant::now(), &mut timing);
        apply_all(&rx, &mut s);
        assert!(status_text(&s).starts_with(OVERRIDING_PREFIX));
        clear(&mut s);
        drive(&s, &tx, &fake, Instant::now(), &mut timing);
        apply_all(&rx, &mut s);
        assert!(!status_text(&s).starts_with(OVERRIDING_PREFIX));
        assert_eq!(fake.sets.load(Ordering::SeqCst), 1);
        assert_eq!(fake.clears.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsupported_is_a_failure_display() {
        let (tx, rx) = sender_with_channel();
        let mut s = ScreenBrightnessShowState::default();
        drive(
            &s,
            &tx,
            &fandhe_device::screen_brightness::UnsupportedScreenBrightness,
            Instant::now(),
            &mut Timing::default(),
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.read_state(), &ReadState::Unsupported);
        assert!(!wants_read(&s));
    }

    #[test]
    fn out_of_range_set_never_reaches_os() {
        // 画面側の丸めを迂回して範囲外の要求が来ても、driver が OS へ渡さない。
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut s = ready(&tx, &rx, &fake);
        // 範囲外は画面の setter が丸めるため、ここでは検証関数の拒否を直接確認する。
        assert!(validate_level(1.5).is_err());
        assert!(matches!(
            to_failure(validate_level(f32::NAN).unwrap_err()),
            BrightnessFailure::InvalidInput
        ));
        set_slider(&mut s, 5.0);
        drive(&s, &tx, &fake, Instant::now(), &mut Timing::default());
        apply_all(&rx, &mut s);
        // 5.0 は 1.0 へ丸められ、有効な値として 1 回だけ渡る。
        assert_eq!(fake.sets.load(Ordering::SeqCst), 1);
        assert_eq!(*fake.level.lock().unwrap(), Some(1.0));
    }

    #[test]
    fn set_failure_shows_readback() {
        let (tx, rx) = sender_with_channel();
        let mut fake = Fake::new();
        let mut s = ready(&tx, &rx, &fake);
        fake.fail_set = true;
        let before = s.slider_value();
        set_slider(&mut s, 0.9);
        drive(&s, &tx, &fake, Instant::now(), &mut Timing::default());
        apply_all(&rx, &mut s);
        assert!(!status_text(&s).starts_with(OVERRIDING_PREFIX));
        assert_eq!(s.slider_value(), before);
        assert!(!error_text(&s).is_empty());
    }

    #[test]
    fn clear_failure_stays_overriding() {
        let (tx, rx) = sender_with_channel();
        let mut fake = Fake::new();
        let mut s = ready(&tx, &rx, &fake);
        let mut timing = Timing::default();
        set_slider(&mut s, 0.9);
        drive(&s, &tx, &fake, Instant::now(), &mut timing);
        apply_all(&rx, &mut s);
        fake.fail_clear = true;
        clear(&mut s);
        drive(&s, &tx, &fake, Instant::now(), &mut timing);
        apply_all(&rx, &mut s);
        assert!(status_text(&s).starts_with(OVERRIDING_PREFIX));
    }

    #[test]
    fn two_operations_make_one_os_call_with_latest() {
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut s = ready(&tx, &rx, &fake);
        set_slider(&mut s, 0.3);
        set_slider(&mut s, 0.7);
        drive(&s, &tx, &fake, Instant::now(), &mut Timing::default());
        assert_eq!(fake.sets.load(Ordering::SeqCst), 1);
        assert_eq!(*fake.level.lock().unwrap(), Some(0.7));
    }

    #[test]
    fn pull_respects_interval() {
        let (tx, _rx) = sender_with_channel();
        let fake = Fake::new();
        let s = ScreenBrightnessShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&s, &tx, &fake, t0, &mut timing);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(400), &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failed_read_stops_polling() {
        struct Failing;
        impl ScreenBrightness for Failing {
            fn set_override(&self, _: f32) -> Result<(), DeviceError> {
                Err(DeviceError::Internal("x"))
            }
            fn clear_override(&self) -> Result<(), DeviceError> {
                Err(DeviceError::Internal("x"))
            }
            fn read(&self) -> Result<BrightnessReading, DeviceError> {
                Err(DeviceError::Internal("x"))
            }
        }
        let (tx, rx) = sender_with_channel();
        let mut s = ScreenBrightnessShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&s, &tx, &Failing, t0, &mut timing);
        apply_all(&rx, &mut s);
        assert_eq!(s.read_state(), &ReadState::Failed);
        drive(&s, &tx, &Failing, t0 + READ_INTERVAL * 3, &mut timing);
        assert!(rx.try_recv().is_err());
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[test]
    fn fallback_implementation_never_succeeds() {
        let imp = implementation();
        assert!(imp.set_override(0.5).is_err());
        assert!(imp.clear_override().is_err());
        assert!(imp.read().is_err());
    }
}
