//! Phase 11（親 #127）型サンプル「ウィンドウ管理」（Issue #636）の effect ドライバ。
//! `fandhe_core::screens::window_management_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::window_management::WindowManager` で位置・大きさ・最前面の変更を依頼して、
//! 毎フレーム読み戻した実際の状態が目標に一致したら成功として `CompletionSender` 経由で返す
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約（依頼と完了の分離・全画面中の拒否・エラー種別・守る範囲）の SSOT は
//! `fandhe_device::window_management`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象。Web は本経路を通らず
//! `fandhe-web-dom` が配線する（#638）。
//!
//! # 実装
//!
//! 書き込みは eframe の `ViewportCommand`（`OuterPosition`・`InnerSize`・`WindowLevel`）で行う
//! （[`ViewportWindowManager`]）。egui の viewport 情報に無い「最前面の読み戻し」と「今いる画面の
//! 作業領域」だけを OS 別の [`NativeReads`] が読む（macOS は `macos_window_management.rs`、
//! Windows は `windows_window_management.rs` が #637 で埋める）。
//! [`implementation`] は全 OS の最終形で、後続の子は触らない。iOS・Android は対応表で `n-a` のため
//! `UnsupportedWindowManager` 固定。
//!
//! # スレッド・再描画
//!
//! 全 OS とも UI スレッド（egui の update ループ）で同期に呼び、ワーカーは作らない。
//! 確認待ち中は [`request_slot::POLL_INTERVAL`] で次フレームを予約する（契約表 4）。平常時は
//! 外部変化（ドラッグ・外部からの最前面変更）を拾うため 1 秒後の読み直しだけを予約し、
//! 再描画ループは作らない。座標・画面の大きさはログへ出さない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::window_management_show::{
    apply_observed, apply_support, finish, mark_applying, take_request, WindowAction,
    WindowFailure, WindowManagementState, WindowObserved, WindowPhase, WindowSupportView,
    CONFIRM_TIMEOUT_SECS, DEFAULT_INNER_SIZE, NOT_APPLIED_REASON, UNREADABLE_REASON,
};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use fandhe_device::window_management::UnsupportedWindowManager;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use fandhe_device::window_management::WindowFrame;
use fandhe_device::window_management::{
    reached, WindowManager, WindowOp, WindowSnapshot, WindowSupport,
};
use fandhe_device::DeviceError;

use super::request_slot;
use crate::app::CompletionSender;

/// 平常時に外部変化を読み直す間隔。
const IDLE_REFRESH: Duration = Duration::from_secs(1);

/// egui の viewport 情報に無い 2 つの読み取り（OS 別実装）。
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) trait NativeReads: Send + Sync {
    /// ウィンドウが最前面か。読めなければ `None`。
    fn always_on_top(&self) -> Option<bool>;
    /// 今いる画面の作業領域（左上原点・egui point）。読めなければ `None`。
    fn work_area(&self, ctx: &egui::Context) -> Option<WindowFrame>;
}

/// eframe の viewport コマンドで変更し、viewport 情報と [`NativeReads`] で読み戻す実装。
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct ViewportWindowManager<R: NativeReads> {
    pub(crate) ctx: egui::Context,
    pub(crate) reads: R,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn frame_from_rect(r: egui::Rect) -> WindowFrame {
    WindowFrame {
        x: f64::from(r.min.x),
        y: f64::from(r.min.y),
        width: f64::from(r.width()),
        height: f64::from(r.height()),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl<R: NativeReads> WindowManager for ViewportWindowManager<R> {
    fn support(&self) -> WindowSupport {
        WindowSupport {
            move_resize: true,
            always_on_top: true,
        }
    }

    fn snapshot(&self) -> WindowSnapshot {
        let (outer, inner, fullscreen, minimized) = self.ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.fullscreen, v.minimized)
        });
        // 最小化中は位置・大きさが実際と食い違うため読めない扱いにする。
        let readable = minimized != Some(true);
        WindowSnapshot {
            outer: outer.filter(|_| readable).map(frame_from_rect),
            inner: inner.filter(|_| readable).map(frame_from_rect),
            always_on_top: self.reads.always_on_top(),
            fullscreen,
        }
    }

    fn target(&self) -> Result<WindowFrame, DeviceError> {
        self.reads
            .work_area(&self.ctx)
            .ok_or(DeviceError::Internal("window-management work area"))
    }

    fn request(&self, op: WindowOp) -> Result<(), DeviceError> {
        use egui::viewport::WindowLevel;
        // OS へ渡す前に有限性と正の大きさを検査する（A03）。
        let cmd = match op {
            WindowOp::MoveTo { x, y } if x.is_finite() && y.is_finite() => {
                egui::ViewportCommand::OuterPosition(egui::pos2(x as f32, y as f32))
            }
            WindowOp::ResizeInner { width, height }
                if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 =>
            {
                egui::ViewportCommand::InnerSize(egui::vec2(width as f32, height as f32))
            }
            WindowOp::SetAlwaysOnTop(on) => egui::ViewportCommand::WindowLevel(if on {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            }),
            _ => return Err(DeviceError::Internal("window-management input")),
        };
        self.ctx.send_viewport_cmd(cmd);
        Ok(())
    }
}

