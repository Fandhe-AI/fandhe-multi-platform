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
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS・Android・Windows は後続の子
//! （#460・#461・#459）が埋めるスタブ、それ以外は `UnsupportedFlashlight` 固定。Web は
//! `fandhe-web-dom` 側（#462）。
//!
//! Windows は最小化を検知したら（専用ウォッチャースレッドが HWND の `IsIconic` を定期確認する。
//! 描画フレームに依存しない）、ワーカーで `set_on(false)` を直列に実行する。最小化中は
//! eframe が次フレームを描かないため、状態経由（次フレームの `take_request`）にすると消灯が
//! 復帰まで遅れる。ワーカーは常に 1 本（`WIN`）。点灯処理中（Requested / Applying）に最小化を
//! 観測したら保留として記録し、その点灯ワーカーが終わった直後に同じスレッドで消灯する。
//! 消灯に失敗したら読み戻した値と失敗を状態へ通知し、最小化が続く間は `READ_INTERVAL` ごとに
//! ワーカー自身が再試行する（フレームに依存しない）。戻っても点け直さない。

#[cfg(target_os = "windows")]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::flashlight_toggle::{
    apply_read, confirm_target, finish, mark_applying, mark_confirming, take_request,
    FlashlightFailure, FlashlightPhase, FlashlightToggleState,
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

/// Windows のライト操作ワーカーの直列化状態（`active`: 走行中、`pending`: 走行中に最小化を観測）。
#[cfg(target_os = "windows")]
struct WinSync {
    active: bool,
    pending: bool,
}

#[cfg(target_os = "windows")]
static WIN: Mutex<WinSync> = Mutex::new(WinSync {
    active: false,
    pending: false,
});

/// 最後に観測した最小化状態（ウォッチャーが `IsIconic` で更新し、ワーカーが読む）。
#[cfg(target_os = "windows")]
static MINIMIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "windows")]
fn win_lock() -> std::sync::MutexGuard<'static, WinSync> {
    WIN.lock().unwrap_or_else(|p| p.into_inner())
}

/// ワーカーの実行権を取る。走行中なら `false`（呼び出し側は次フレームで再試行する）。
#[cfg(target_os = "windows")]
fn win_begin() -> bool {
    let mut w = win_lock();
    !std::mem::replace(&mut w.active, true)
}

/// 点灯ワーカーの終了処理。走行中に最小化を観測していたなら `true`（実行権は保持したまま、
/// 呼び出し側が消灯する）。判定と `active` の解放は同じロック内で行う。
#[cfg(target_os = "windows")]
fn win_end_set() -> bool {
    let mut w = win_lock();
    let again = std::mem::take(&mut w.pending) || MINIMIZED.load(Ordering::Acquire);
    if !again {
        w.active = false;
    }
    again
}

/// 最小化消灯の結果を状態へ反映する。読み戻した値をそのまま `apply_read` に渡すので、
/// `Unsupported`・読み取り失敗は成功値で上書きされない。消灯失敗かつ消灯未確認なら失敗を表示する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn apply_minimize_off(
    s: &mut FlashlightToggleState,
    set_ok: bool,
    read: Result<Option<bool>, FlashlightFailure>,
) {
    let still_on = !set_ok && read != Ok(Some(false));
    apply_read(s, read);
    if still_on {
        s.error = format!(
            "{}{}",
            fandhe_core::screens::flashlight_toggle::FAILED_PREFIX,
            fandhe_core::screens::flashlight_toggle::READ_FAILED
        );
    }
}

/// 最小化中の消灯（実行権を保持して呼ぶ）。失敗は読み戻した値つきで状態へ通知し、
/// 最小化が続く間は `READ_INTERVAL` ごとに再試行する。終わったら実行権を手放す。
#[cfg(target_os = "windows")]
fn minimize_off(light: &dyn Flashlight, sender: &CompletionSender<FlashlightToggleState>) {
    loop {
        // 成功時も読み戻した値を通知する（Lamp 未保持の成功で確定済みの Unsupported を消さない）。
        let set_ok = light.set_on(false).is_ok();
        let read = light.is_on().map_err(to_failure);
        let _ = sender.send(Box::new(move |s: &mut FlashlightToggleState| {
            if s.phase == FlashlightPhase::Idle {
                apply_minimize_off(s, set_ok, read.clone());
            }
        }));
        if set_ok {
            break;
        }
        sender.request_repaint_after(READ_INTERVAL);
        std::thread::sleep(READ_INTERVAL);
        if !MINIMIZED.load(Ordering::Acquire) {
            break;
        }
    }
    let mut w = win_lock();
    w.active = false;
    w.pending = false;
}

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
        // Windows は最小化消灯ワーカーが走行中なら発送を見送り、次フレームで再試行する。
        #[cfg(target_os = "windows")]
        if !win_begin() {
            sender.request_repaint_after(CONFIRM_POLL);
            return;
        }
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
                    );
                    // 点灯処理中に最小化されていたら、同じスレッドで直列に消灯する。
                    if win_end_set() {
                        minimize_off(worker_light.as_ref(), &worker_sender);
                    }
                });
            if spawned.is_err() {
                win_lock().active = false;
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

/// 最小化中に消灯を要求すべきか。消灯を確認できている（`Idle` で `Some(false)`）とき以外は要求する。
/// `None`（読み取り不能）は消灯の確認ではないため、Lamp を保持したまま点灯している場合に備えて試行する。
/// 進行中の点灯（Requested / Applying / Confirming）は結果が点灯になりうるため。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn should_turn_off_on_minimize(state: &FlashlightToggleState, minimized: bool) -> bool {
    minimized && !(state.phase == FlashlightPhase::Idle && state.on == Some(false))
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
    #[cfg(target_os = "windows")]
    {
        // 最小化の検知はフレームに依存させない（最小化中は eframe が描画を止め、この関数が呼ばれない）。
        CONFIRMED_OFF.store(!should_turn_off_on_minimize(state, true), Ordering::Release);
        start_minimize_watcher(sender);
    }
}

