//! Phase 11 の拡充ループ（親 #106、Issue #458）「ライト（トーチ）」の effect ドライバ。
//! `fandhe_core::screens::flashlight_toggle`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::flashlight::Flashlight` で OS へ点灯 / 消灯を要求して結果を注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。
//!
//! # 流れ
//!
//! 1. `Requested`: `set_on` を呼ぶ（Windows はワーカー、他は同期）。成功なら `Confirming` へ、
//!    失敗なら読み戻した値つきで `finish`
//! 2. `Confirming`: `CONFIRM_POLL` ごとに `is_on` を読み、要求どおりなら成功。
//!    `CONFIRM_TIMEOUT`（最初に観測したフレームを起点に `Instant` で測る）を過ぎたら、
//!    `set_on` が成功でも失敗にする（嘘の成功の禁止）。期限後に届いた点灯は成功扱いにせず、
//!    次の定期 pull が状態として出すだけ
//! 3. `Idle`: `READ_INTERVAL` ごとに `is_on` を pull して反映（外部の点け消しへの追従）。
//!    送信を省く分岐でも次回の再描画を必ず予約する（契約表 4）
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS は AVFoundation の torch 実装（#460）、Android・Windows は後続の子
//! （#461・#459）が埋めるスタブ、それ以外は `UnsupportedFlashlight` 固定。Web は
//! `fandhe-web-dom` 側（#462）。Windows の最小化での消灯は #459 の担当。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::flashlight_toggle::{
    apply_read, confirm_target, finish, mark_applying, mark_confirming, take_request,
    FlashlightFailure, FlashlightToggleState,
};
#[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
use fandhe_device::flashlight::UnsupportedFlashlight;
use fandhe_device::flashlight::{Flashlight, CONFIRM_POLL, CONFIRM_TIMEOUT, READ_INTERVAL};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセス全体で 1 個）。
fn implementation() -> Arc<dyn Flashlight> {
    static IMPL: OnceLock<Arc<dyn Flashlight>> = OnceLock::new();
    IMPL.get_or_init(|| {
        #[cfg(target_os = "ios")]
        {
            super::ios_flashlight::implementation()
        }
        #[cfg(target_os = "android")]
        {
            super::android_flashlight::implementation()
        }
        #[cfg(target_os = "windows")]
        {
            super::windows_flashlight::implementation()
        }
        #[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
        {
            Arc::new(UnsupportedFlashlight)
        }
    })
    .clone()
}

/// `DeviceError` を画面の失敗種別へ写す。`Display` は機能名だけで OS のコードを含まない
/// （security.md A09）。
fn to_failure(e: DeviceError) -> FlashlightFailure {
    match e {
        DeviceError::Unsupported(_) => FlashlightFailure::Unsupported,
        other => FlashlightFailure::Failed(other.to_string()),
    }
}

/// 確認待ち 1 件の時刻記録。
struct Confirm {
    generation: u64,
    started: Instant,
    last_poll: Option<Instant>,
    /// 完了を送信済み。画面が反映するまでの間に二重送信しない。
    finished: bool,
}

/// 時刻の記録（プロセス全体で 1 つ）。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
    confirm: Option<Confirm>,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing {
    last_read: None,
    confirm: None,
});

/// `set_on` を呼んで結果を送る。Windows ではワーカー、他の OS では UI スレッドから呼ばれる。
fn run_set(
    light: &dyn Flashlight,
    sender: &CompletionSender<FlashlightToggleState>,
    generation: u64,
    on: bool,
) {
    match light.set_on(on) {
        Ok(()) => {
            let _ = sender.send(Box::new(move |s: &mut FlashlightToggleState| {
                mark_confirming(s, generation)
            }));
        }
        Err(e) => {
            // 失敗しても表示は読み戻した値で決める。読めなければ分からないまま（None）。
            let read = light.is_on().ok().flatten();
            let failure = to_failure(e);
            let _ = sender.send(Box::new(move |s: &mut FlashlightToggleState| {
                finish(s, generation, Err(failure.clone()), read)
            }));
        }
    }
}