/// 現在のプラットフォームに応じた `WindowManager` 実装を返す。全 OS の最終形（後続の子は触らない）。
/// iOS・Android は対応表で `n-a`、Web は本経路を通らず `fandhe-web-dom`（#638）。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(unused_variables)
)]
fn implementation<S>(sender: &CompletionSender<S>) -> Arc<dyn WindowManager> {
    #[cfg(target_os = "macos")]
    {
        super::macos_window_management::implementation(sender.egui_context())
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_window_management::implementation(sender.egui_context())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Arc::new(UnsupportedWindowManager)
    }
}

/// 確認が確定した結果（`finish` へ渡す値）。`RequestSlot` に置いて `poll_and_finish` で画面へ返す。
#[derive(Clone)]
struct Outcome {
    generation: u64,
    result: Result<(), WindowFailure>,
    observed: WindowObserved,
}

/// 要求の追跡（採番・結果・完了処理）は共通部品 `RequestSlot` / `poll_and_finish` に任せる。
/// 本機能は OS が結果を push しないため、毎フレームの読み戻しが目標に一致するか期限が来た時点で
/// 自分で `deliver` する。
static SLOT: request_slot::RequestSlot<Outcome> = request_slot::RequestSlot::new();

/// 読み戻しと期限判定に要る確認条件（`SLOT` の要求 id に紐づく）。
struct Watch {
    request_id: u64,
    generation: u64,
    op: WindowOp,
    started: Instant,
}

// ponytail: 1 プロセス 1 ウィンドウ前提の static。複数 Context を扱うなら `ctx.data` へ移す。
// 読み書きは UI スレッドからだけ。
static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

/// `DeviceError` を画面の失敗種別へ写す。`Unsupported` 以外は `Display` の文言のみを転送する
/// （OS のエラーコードを画面・ログへ出さない、security.md A09）。
fn to_failure(e: DeviceError) -> WindowFailure {
    match e {
        DeviceError::Unsupported(_) => WindowFailure::Unsupported,
        other => WindowFailure::Failed(other.to_string()),
    }
}

fn observed_from(s: &WindowSnapshot) -> WindowObserved {
    WindowObserved {
        position: s.outer.map(|f| (f.x, f.y)),
        inner_size: s.inner.map(|f| (f.width, f.height)),
        always_on_top: s.always_on_top,
        fullscreen: s.fullscreen,
    }
}

fn support_view(s: WindowSupport) -> WindowSupportView {
    WindowSupportView {
        move_resize: Some(s.move_resize),
        always_on_top: Some(s.always_on_top),
        in_browser_tab: false,
    }
}

/// 画面の操作を依頼する `WindowOp` へ変換する（失敗は依頼を出さない理由）。
fn plan(
    action: WindowAction,
    support: WindowSupport,
    snap: &WindowSnapshot,
    mgr: &dyn WindowManager,
) -> Result<WindowOp, WindowFailure> {
    let supported = match action {
        WindowAction::SetAlwaysOnTop(_) => support.always_on_top,
        _ => support.move_resize,
    };
    if !supported {
        return Err(WindowFailure::Unsupported);
    }
    // 全画面中・全画面の状態が読めないときは OS へ依頼を出さない。
    match snap.fullscreen {
        Some(true) => return Err(WindowFailure::FullscreenRefused),
        Some(false) => {}
        None => return Err(WindowFailure::Failed(UNREADABLE_REASON.to_string())),
    }
    match action {
        WindowAction::MoveTopLeft => {
            let area = mgr.target().map_err(to_failure)?;
            Ok(WindowOp::MoveTo {
                x: area.x,
                y: area.y,
            })
        }
        WindowAction::ResetSize => Ok(WindowOp::ResizeInner {
            width: f64::from(DEFAULT_INNER_SIZE[0]),
            height: f64::from(DEFAULT_INNER_SIZE[1]),
        }),
        WindowAction::SetAlwaysOnTop(on) => Ok(WindowOp::SetAlwaysOnTop(on)),
    }
}

