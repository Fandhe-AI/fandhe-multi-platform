//! Phase 11（親 #1129）型サンプル「画面スリープの抑止」（Issue #1278）の effect ドライバ。
//! `fandhe_core::screens::wake_lock_toggle`（端末依存ゼロ）の要求を観測し、
//! `crate::contract::ScreenWakeLock` で抑止を取る / 外し、直後に
//! [`ScreenWakeLock::is_held`] で実際に効いているかを読み戻して `CompletionSender` 経由で
//! 返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。
//! 契約（冪等・エラー種別・守る範囲）の SSOT は `crate::contract`。
//!
//! Web（wasm32）は本経路を通らず `fandhe-web-dom` が Screen Wake Lock API へ直接配線する（#1281）。
//!
//! # スレッド
//!
//! 全 OS とも呼ばれたスレッド（egui の update ループ = UI スレッド）で同期に呼び、
//! ワーカースレッドは作らない。Windows の `SetThreadExecutionState` は呼んだスレッドに
//! 紐づくため、取得と解除を同じスレッドで対にする必要がある（haptics の Windows ワーカー経路は
//! 踏襲しない）。
//!
//! # 対応 OS と後続
//!
//! macOS: `macos`、Windows: `windows`、iOS（#1279）:
//! `ios`。Android（#1280）: `android`。
//! dispatch・共通コア画面は本 PR で最終形にしてある。それ以外（Linux）は
//! `UnsupportedScreenWakeLock` 固定。

use std::sync::Arc;

use crate::contract::ScreenWakeLock;
use crate::error::ScreenWakeLockError;
use fandhe_core::screens::wake_lock_toggle::{
    WakeLockFailure, WakeLockToggleState, finish, mark_applying, take_request,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた `ScreenWakeLock` 実装を返す（プロセスで 1 個）。
/// 画面の状態型に依存しないため、フレームワークの外のアプリも使える。
pub fn platform_screen_wake_lock() -> Arc<dyn ScreenWakeLock> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn ScreenWakeLock>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                crate::macos::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                crate::windows::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                crate::ios::implementation()
            }
            #[cfg(target_os = "android")]
            {
                crate::android::implementation()
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "ios",
                target_os = "android"
            )))]
            {
                Arc::new(crate::contract::UnsupportedScreenWakeLock)
            }
        })
        .clone()
}

