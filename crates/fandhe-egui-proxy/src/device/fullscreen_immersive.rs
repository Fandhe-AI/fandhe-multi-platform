//! Phase 11（親 #1133）型サンプル「全画面表示」（Issue #1312）の effect ドライバ。
//! `fandhe_core::screens::fullscreen_toggle`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::fullscreen_immersive::Fullscreen` で切り替えを依頼して、毎フレーム
//! 読み戻した実際の状態が目標に一致したら成功として `CompletionSender` 経由で返す
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約（依頼と完了の分離・エラー種別・守る範囲）の SSOT は `fandhe_device::fullscreen_immersive`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象。Web は本経路を通らず
//! `fandhe-web-dom` が Fullscreen API へ直接配線する（#1315）。
//!
//! # 実装
//!
//! macOS / Windows は eframe の `ViewportCommand::Fullscreen`（winit の `set_fullscreen`）で
//! 足りるため、FFI・新規依存・`unsafe` は無い（[`ViewportFullscreen`]）。macOS はネイティブ全画面で
//! メニューバーと Dock が、Windows は枠なし全画面でタスクバーが隠れる。
//! iOS（#1313）/ Android（#1314）は OS 別モジュールの実装を返す。
//! dispatch（[`implementation`]）は全 OS の最終形。
//! 他の OS（Linux 等）は `UnsupportedFullscreen` 固定。
//!
//! # スレッド・再描画
//!
//! 全 OS とも UI スレッド（egui の update ループ）で同期に呼び、ワーカーは作らない。
//! 確認待ち中は [`request_slot::POLL_INTERVAL`] で次フレームを予約する（契約表 4）。変化が無い
//! 平常時は何も送らず、再描画ループを作らない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::fullscreen_toggle::{
    apply_observed, finish, mark_applying, take_request, FullscreenFailure, FullscreenPhase,
    FullscreenToggleState, CONFIRM_TIMEOUT_SECS, NOT_SWITCHED_REASON,
};
use fandhe_device::fullscreen_immersive::Fullscreen;
#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::fullscreen_immersive::UnsupportedFullscreen;
use fandhe_device::DeviceError;

use super::request_slot;
use crate::app::CompletionSender;

/// eframe の viewport コマンドで全画面を切り替える実装（macOS / Windows）。
#[cfg(any(target_os = "macos", target_os = "windows", test))]
struct ViewportFullscreen {
    ctx: egui::Context,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl Fullscreen for ViewportFullscreen {
    fn request(&self, on: bool) -> Result<(), DeviceError> {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
        Ok(())
    }

    fn is_fullscreen(&self) -> Option<bool> {
        self.ctx.input(|i| i.viewport().fullscreen)
    }
}

/// 現在のプラットフォームに応じた `Fullscreen` 実装を返す。全 OS の最終形（後続の子は触らない）。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(unused_variables)
)]
fn implementation<S>(sender: &CompletionSender<S>) -> Arc<dyn Fullscreen> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Arc::new(ViewportFullscreen {
            ctx: sender.egui_context(),
        })
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_fullscreen_immersive::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_fullscreen_immersive::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Arc::new(UnsupportedFullscreen)
    }
}

/// 確認が確定した結果（`finish` へ渡す値）。`RequestSlot` に置いて `poll_and_finish` で画面へ返す。
#[derive(Clone)]
struct Outcome {
    generation: u64,
    result: Result<(), FullscreenFailure>,
    observed: Option<bool>,
}

/// 要求の追跡（採番・結果・完了処理）は共通部品 `RequestSlot` / `poll_and_finish` に任せる。
/// 本機能は OS が結果を push しないため、毎フレームの読み戻しが目標に一致するか期限が来た時点で
/// 自分で `deliver` する。
static SLOT: request_slot::RequestSlot<Outcome> = request_slot::RequestSlot::new();

/// 読み戻しと期限判定に要る、全画面固有の確認条件（`SLOT` の要求 id に紐づく）。
struct Watch {
    request_id: u64,
    generation: u64,
    on: bool,
    started: Instant,
}

// ponytail: 1 プロセス 1 ウィンドウ前提の static。複数 Context を扱うなら `ctx.data` へ移す。
// 読み書きは UI スレッドからだけ。
static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

/// `DeviceError` を画面の失敗種別へ写す。`Unsupported` 以外は `Display` の文言のみを転送する
/// （OS のエラーコードを画面・ログへ出さない、security.md A09）。
fn to_failure(e: DeviceError) -> FullscreenFailure {
    match e {
        DeviceError::Unsupported(_) => FullscreenFailure::Unsupported,
        other => FullscreenFailure::Failed(other.to_string()),
    }
}