/// 要求の発送・確認・定期 pull を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &FlashlightToggleState,
    sender: &CompletionSender<FlashlightToggleState>,
    light: Arc<dyn Flashlight>,
    now: Instant,
    timing: &mut Timing,
) {
    if let Some(req) = take_request(state) {
        // 次のフレームから `take_request` が `None` になり、二重に OS へ渡さない。
        let _ = sender.send(Box::new(mark_applying));
        #[cfg(target_os = "windows")]
        {
            let worker_sender = sender.clone();
            let worker_light = light.clone();
            let spawned = std::thread::Builder::new()
                .name("flashlight-set".to_string())
                .spawn(move || {
                    run_set(
                        worker_light.as_ref(),
                        &worker_sender,
                        req.generation,
                        req.on,
                    )
                });
            if spawned.is_err() {
                let failure = FlashlightFailure::Failed(
                    DeviceError::Internal("flashlight worker").to_string(),
                );
                let generation = req.generation;
                let _ = sender.send(Box::new(move |s: &mut FlashlightToggleState| {
                    finish(s, generation, Err(failure.clone()), None)
                }));
            }
        }
        #[cfg(not(target_os = "windows"))]
        run_set(light.as_ref(), sender, req.generation, req.on);
        return;
    }

    if let Some(req) = confirm_target(state) {
        let confirm = match &mut timing.confirm {
            Some(c) if c.generation == req.generation => c,
            slot => slot.insert(Confirm {
                generation: req.generation,
                started: now,
                last_poll: None,
                finished: false,
            }),
        };
        if !confirm.finished {
            let timed_out = now.saturating_duration_since(confirm.started) >= CONFIRM_TIMEOUT;
            let poll_due = confirm
                .last_poll
                .is_none_or(|p| now.saturating_duration_since(p) >= CONFIRM_POLL);
            if poll_due || timed_out {
                confirm.last_poll = Some(now);
                let generation = req.generation;
                let outcome = match light.is_on() {
                    Err(e) => Some((Err(to_failure(e)), None)),
                    // 期限判定を一致判定より先に行う。期限後に届いた点灯は成功にせず、
                    // 読み戻した値だけ状態表示へ反映する（契約表 3）。
                    Ok(other) if timed_out => Some((
                        Err(FlashlightFailure::Failed(
                            fandhe_core::screens::flashlight_toggle::NOT_CONFIRMED_REASON
                                .to_string(),
                        )),
                        other,
                    )),
                    Ok(Some(v)) if v == req.on => Some((Ok(()), Some(v))),
                    Ok(_) => None,
                };
                if let Some((result, read)) = outcome {
                    confirm.finished = true;
                    let _ = sender.send(Box::new(move |s: &mut FlashlightToggleState| {
                        finish(s, generation, result.clone(), read)
                    }));
                }
            }
        }
        sender.request_repaint_after(CONFIRM_POLL);
        return;
    }

    if state.phase != fandhe_core::screens::flashlight_toggle::FlashlightPhase::Idle {
        // Applying（Windows ワーカー実行中）。完了通知が再描画を起こす。
        return;
    }

    let due = timing
        .last_read
        .is_none_or(|prev| now.saturating_duration_since(prev) >= READ_INTERVAL);
    let result = if due {
        timing.last_read = Some(now);
        Some(light.is_on().map_err(to_failure))
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
    let _ = sender.send(Box::new(move |s: &mut FlashlightToggleState| {
        apply_read(s, result.clone())
    }));
}

/// `registry.rs` の `effect` 区画から `flashlight_toggle` 画面用に配線される effect ドライバ。
pub fn flashlight_toggle_effect(
    state: &FlashlightToggleState,
    sender: &CompletionSender<FlashlightToggleState>,
) {
    let mut timing = TIMING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(state, sender, implementation(), Instant::now(), &mut timing);
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;
    use fandhe_core::screens::flashlight_toggle::{
        error_text, status_text, turn_off, turn_on, FlashlightPhase, FAILED_PREFIX,
        NOT_CONFIRMED_REASON, ON_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::flashlight::UnsupportedFlashlight as Unsupported;
    use std::collections::VecDeque;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<FlashlightToggleState>>;

    #[derive(Clone, Copy)]
    enum Rd {
        Val(Option<bool>),
        Fail,
        Unsupported,
    }

    /// `set` の結果を差し込み、`is_on` は列を順に返して最後の要素を繰り返す。
    struct Fake {
        set: fn(bool) -> Result<(), DeviceError>,
        reads: Mutex<VecDeque<Rd>>,
        read_count: Mutex<usize>,
    }

    impl Fake {
        fn new(set: fn(bool) -> Result<(), DeviceError>, reads: &[Rd]) -> Arc<Self> {
            Arc::new(Self {
                set,
                reads: Mutex::new(reads.iter().copied().collect()),
                read_count: Mutex::new(0),
            })
        }
        fn count(&self) -> usize {
            *self.read_count.lock().unwrap()
        }
    }

    impl Flashlight for Fake {
        fn set_on(&self, on: bool) -> Result<(), DeviceError> {
            (self.set)(on)
        }
        fn is_on(&self) -> Result<Option<bool>, DeviceError> {
            *self.read_count.lock().unwrap() += 1;
            let mut q = self.reads.lock().unwrap();
            let r = if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                *q.front().unwrap()
            };
            match r {
                Rd::Val(v) => Ok(v),
                Rd::Fail => Err(DeviceError::Internal("flashlight")),
                Rd::Unsupported => Err(DeviceError::Unsupported("flashlight")),
            }
        }
    }

    fn ok(_: bool) -> Result<(), DeviceError> {
        Ok(())
    }

    struct Harness {
        state: FlashlightToggleState,
        tx: CompletionSender<FlashlightToggleState>,
        rx: Rx,
        timing: Timing,
        t0: Instant,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let (tx, rx) = std::sync::mpsc::channel();
            Self {
                state: FlashlightToggleState::default(),
                tx: CompletionSender::new(tx, ctx),
                rx,
                timing: Timing::default(),
                t0: Instant::now(),
            }
        }
        /// 1 フレーム: drive して届いた完了を適用し、送信数を返す。
        fn frame(&mut self, light: &Arc<Fake>, at_ms: u64) -> usize {
            let l: Arc<dyn Flashlight> = light.clone();
            drive(
                &self.state,
                &self.tx,
                l,
                self.t0 + Duration::from_millis(at_ms),
                &mut self.timing,
            );
            let mut n = 0;
            while let Ok(c) = self.rx.try_recv() {
                c(&mut self.state);
                n += 1;
            }
            n
        }
    }

    #[test]
    fn on_then_off_roundtrip() {
        let mut h = Harness::new();
        let light = Fake::new(ok, &[Rd::Val(Some(true)), Rd::Val(Some(false))]);
        turn_on(&mut h.state);
        h.frame(&light, 0);
        assert_eq!(h.state.phase, FlashlightPhase::Confirming);
        h.frame(&light, 10);
        assert_eq!(h.state.phase, FlashlightPhase::Idle);
        assert_eq!(status_text(&h.state), ON_STATUS);
        turn_off(&mut h.state);
        h.frame(&light, 2000);
        h.frame(&light, 2010);
        assert_eq!(h.state.phase, FlashlightPhase::Idle);
        assert_eq!(h.state.on, Some(false));
        assert_eq!(error_text(&h.state), "");
    }

    #[test]
    fn ok_but_never_lights_fails_at_timeout_and_late_value_is_not_success() {
        let mut h = Harness::new();
        let light = Fake::new(ok, &[Rd::Val(Some(false))]);
        turn_on(&mut h.state);
        h.frame(&light, 0);
        for ms in (0..=900).step_by(100) {
            h.frame(&light, ms);
            assert_eq!(h.state.phase, FlashlightPhase::Confirming, "at {ms}");
        }
        h.frame(&light, 1000);
        assert_eq!(h.state.phase, FlashlightPhase::Idle);
        let expected = format!("{FAILED_PREFIX}{NOT_CONFIRMED_REASON}");
        assert_eq!(error_text(&h.state), expected);
        assert_ne!(status_text(&h.state), ON_STATUS);
        // 期限後に点灯が見えても、状態として出るだけで失敗は残る。
        let late = Fake::new(ok, &[Rd::Val(Some(true))]);
        h.frame(&late, 3000);
        assert_eq!(h.state.on, Some(true));
        assert_eq!(error_text(&h.state), expected);
    }

    #[test]
    fn match_on_first_poll_after_deadline_is_failure() {
        let mut h = Harness::new();
        let light = Fake::new(ok, &[Rd::Val(Some(false)), Rd::Val(Some(true))]);
        turn_on(&mut h.state);
        h.frame(&light, 0);
        h.frame(&light, 10);
        assert_eq!(h.state.phase, FlashlightPhase::Confirming);
        // 期限後の最初の確認で点灯が見えても成功にしない。
        h.frame(&light, 1100);
        assert_eq!(h.state.phase, FlashlightPhase::Idle);
        assert_eq!(h.state.on, Some(true));
        let expected = format!("{FAILED_PREFIX}{NOT_CONFIRMED_REASON}");
        assert_eq!(error_text(&h.state), expected);
    }

    #[test]
    fn set_failures_are_failures_with_read_back_value() {
        let cases: [fn(bool) -> Result<(), DeviceError>; 3] = [
            |_| Err(DeviceError::PermissionDenied("flashlight")),
            |_| Err(DeviceError::Cancelled("flashlight")),
            |_| Err(DeviceError::Internal("flashlight")),
        ];
        for set in cases {
            let mut h = Harness::new();
            let light = Fake::new(set, &[Rd::Val(Some(false))]);
            turn_on(&mut h.state);
            h.frame(&light, 0);
            assert_eq!(h.state.phase, FlashlightPhase::Idle);
            assert!(error_text(&h.state).starts_with(FAILED_PREFIX));
            assert_eq!(h.state.on, Some(false));
            assert_ne!(status_text(&h.state), ON_STATUS);
        }
    }

    #[test]
    fn confirm_read_error_is_failure() {
        let mut h = Harness::new();
        let light = Fake::new(ok, &[Rd::Fail]);
        turn_on(&mut h.state);
        h.frame(&light, 0);
        h.frame(&light, 10);
        assert_eq!(h.state.phase, FlashlightPhase::Idle);
        assert!(error_text(&h.state).starts_with(FAILED_PREFIX));
        assert_eq!(h.state.on, None);
    }

    #[test]
    fn unsupported_is_not_success() {
        let mut h = Harness::new();
        let light = Fake::new(
            |_| Err(DeviceError::Unsupported("flashlight")),
            &[Rd::Unsupported],
        );
        turn_on(&mut h.state);
        h.frame(&light, 0);
        assert_eq!(status_text(&h.state), UNSUPPORTED_STATUS);
        // 実際の Unsupported 実装でも同じ。
        let mut h2 = Harness::new();
        turn_on(&mut h2.state);
        let l: Arc<dyn Flashlight> = Arc::new(Unsupported);
        drive(&h2.state, &h2.tx, l, h2.t0, &mut h2.timing);
        while let Ok(c) = h2.rx.try_recv() {
            c(&mut h2.state);
        }
        assert_eq!(status_text(&h2.state), UNSUPPORTED_STATUS);
    }

    #[test]
    fn idle_pulls_on_interval_and_skips_send_when_unchanged() {
        let mut h = Harness::new();
        let light = Fake::new(ok, &[Rd::Val(Some(true))]);
        assert_eq!(h.frame(&light, 0), 1);
        assert_eq!(h.state.on, Some(true));
        // 間隔の途中は読まない・送らない。
        assert_eq!(h.frame(&light, 500), 0);
        assert_eq!(light.count(), 1);
        // 間隔が来たら読むが、値が同じなら送らない。
        assert_eq!(h.frame(&light, 1000), 0);
        assert_eq!(light.count(), 2);
    }

    #[test]
    fn idle_read_failure_drops_value() {
        let mut h = Harness::new();
        let light = Fake::new(ok, &[Rd::Val(Some(true)), Rd::Fail]);
        h.frame(&light, 0);
        h.frame(&light, 1000);
        assert_eq!(h.state.on, None);
        assert!(error_text(&h.state).starts_with(FAILED_PREFIX));
    }

    #[test]
    fn default_impl_on_host_is_unsupported() {
        #[cfg(not(any(target_os = "ios", target_os = "android", target_os = "windows")))]
        {
            let f = implementation();
            assert!(matches!(f.set_on(true), Err(DeviceError::Unsupported(_))));
            assert!(matches!(f.is_on(), Err(DeviceError::Unsupported(_))));
        }
    }
}
