//! Phase 11（親 #1129）型サンプル「画面スリープの抑止」（Issue #1278）の effect ドライバ。
//! `fandhe_core::screens::wake_lock_toggle`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::screen_wake_lock::ScreenWakeLock` で抑止を取る / 外し、直後に
//! [`ScreenWakeLock::is_held`] で実際に効いているかを読み戻して `CompletionSender` 経由で
//! 返す（`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約（冪等・エラー種別・守る範囲）の SSOT は `fandhe_device::screen_wake_lock`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。Web（wasm32）は
//! 本経路を通らず `fandhe-web-dom` が Screen Wake Lock API へ直接配線する（#1281）。
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
//! macOS: `macos_screen_wake_lock`、Windows: `windows_screen_wake_lock`、iOS（#1279）:
//! `ios_screen_wake_lock`。Android（#1280）: `android_screen_wake_lock`。
//! dispatch・共通コア画面は本 PR で最終形にしてある。それ以外（Linux）は
//! `UnsupportedScreenWakeLock` 固定。

use std::sync::Arc;

use fandhe_core::screens::wake_lock_toggle::{
    finish, mark_applying, take_request, WakeLockFailure, WakeLockToggleState,
};
use fandhe_device::screen_wake_lock::ScreenWakeLock;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `ScreenWakeLock` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn ScreenWakeLock> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn ScreenWakeLock>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_screen_wake_lock::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_screen_wake_lock::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_screen_wake_lock::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_screen_wake_lock::implementation()
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "ios",
                target_os = "android"
            )))]
            {
                Arc::new(fandhe_device::screen_wake_lock::UnsupportedScreenWakeLock)
            }
        })
        .clone()
}

/// `DeviceError` を画面の失敗種別へ写す。`Unsupported` 以外は `Display` の文言のみを転送する
/// （OS のエラーコードを画面・ログへ出さない、security.md A09）。
fn to_failure(e: DeviceError) -> WakeLockFailure {
    match e {
        DeviceError::Unsupported(_) => WakeLockFailure::Unsupported,
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
    drive(state, sender, implementation());
}

/// `SetThreadExecutionState` の戻り値（直前の状態。失敗時は 0）を契約のエラー種別へ写す。
/// ホストでも検証できるよう OS 非依存の純関数にしてある。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn exec_state_result(prev: u32) -> Result<(), DeviceError> {
    if prev == 0 {
        Err(DeviceError::Internal("screen-wake-lock"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::wake_lock_toggle::{
        turn_off, turn_on, WakeLockPhase, FAILED_PREFIX, HELD_STATUS, RELEASED_STATUS,
        UNSUPPORTED_STATUS,
    };
    use fandhe_device::screen_wake_lock::UnsupportedScreenWakeLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn sender_with_channel() -> (
        CompletionSender<WakeLockToggleState>,
        std::sync::mpsc::Receiver<crate::app::Completion<WakeLockToggleState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 成功なら `held` を要求どおりに更新し、失敗なら変えない Fake。
    struct Fake {
        held: AtomicBool,
        fail: Option<fn() -> DeviceError>,
        /// `Ok` を返すのに `held` を変えない（嘘の成功の再現）。
        ignore: bool,
    }

    impl ScreenWakeLock for Fake {
        fn set_keep_awake(&self, keep: bool) -> Result<(), DeviceError> {
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

    fn fake(fail: Option<fn() -> DeviceError>, ignore: bool) -> Arc<Fake> {
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
        let lock = fake(Some(|| DeviceError::Internal("screen-wake-lock")), false);
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
            implementation().set_keep_awake(true),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn exec_state_result_maps_zero_to_internal() {
        assert!(matches!(
            exec_state_result(0),
            Err(DeviceError::Internal(_))
        ));
        assert!(exec_state_result(0x8000_0000).is_ok());
        assert!(exec_state_result(1).is_ok());
    }
}