/// 1 フレーム分の処理（テストから実装と時刻を差し込めるよう切り出す）。
fn drive(
    state: &FullscreenToggleState,
    sender: &CompletionSender<FullscreenToggleState>,
    ctl: &dyn Fullscreen,
    now: Instant,
) {
    let mut watch = WATCH.lock().unwrap_or_else(|p| p.into_inner());

    if let Some(req) = take_request(state) {
        let generation = req.generation;
        match ctl.request(req.on) {
            Err(e) => {
                *watch = None;
                SLOT.reset();
                let observed = ctl.is_fullscreen();
                let failure = to_failure(e);
                let _ = sender.send(Box::new(move |s: &mut FullscreenToggleState| {
                    finish(s, generation, Err(failure), observed);
                }));
            }
            Ok(()) => {
                *watch = Some(Watch {
                    request_id: SLOT.begin(),
                    generation,
                    on: req.on,
                    started: now,
                });
                let _ = sender.send(Box::new(mark_applying));
            }
        }
        return;
    }

    if let Some(w) = watch.as_ref() {
        if state.phase != FullscreenPhase::Applying || state.generation != w.generation {
            // 古い世代・完了済みの確認待ちは捨てる。
            *watch = None;
            SLOT.reset();
        } else {
            let observed = ctl.is_fullscreen();
            // 期限判定を一致判定より先に行う。完了時刻が取れないため、期限後に
            // 確認できた切り替えは成功にせず失敗で確定し、observed で実表示を反映する。
            let result = if now.saturating_duration_since(w.started)
                >= Duration::from_secs(CONFIRM_TIMEOUT_SECS)
            {
                Some(Err(FullscreenFailure::Failed(
                    NOT_SWITCHED_REASON.to_string(),
                )))
            } else if observed == Some(w.on) {
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
                        observed,
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
        |s: &mut FullscreenToggleState, o: Outcome| {
            finish(s, o.generation, o.result, o.observed);
            true
        },
    );
    if SLOT.snapshot().is_some() {
        return;
    }

    // 外部変化（緑ボタン等）の追従。変化が無いときは送らない（再描画ループを作らない）。
    if state.phase == FullscreenPhase::Idle {
        let observed = ctl.is_fullscreen();
        if observed != state.fullscreen {
            let _ = sender.send(Box::new(move |s: &mut FullscreenToggleState| {
                apply_observed(s, observed);
            }));
        }
    }
}

/// `registry.rs` の `effect` 区画から `fullscreen_toggle` 画面用に配線される effect ドライバ。
pub fn fullscreen_toggle_effect(
    state: &FullscreenToggleState,
    sender: &CompletionSender<FullscreenToggleState>,
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
    use fandhe_core::screens::fullscreen_toggle::{
        enter, exit, status_text, FAILED_PREFIX, FULLSCREEN_STATUS, UNKNOWN_STATUS,
        UNSUPPORTED_STATUS, WINDOWED_STATUS,
    };
    use fandhe_device::fullscreen_immersive::UnsupportedFullscreen;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::mpsc::Receiver;

    /// `SLOT` / `WATCH` が static のため、テストは直列化してから初期化する。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        *WATCH.lock().unwrap_or_else(|p| p.into_inner()) = None;
        SLOT.reset();
        g
    }

    type Rx = Receiver<crate::app::Completion<FullscreenToggleState>>;

    fn sender_with_channel() -> (CompletionSender<FullscreenToggleState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 0 = 通常, 1 = 全画面, 2 = 読めない。
    struct Fake {
        mode: AtomicU8,
        /// `Ok` を返すのに状態を変えない（嘘の成功の再現）。
        ignore: bool,
        fail: bool,
    }

    impl Fullscreen for Fake {
        fn request(&self, on: bool) -> Result<(), DeviceError> {
            if self.fail {
                return Err(DeviceError::Internal("fullscreen-immersive"));
            }
            if !self.ignore {
                self.mode.store(u8::from(on), Ordering::SeqCst);
            }
            Ok(())
        }
        fn is_fullscreen(&self) -> Option<bool> {
            match self.mode.load(Ordering::SeqCst) {
                2 => None,
                m => Some(m == 1),
            }
        }
    }

    fn fake(ignore: bool, fail: bool) -> Fake {
        Fake {
            mode: AtomicU8::new(0),
            ignore,
            fail,
        }
    }

    fn apply_all(state: &mut FullscreenToggleState, rx: &Rx) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(state);
            n += 1;
        }
        n
    }

    /// 要求を出して依頼し、確認待ち 1 フレームを回す。
    fn run(
        state: &mut FullscreenToggleState,
        ctl: &dyn Fullscreen,
        on: impl Fn(&mut FullscreenToggleState),
        wait: Duration,
    ) {
        let (sender, rx) = sender_with_channel();
        let t0 = Instant::now();
        on(state);
        drive(state, &sender, ctl, t0);
        apply_all(state, &rx);
        drive(state, &sender, ctl, t0 + wait);
        apply_all(state, &rx);
    }

    #[test]
    fn no_request_and_no_change_sends_nothing() {
        let _g = guard();
        let (sender, rx) = sender_with_channel();
        let state = FullscreenToggleState {
            fullscreen: Some(false),
            ..Default::default()
        };
        drive(&state, &sender, &fake(false, false), Instant::now());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn round_trip_enter_then_exit() {
        let _g = guard();
        let ctl = fake(false, false);
        let mut state = FullscreenToggleState::default();
        run(&mut state, &ctl, enter, Duration::ZERO);
        assert_eq!(status_text(&state), FULLSCREEN_STATUS);
        run(&mut state, &ctl, exit, Duration::ZERO);
        assert_eq!(status_text(&state), WINDOWED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn ok_without_effect_waits_then_fails_at_timeout() {
        let _g = guard();
        let ctl = fake(true, false);
        let mut state = FullscreenToggleState::default();
        run(&mut state, &ctl, enter, Duration::from_secs(1));
        assert_eq!(state.phase, FullscreenPhase::Applying, "期限前は確定しない");
        let (sender, rx) = sender_with_channel();
        drive(
            &state,
            &sender,
            &ctl,
            Instant::now() + Duration::from_secs(10),
        );
        apply_all(&mut state, &rx);
        assert_eq!(state.phase, FullscreenPhase::Idle);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_ne!(status_text(&state), FULLSCREEN_STATUS);
    }

    #[test]
    fn match_after_deadline_is_failure_with_observed_state() {
        let _g = guard();
        let ctl = fake(true, false);
        let mut state = FullscreenToggleState::default();
        let (sender, rx) = sender_with_channel();
        let t0 = Instant::now();
        enter(&mut state);
        drive(&state, &sender, &ctl, t0);
        apply_all(&mut state, &rx);
        assert_eq!(state.phase, FullscreenPhase::Applying);
        ctl.mode.store(1, Ordering::SeqCst);
        drive(
            &state,
            &sender,
            &ctl,
            t0 + Duration::from_secs(CONFIRM_TIMEOUT_SECS + 1),
        );
        apply_all(&mut state, &rx);
        assert_eq!(state.phase, FullscreenPhase::Idle);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_eq!(state.fullscreen, Some(true), "実表示は反映する");
    }

    #[test]
    fn unreadable_viewport_is_failure_not_success() {
        let _g = guard();
        let ctl = ViewportFullscreen {
            ctx: egui::Context::default(),
        };
        let mut state = FullscreenToggleState::default();
        run(&mut state, &ctl, enter, Duration::from_secs(10));
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_eq!(status_text(&state), UNKNOWN_STATUS);
    }

    #[test]
    fn unsupported_is_failure_not_success() {
        let _g = guard();
        let (sender, rx) = sender_with_channel();
        let mut state = FullscreenToggleState::default();
        enter(&mut state);
        drive(&state, &sender, &UnsupportedFullscreen, Instant::now());
        apply_all(&mut state, &rx);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.fullscreen, None);
    }

    #[test]
    fn internal_error_is_failure_with_prefix() {
        let _g = guard();
        let mut state = FullscreenToggleState::default();
        run(&mut state, &fake(false, true), enter, Duration::ZERO);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_eq!(state.phase, FullscreenPhase::Idle);
    }

    #[test]
    fn stale_pending_is_dropped_without_finish() {
        let _g = guard();
        let (sender, rx) = sender_with_channel();
        *WATCH.lock().unwrap() = Some(Watch {
            request_id: SLOT.begin(),
            generation: 7,
            on: true,
            started: Instant::now(),
        });
        let mut state = FullscreenToggleState::default();
        enter(&mut state);
        mark_applying(&mut state);
        drive(&state, &sender, &fake(false, false), Instant::now());
        assert!(WATCH.lock().unwrap().is_none());
        assert!(SLOT.snapshot().is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn external_change_is_applied_once() {
        let _g = guard();
        let ctl = fake(false, false);
        let (sender, rx) = sender_with_channel();
        let mut state = FullscreenToggleState {
            fullscreen: Some(false),
            ..Default::default()
        };
        ctl.mode.store(1, Ordering::SeqCst);
        drive(&state, &sender, &ctl, Instant::now());
        assert_eq!(apply_all(&mut state, &rx), 1);
        drive(&state, &sender, &ctl, Instant::now());
        assert_eq!(apply_all(&mut state, &rx), 0);
        assert_eq!(status_text(&state), FULLSCREEN_STATUS);
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        let (sender, _rx) = sender_with_channel();
        assert!(matches!(
            implementation(&sender).request(true),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