/// 最小化消灯ワーカーの起動を試みる。走行中のワーカーがあれば保留として記録して `true`、
/// 新規に起動できても `true`、起動に失敗したら `false`（呼び出し側が次回再試行する）。
#[cfg(target_os = "windows")]
fn request_minimize_off(sender: &CompletionSender<FlashlightToggleState>) -> bool {
    let mut w = win_lock();
    if w.active {
        w.pending = true;
        return true;
    }
    w.active = true;
    drop(w);
    let worker_sender = sender.clone();
    let light = implementation();
    let spawned = std::thread::Builder::new()
        .name("flashlight-minimize-off".to_string())
        .spawn(move || minimize_off(light.as_ref(), &worker_sender));
    if spawned.is_err() {
        win_lock().active = false;
        let _ = sender.send(Box::new(|s: &mut FlashlightToggleState| {
            if s.phase == FlashlightPhase::Idle {
                apply_read(s, Err(FlashlightFailure::Failed(String::new())));
            }
        }));
        return false;
    }
    true
}

/// 最後のフレームで消灯を確認できていたか。ウォッチャーが最小化の立ち上がりで消灯を省く判定に使う
/// （[`should_turn_off_on_minimize`] と同じ条件。最小化中はフレームが止まるため静的に持つ）。
#[cfg(target_os = "windows")]
static CONFIRMED_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 最小化ウォッチャー（プロセスに 1 本）を起動する。HWND の `IsIconic` を `READ_INTERVAL` 未満の
/// 間隔でポーリングするため、eframe の描画停止に左右されない。最小化の立ち上がりごとに
/// [`request_minimize_off`] を呼ぶ（以降の再試行は消灯ワーカー自身が `MINIMIZED` を見て行う）。
/// ウィンドウ未取得なら何もせず、次フレームで再度試みる。
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn start_minimize_watcher(sender: &CompletionSender<FlashlightToggleState>) {
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if super::windows_share::window().is_none() || STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("flashlight-minimize-watch".to_string())
        .spawn(move || {
            let mut was_minimized = false;
            loop {
                if let Some(hwnd) = super::windows_share::window() {
                    // SAFETY: `hwnd` は `remember_window` が控えたメインウィンドウで、プロセス生存中は有効。
                    // `IsIconic` は無効な HWND でも FALSE を返すだけで副作用がない。
                    let minimized = unsafe { IsIconic(hwnd) }.as_bool();
                    MINIMIZED.store(minimized, Ordering::Release);
                    if minimized && !was_minimized && !CONFIRMED_OFF.load(Ordering::Acquire) {
                        // 起動に失敗したら立ち上がりを消費せず、次の周期で再試行する。
                        was_minimized = request_minimize_off(&sender);
                    } else {
                        was_minimized = minimized;
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
    if spawned.is_err() {
        STARTED.store(false, Ordering::Release);
    }
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
    fn minimize_off_keeps_unsupported_after_success() {
        let mut s = FlashlightToggleState::default();
        apply_read(&mut s, Err(FlashlightFailure::Unsupported));
        apply_minimize_off(&mut s, true, Err(FlashlightFailure::Unsupported));
        assert!(s.unsupported);
        assert_eq!(s.on, None);
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
    fn minimize_turns_off_unless_idle_and_confirmed_off() {
        let st = |phase, on| FlashlightToggleState {
            phase,
            on,
            ..FlashlightToggleState::default()
        };
        let idle_on = st(FlashlightPhase::Idle, Some(true));
        assert!(should_turn_off_on_minimize(&idle_on, true));
        assert!(!should_turn_off_on_minimize(&idle_on, false));
        for phase in [
            FlashlightPhase::Requested,
            FlashlightPhase::Applying,
            FlashlightPhase::Confirming,
        ] {
            assert!(should_turn_off_on_minimize(&st(phase, Some(false)), true));
            assert!(!should_turn_off_on_minimize(&st(phase, Some(false)), false));
        }
        assert!(!should_turn_off_on_minimize(
            &st(FlashlightPhase::Idle, Some(false)),
            true
        ));
        // 読み取り不能（None）でも最小化時は消灯を試みる
        assert!(should_turn_off_on_minimize(
            &st(FlashlightPhase::Idle, None),
            true
        ));
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