/// 1 フレーム分の処理（テストから実装と時刻を差し込めるよう切り出す）。
fn drive(
    state: &WindowManagementState,
    sender: &CompletionSender<WindowManagementState>,
    mgr: &dyn WindowManager,
    now: Instant,
) {
    let mut watch = WATCH.lock().unwrap_or_else(|p| p.into_inner());

    if let Some(req) = take_request(state) {
        let generation = req.generation;
        let snap = mgr.snapshot();
        let observed = observed_from(&snap);
        let outcome = plan(req.action, mgr.support(), &snap, mgr)
            .and_then(|op| mgr.request(op).map(|()| op).map_err(to_failure));
        match outcome {
            Err(failure) => {
                *watch = None;
                SLOT.reset();
                let _ = sender.send(Box::new(move |s: &mut WindowManagementState| {
                    finish(s, generation, Err(failure), observed);
                }));
            }
            Ok(op) => {
                *watch = Some(Watch {
                    request_id: SLOT.begin(),
                    generation,
                    op,
                    started: now,
                });
                let _ = sender.send(Box::new(mark_applying));
            }
        }
        return;
    }

    if let Some(w) = watch.as_ref() {
        if state.phase != WindowPhase::Applying || state.generation != w.generation {
            // 古い世代・完了済みの確認待ちは捨てる。
            *watch = None;
            SLOT.reset();
        } else {
            let snap = mgr.snapshot();
            // 期限判定を一致判定より先に行う。完了時刻が取れないため、期限後に
            // 確認できた変化は成功にせず失敗で確定し、observed で実表示を反映する。
            let result = if now.saturating_duration_since(w.started)
                >= Duration::from_secs(CONFIRM_TIMEOUT_SECS)
            {
                Some(Err(WindowFailure::Failed(NOT_APPLIED_REASON.to_string())))
            } else if reached(&w.op, &snap) {
                Some(Ok(()))
            } else {
                None
            };
            if let Some(result) = result {
                SLOT.deliver(
                    w.request_id,
                    Outcome {
                        generation: w.generation,
                        result,
                        observed: observed_from(&snap),
                    },
                );
                *watch = None;
            }
        }
    }
    drop(watch);

    // 確認待ちの再描画予約と、確定結果の画面反映（共通部品）。
    request_slot::poll_and_finish(
        &SLOT,
        sender,
        |_| {},
        |s: &mut WindowManagementState, o: Outcome| {
            finish(s, o.generation, o.result, o.observed);
            true
        },
    );
    if SLOT.snapshot().is_some() {
        return;
    }

    // 外部変化の追従。変化が無いときは送らない。
    if state.phase == WindowPhase::Idle {
        let observed = observed_from(&mgr.snapshot());
        if observed != state.observed {
            let _ = sender.send(Box::new(move |s: &mut WindowManagementState| {
                apply_observed(s, observed);
            }));
        }
        let support = support_view(mgr.support());
        if support != state.support {
            let _ = sender.send(Box::new(move |s: &mut WindowManagementState| {
                apply_support(s, support);
            }));
        }
        // ドラッグ・外部からの最前面変更を拾うため、1 秒後に読み直す（再描画ループにはしない）。
        sender.request_repaint_after(IDLE_REFRESH);
    }
}