/// `ScreenWakeLockError` を画面の失敗種別へ写す。`Unsupported` 以外は `Display` の文言のみを転送する
/// （OS のエラーコードを画面・ログへ出さない、security.md A09）。
fn to_failure(e: ScreenWakeLockError) -> WakeLockFailure {
    match e {
        ScreenWakeLockError::Unsupported => WakeLockFailure::Unsupported,
        other => WakeLockFailure::Failed(other.to_string()),
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &WakeLockToggleState,
    sender: &CompletionSender<WakeLockToggleState>,
    lock: Arc<dyn ScreenWakeLock>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    // 受信側のドロップ（アプリ終了中）による送信失敗は無視してよい。
    let _ = sender.send(Box::new(mark_applying));

    let result = lock.set_keep_awake(request.keep).map_err(to_failure);
    // 成否に関わらず、結果を届ける前に実際の状態を読み戻す（嘘の成功の禁止）。
    let held = lock.is_held();
    let _ = sender.send(Box::new(move |s: &mut WakeLockToggleState| {
        finish(s, generation, result, held);
    }));
}

/// `registry.rs` の `effect` 区画から `wake_lock_toggle` 画面用に配線される effect ドライバ。
pub fn wake_lock_toggle_effect(
    state: &WakeLockToggleState,
    sender: &CompletionSender<WakeLockToggleState>,
) {
    drive(state, sender, platform_screen_wake_lock());
}

/// `SetThreadExecutionState` の戻り値（直前の状態。失敗時は 0）を契約のエラー種別へ写す。
/// ホストでも検証できるよう OS 非依存の純関数にしてある。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn exec_state_result(prev: u32) -> Result<(), ScreenWakeLockError> {
    if prev == 0 {
        Err(ScreenWakeLockError::Internal)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedScreenWakeLock;
    use fandhe_core::screens::wake_lock_toggle::{
        FAILED_PREFIX, HELD_STATUS, RELEASED_STATUS, UNSUPPORTED_STATUS, WakeLockPhase, turn_off,
        turn_on,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    fn sender_with_channel() -> (
        CompletionSender<WakeLockToggleState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<WakeLockToggleState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    /// 成功なら `held` を要求どおりに更新し、失敗なら変えない Fake。
    struct Fake {
        held: AtomicBool,
        fail: Option<fn() -> ScreenWakeLockError>,
        /// `Ok` を返すのに `held` を変えない（嘘の成功の再現）。
        ignore: bool,
    }

    impl ScreenWakeLock for Fake {
        fn set_keep_awake(&self, keep: bool) -> Result<(), ScreenWakeLockError> {
            if let Some(f) = self.fail {
                return Err(f());
            }
            if !self.ignore {
                self.held.store(keep, Ordering::SeqCst);
            }
            Ok(())
        }
        fn is_held(&self) -> bool {
            self.held.load(Ordering::SeqCst)
        }
    }

    fn fake(fail: Option<fn() -> ScreenWakeLockError>, ignore: bool) -> Arc<Fake> {
        Arc::new(Fake {
            held: AtomicBool::new(false),
            fail,
            ignore,
        })
    }

    /// 要求を立てて `drive` し、届いた完了（mark_applying + finish）をすべて適用する。
    fn run(
        state: &mut WakeLockToggleState,
        lock: Arc<dyn ScreenWakeLock>,
        on: impl Fn(&mut WakeLockToggleState),
    ) {
        let (sender, rx) = sender_with_channel();
        on(state);
        drive(state, &sender, lock);
        for _ in 0..2 {
            let c = rx.try_recv().expect("mark_applying と完了が届く");
            c(state);
        }
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        drive(&WakeLockToggleState::default(), &sender, fake(None, false));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn acquire_then_release_round_trip() {
        let lock = fake(None, false);
        let mut state = WakeLockToggleState::default();
        run(&mut state, lock.clone(), turn_on);
        assert_eq!(
            fandhe_core::screens::wake_lock_toggle::status_text(&state),
            HELD_STATUS
        );
        run(&mut state, lock, turn_off);
        assert_eq!(
            fandhe_core::screens::wake_lock_toggle::status_text(&state),
            RELEASED_STATUS
        );
        assert_eq!(state.phase, WakeLockPhase::Idle);
    }

    #[test]
    fn ok_without_effect_is_failure_not_held() {
        let mut state = WakeLockToggleState::default();
        run(&mut state, fake(None, true), turn_on);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert!(!state.held);
    }

    #[test]
    fn unsupported_is_failure_not_success() {
        let mut state = WakeLockToggleState::default();
        run(&mut state, Arc::new(UnsupportedScreenWakeLock), turn_on);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert!(!state.held);
    }

    #[test]
    fn internal_error_is_failure_with_prefix() {
        let mut state = WakeLockToggleState::default();
        let lock = fake(Some(|| ScreenWakeLockError::Internal), false);
        run(&mut state, lock, turn_on);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert!(!state.held);
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            platform_screen_wake_lock().set_keep_awake(true),
            Err(ScreenWakeLockError::Unsupported)
        ));
    }

    #[test]
    fn exec_state_result_maps_zero_to_internal() {
        assert!(matches!(
            exec_state_result(0),
            Err(ScreenWakeLockError::Internal)
        ));
        assert!(exec_state_result(0x8000_0000).is_ok());
        assert!(exec_state_result(1).is_ok());
    }
}