/// `registry.rs` の `effect` 区画から `window_management_show` 画面用に配線される effect ドライバ。
pub fn window_management_show_effect(
    state: &WindowManagementState,
    sender: &CompletionSender<WindowManagementState>,
) {
    drive(
        state,
        sender,
        implementation(sender).as_ref(),
        Instant::now(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::window_management_show::{
        always_on_top_off, always_on_top_on, frame_text, move_top_left, reset_size, FAILED_PREFIX,
        FRAME_UNKNOWN_STATUS, FULLSCREEN_REFUSED_REASON, UNSUPPORTED_STATUS,
    };
    use fandhe_device::window_management::UnsupportedWindowManager;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Receiver;

    /// `SLOT` / `WATCH` が static のため、テストは直列化してから初期化する。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        *WATCH.lock().unwrap_or_else(|p| p.into_inner()) = None;
        SLOT.reset();
        g
    }

    type Rx = Receiver<crate::app::Completion<WindowManagementState>>;

    fn sender_with_channel() -> (CompletionSender<WindowManagementState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    const AREA: WindowFrame = WindowFrame {
        x: 0.0,
        y: 25.0,
        width: 1440.0,
        height: 875.0,
    };

    /// 実 OS の代わりに内部状態を持つ偽実装。
    struct Fake {
        snap: Mutex<WindowSnapshot>,
        /// `Ok` を返すのに状態を変えない（嘘の成功の再現）。
        ignore: bool,
        fail: bool,
        target_ok: bool,
        requests: AtomicUsize,
    }

    impl WindowManager for Fake {
        fn support(&self) -> WindowSupport {
            WindowSupport {
                move_resize: true,
                always_on_top: true,
            }
        }
        fn snapshot(&self) -> WindowSnapshot {
            *self.snap.lock().unwrap()
        }
        fn target(&self) -> Result<WindowFrame, DeviceError> {
            if self.target_ok {
                Ok(AREA)
            } else {
                Err(DeviceError::Internal("window-management work area"))
            }
        }
        fn request(&self, op: WindowOp) -> Result<(), DeviceError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(DeviceError::Internal("window-management"));
            }
            if self.ignore {
                return Ok(());
            }
            let mut s = self.snap.lock().unwrap();
            match op {
                WindowOp::MoveTo { x, y } => {
                    let o = s.outer.as_mut().unwrap();
                    o.x = x;
                    o.y = y;
                }
                WindowOp::ResizeInner { width, height } => {
                    let i = s.inner.as_mut().unwrap();
                    i.width = width;
                    i.height = height;
                }
                WindowOp::SetAlwaysOnTop(on) => s.always_on_top = Some(on),
            }
            Ok(())
        }
    }

    fn base_snapshot() -> WindowSnapshot {
        WindowSnapshot {
            outer: Some(WindowFrame {
                x: 300.0,
                y: 200.0,
                width: 480.0,
                height: 390.0,
            }),
            inner: Some(WindowFrame {
                x: 300.0,
                y: 230.0,
                width: 600.0,
                height: 400.0,
            }),
            always_on_top: Some(false),
            fullscreen: Some(false),
        }
    }

    fn fake(ignore: bool, fail: bool) -> Fake {
        Fake {
            snap: Mutex::new(base_snapshot()),
            ignore,
            fail,
            target_ok: true,
            requests: AtomicUsize::new(0),
        }
    }

    fn apply_all(state: &mut WindowManagementState, rx: &Rx) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(state);
            n += 1;
        }
        n
    }

    /// 平常時の 1 フレームを回して support と観測値を画面へ入れる。
    fn settle(state: &mut WindowManagementState, mgr: &dyn WindowManager) {
        let (sender, rx) = sender_with_channel();
        drive(state, &sender, mgr, Instant::now());
        apply_all(state, &rx);
    }

    /// 要求を出して依頼し、確認待ち 1 フレームを回す。
    fn run(
        state: &mut WindowManagementState,
        mgr: &dyn WindowManager,
        on: impl Fn(&mut WindowManagementState),
        wait: Duration,
    ) {
        let (sender, rx) = sender_with_channel();
        let t0 = Instant::now();
        on(state);
        drive(state, &sender, mgr, t0);
        apply_all(state, &rx);
        drive(state, &sender, mgr, t0 + wait);
        apply_all(state, &rx);
    }

    #[test]
    fn no_request_and_no_change_sends_nothing() {
        let _g = guard();
        let mgr = fake(false, false);
        let mut state = WindowManagementState::default();
        settle(&mut state, &mgr);
        let (sender, rx) = sender_with_channel();
        drive(&state, &sender, &mgr, Instant::now());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn move_reset_and_top_round_trip_succeed_with_read_back_values() {
        let _g = guard();
        let mgr = fake(false, false);
        let mut state = WindowManagementState::default();
        settle(&mut state, &mgr);
        run(&mut state, &mgr, move_top_left, Duration::ZERO);
        assert_eq!(state.error, "");
        assert_eq!(state.observed.position, Some((0.0, 25.0)));
        run(&mut state, &mgr, reset_size, Duration::ZERO);
        assert_eq!(state.error, "");
        assert_eq!(state.observed.inner_size, Some((480.0, 360.0)));
        run(&mut state, &mgr, always_on_top_on, Duration::ZERO);
        assert_eq!(state.observed.always_on_top, Some(true));
        run(&mut state, &mgr, always_on_top_off, Duration::ZERO);
        assert_eq!(state.observed.always_on_top, Some(false));
        assert_eq!(state.error, "");
        assert_eq!(state.phase, WindowPhase::Idle);
    }

    #[test]
    fn ok_without_effect_waits_then_fails_at_timeout() {
        let _g = guard();
        let mgr = fake(true, false);
        let mut state = WindowManagementState::default();
        run(&mut state, &mgr, move_top_left, Duration::from_secs(1));
        assert_eq!(state.phase, WindowPhase::Applying, "期限前は確定しない");
        let (sender, rx) = sender_with_channel();
        drive(
            &state,
            &sender,
            &mgr,
            Instant::now() + Duration::from_secs(10),
        );
        apply_all(&mut state, &rx);
        assert_eq!(state.phase, WindowPhase::Idle);
        assert_eq!(state.error, format!("{FAILED_PREFIX}{NOT_APPLIED_REASON}"));
    }

    #[test]
    fn match_after_deadline_is_failure_with_observed_state() {
        let _g = guard();
        let mgr = fake(true, false);
        let mut state = WindowManagementState::default();
        let (sender, rx) = sender_with_channel();
        let t0 = Instant::now();
        move_top_left(&mut state);
        drive(&state, &sender, &mgr, t0);
        apply_all(&mut state, &rx);
        assert_eq!(state.phase, WindowPhase::Applying);
        mgr.snap.lock().unwrap().outer.as_mut().unwrap().x = 0.0;
        mgr.snap.lock().unwrap().outer.as_mut().unwrap().y = 25.0;
        drive(
            &state,
            &sender,
            &mgr,
            t0 + Duration::from_secs(CONFIRM_TIMEOUT_SECS + 1),
        );
        apply_all(&mut state, &rx);
        assert_eq!(state.phase, WindowPhase::Idle);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_eq!(
            state.observed.position,
            Some((0.0, 25.0)),
            "実表示は反映する"
        );
    }

    #[test]
    fn fullscreen_refuses_all_ops_without_calling_os() {
        let _g = guard();
        for on in [
            move_top_left as fn(&mut WindowManagementState),
            reset_size,
            always_on_top_on,
        ] {
            let mgr = fake(false, false);
            mgr.snap.lock().unwrap().fullscreen = Some(true);
            let mut state = WindowManagementState::default();
            run(&mut state, &mgr, on, Duration::ZERO);
            assert_eq!(mgr.requests.load(Ordering::SeqCst), 0);
            assert_eq!(
                state.error,
                format!("{FAILED_PREFIX}{FULLSCREEN_REFUSED_REASON}")
            );
            assert_eq!(state.phase, WindowPhase::Idle);
        }
    }

    #[test]
    fn unknown_fullscreen_does_not_request_and_fails() {
        let _g = guard();
        let mgr = fake(false, false);
        mgr.snap.lock().unwrap().fullscreen = None;
        let mut state = WindowManagementState::default();
        run(&mut state, &mgr, move_top_left, Duration::ZERO);
        assert_eq!(mgr.requests.load(Ordering::SeqCst), 0);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
    }

    #[test]
    fn minimized_unreadable_frame_never_succeeds() {
        let _g = guard();
        let mgr = fake(false, false);
        // 最小化中を模す: 依頼後の読み戻しで位置が読めない。
        let mut state = WindowManagementState::default();
        let (sender, rx) = sender_with_channel();
        let t0 = Instant::now();
        move_top_left(&mut state);
        drive(&state, &sender, &mgr, t0);
        apply_all(&mut state, &rx);
        mgr.snap.lock().unwrap().outer = None;
        drive(&state, &sender, &mgr, t0 + Duration::from_secs(1));
        apply_all(&mut state, &rx);
        assert_eq!(state.phase, WindowPhase::Applying);
        drive(&state, &sender, &mgr, t0 + Duration::from_secs(10));
        apply_all(&mut state, &rx);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_eq!(frame_text(&state), FRAME_UNKNOWN_STATUS);
    }

    #[test]
    fn unsupported_manager_never_succeeds() {
        let _g = guard();
        for on in [
            move_top_left as fn(&mut WindowManagementState),
            reset_size,
            always_on_top_on,
        ] {
            let (sender, rx) = sender_with_channel();
            let mut state = WindowManagementState::default();
            on(&mut state);
            drive(&state, &sender, &UnsupportedWindowManager, Instant::now());
            apply_all(&mut state, &rx);
            assert_eq!(state.error, UNSUPPORTED_STATUS);
            assert_eq!(state.phase, WindowPhase::Idle);
        }
    }

    #[test]
    fn internal_error_is_failure_with_prefix() {
        let _g = guard();
        let mut state = WindowManagementState::default();
        run(
            &mut state,
            &fake(false, true),
            move_top_left,
            Duration::ZERO,
        );
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_eq!(state.phase, WindowPhase::Idle);
    }

    #[test]
    fn unreadable_work_area_does_not_request_move() {
        let _g = guard();
        let mut mgr = fake(false, false);
        mgr.target_ok = false;
        let mut state = WindowManagementState::default();
        run(&mut state, &mgr, move_top_left, Duration::ZERO);
        assert_eq!(mgr.requests.load(Ordering::SeqCst), 0);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
    }

    #[test]
    fn stale_pending_is_dropped_without_finish() {
        let _g = guard();
        let (sender, rx) = sender_with_channel();
        *WATCH.lock().unwrap() = Some(Watch {
            request_id: SLOT.begin(),
            generation: 7,
            op: WindowOp::SetAlwaysOnTop(true),
            started: Instant::now(),
        });
        let mut state = WindowManagementState::default();
        move_top_left(&mut state);
        mark_applying(&mut state);
        drive(&state, &sender, &fake(false, false), Instant::now());
        assert!(WATCH.lock().unwrap().is_none());
        assert!(SLOT.snapshot().is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn external_change_is_applied_once() {
        let _g = guard();
        let mgr = fake(false, false);
        let mut state = WindowManagementState::default();
        settle(&mut state, &mgr);
        mgr.snap.lock().unwrap().always_on_top = Some(true);
        let (sender, rx) = sender_with_channel();
        drive(&state, &sender, &mgr, Instant::now());
        assert_eq!(apply_all(&mut state, &rx), 1);
        drive(&state, &sender, &mgr, Instant::now());
        assert_eq!(apply_all(&mut state, &rx), 0);
        assert_eq!(state.observed.always_on_top, Some(true));
    }

    struct NoReads;
    impl NativeReads for NoReads {
        fn always_on_top(&self) -> Option<bool> {
            None
        }
        fn work_area(&self, _ctx: &egui::Context) -> Option<WindowFrame> {
            None
        }
    }

    #[test]
    fn viewport_manager_without_viewport_info_does_not_succeed() {
        let _g = guard();
        let mgr = ViewportWindowManager {
            ctx: egui::Context::default(),
            reads: NoReads,
        };
        let mut state = WindowManagementState::default();
        run(&mut state, &mgr, reset_size, Duration::from_secs(10));
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_eq!(state.phase, WindowPhase::Idle);
    }

    #[test]
    fn viewport_manager_rejects_invalid_input() {
        let mgr = ViewportWindowManager {
            ctx: egui::Context::default(),
            reads: NoReads,
        };
        for op in [
            WindowOp::MoveTo {
                x: f64::NAN,
                y: 0.0,
            },
            WindowOp::ResizeInner {
                width: f64::INFINITY,
                height: 360.0,
            },
            WindowOp::ResizeInner {
                width: 0.0,
                height: 360.0,
            },
        ] {
            assert!(matches!(mgr.request(op), Err(DeviceError::Internal(_))));
        }
        assert!(matches!(
            mgr.target(),
            Err(DeviceError::Internal("window-management work area"))
        ));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        let (sender, _rx) = sender_with_channel();
        assert!(matches!(
            implementation(&sender).request(WindowOp::SetAlwaysOnTop(true)),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
