//! Phase 11（親 #174）型サンプル「Dock・タスクバーの進捗表示と注意喚起」（Issue #860）の
//! effect ドライバ。`fandhe_core::screens::app_progress_attention_show`（端末依存ゼロ）の要求を
//! 観測し、進捗は `fandhe_device::app_progress_attention::ProgressValue` で検査してから OS へ渡し、
//! 結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `device/app_badge.rs` の同期経路と同型）。契約と守る範囲は
//! `fandhe_device::app_progress_attention` のモジュール doc を参照。
//!
//! - macOS / Windows: [`AppProgressAttention`] を UI スレッドで同期に呼ぶ
//! - iOS / Android / Linux: `UnsupportedAppProgressAttention` で「非対応」を表示する。モバイルに
//!   該当機能は無い（対応表 n-a）ため bridge・`RequestSlot` は作らない
//! - 注意喚起は「待機」に入った後、ウィンドウがフォーカスを失った最初のフレームで 1 回だけ OS を
//!   呼ぶ。フォーカスは winit の `Focused` → egui の `raw.focused`（OS の観測）を読み、タイマーや
//!   アプリ内の記憶は持たない

use fandhe_core::screens::app_progress_attention_show::{
    finish_attention, finish_progress, mark_attention_requesting, mark_progress_setting,
    take_attention_request, take_progress_request, AppProgressAttentionState, AttentionOutcome,
    ProgressOp, ProgressOutcome, OUT_OF_RANGE_MESSAGE,
};
use fandhe_device::app_progress_attention::{AppProgressAttention, ProgressValue};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 進捗の結果を画面用へ写像する。内部構造は出さず固定文言だけにする（security.md A09）。
fn to_progress_outcome(
    op: ProgressOp,
    result: Result<(), DeviceError>,
) -> Result<ProgressOutcome, String> {
    match result {
        Ok(()) if op == ProgressOp::Clear => Ok(ProgressOutcome::Cleared),
        Ok(()) => Ok(ProgressOutcome::Submitted),
        Err(DeviceError::Unsupported(_)) => Ok(ProgressOutcome::Unsupported),
        Err(DeviceError::InvalidInput(_)) => Err(OUT_OF_RANGE_MESSAGE.to_string()),
        Err(_) => Err("OS の進捗表示に失敗しました".to_string()),
    }
}

/// 注意喚起の結果を画面用へ写像する（固定文言のみ）。
fn to_attention_outcome(result: Result<(), DeviceError>) -> Result<AttentionOutcome, String> {
    match result {
        Ok(()) => Ok(AttentionOutcome::Submitted),
        Err(DeviceError::Unsupported(_)) => Ok(AttentionOutcome::Unsupported),
        Err(_) => Err("OS の注意喚起に失敗しました".to_string()),
    }
}

/// 要求を観測して検査し、`imp` を同期に呼んで結果を返す。`focused` はこのフレームの
/// ウィンドウのフォーカス（OS の観測値）。
fn drive(
    state: &AppProgressAttentionState,
    sender: &CompletionSender<AppProgressAttentionState>,
    imp: &dyn AppProgressAttention,
    focused: bool,
) {
    if let Some(request) = take_progress_request(state) {
        let _ = sender.send(Box::new(mark_progress_setting));
        // 範囲外の値は OS へ渡さずその場で画面へ返す。
        let result = match request.op {
            ProgressOp::Set(v) => ProgressValue::new(v).and_then(|v| imp.set_progress(v)),
            ProgressOp::Clear => imp.clear_progress(),
        };
        let outcome = to_progress_outcome(request.op, result);
        let generation = request.generation;
        let _ = sender.send(Box::new(move |s: &mut AppProgressAttentionState| {
            finish_progress(s, generation, outcome);
        }));
    }
    if let Some(generation) = take_attention_request(state, focused) {
        let _ = sender.send(Box::new(mark_attention_requesting));
        let outcome = to_attention_outcome(imp.request_attention());
        let _ = sender.send(Box::new(move |s: &mut AppProgressAttentionState| {
            finish_attention(s, generation, outcome);
        }));
    }
}

/// OS 別の [`AppProgressAttention`] 実装を返す。iOS / Android / Linux は非対応。
fn platform() -> Box<dyn AppProgressAttention> {
    #[cfg(target_os = "macos")]
    return super::macos_app_progress_attention::implementation();
    #[cfg(target_os = "windows")]
    return super::windows_app_progress_attention::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(fandhe_device::app_progress_attention::UnsupportedAppProgressAttention);
}

/// `registry.rs` から配線される effect ドライバ。
///
/// ponytail: 進捗設定・注意喚起は OS への 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。
/// 戻らない呼び出しが見つかったら作業スレッドへ移す。
pub fn app_progress_attention_show_effect(
    state: &AppProgressAttentionState,
    sender: &CompletionSender<AppProgressAttentionState>,
) {
    let focused = sender.focused();
    if take_progress_request(state).is_none() && take_attention_request(state, focused).is_none() {
        return;
    }
    drive(state, sender, platform().as_ref(), focused);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::app_progress_attention_show::{
        arm_attention, cancel_attention, clear_progress, set_progress, AttentionPhase,
        ProgressPhase, ATTENTION_CANCELLED_STATUS, ATTENTION_SUBMITTED_STATUS,
        PROGRESS_CLEARED_STATUS, PROGRESS_SUBMITTED_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::app_progress_attention::UnsupportedAppProgressAttention;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 呼び出し回数を数え、決めた結果を返す偽実装。
    struct Fake {
        progress: AtomicU32,
        attention: AtomicU32,
        result: fn() -> Result<(), DeviceError>,
    }

    impl AppProgressAttention for Fake {
        fn set_progress(&self, _v: ProgressValue) -> Result<(), DeviceError> {
            self.progress.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
        fn clear_progress(&self) -> Result<(), DeviceError> {
            self.progress.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
        fn request_attention(&self) -> Result<(), DeviceError> {
            self.attention.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<(), DeviceError>) -> Fake {
        Fake {
            progress: AtomicU32::new(0),
            attention: AtomicU32::new(0),
            result,
        }
    }

    /// 1 フレーム回して、完了通知を状態へ反映する。
    fn frame(s: &mut AppProgressAttentionState, imp: &dyn AppProgressAttention, focused: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive(s, &sender, imp, focused);
        while let Ok(apply) = rx.try_recv() {
            apply(s);
        }
    }

    fn with_input(input: &str) -> AppProgressAttentionState {
        AppProgressAttentionState {
            input: input.into(),
            ..Default::default()
        }
    }

    #[test]
    fn mapping_never_leaks_os_detail() {
        use ProgressOp::{Clear, Set};
        assert_eq!(
            to_progress_outcome(Set(3), Ok(())),
            Ok(ProgressOutcome::Submitted)
        );
        assert_eq!(
            to_progress_outcome(Clear, Ok(())),
            Ok(ProgressOutcome::Cleared)
        );
        assert_eq!(
            to_progress_outcome(Set(3), Err(DeviceError::Unsupported("x"))),
            Ok(ProgressOutcome::Unsupported)
        );
        assert_eq!(
            to_progress_outcome(Set(300), Err(DeviceError::InvalidInput("x"))),
            Err(OUT_OF_RANGE_MESSAGE.to_string())
        );
        let internal = to_progress_outcome(Set(3), Err(DeviceError::Internal("secret")));
        assert!(!internal.unwrap_err().contains("secret"));
        let attention = to_attention_outcome(Err(DeviceError::Internal("secret")));
        assert!(!attention.unwrap_err().contains("secret"));
    }

    #[test]
    fn no_request_never_calls_os() {
        let f = fake(|| Ok(()));
        let mut s = AppProgressAttentionState::default();
        frame(&mut s, &f, false);
        assert_eq!(f.progress.load(Ordering::SeqCst), 0);
        assert_eq!(f.attention.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn progress_set_and_clear_are_shown() {
        let f = fake(|| Ok(()));
        let mut s = with_input("40");
        set_progress(&mut s);
        frame(&mut s, &f, true);
        assert_eq!(s.progress_phase, ProgressPhase::Idle);
        assert_eq!(s.progress_status, PROGRESS_SUBMITTED_STATUS);
        clear_progress(&mut s);
        frame(&mut s, &f, true);
        assert_eq!(s.progress_status, PROGRESS_CLEARED_STATUS);
        assert_eq!(f.progress.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn out_of_range_never_reaches_os() {
        let f = fake(|| Ok(()));
        let mut s = with_input("101");
        set_progress(&mut s);
        frame(&mut s, &f, true);
        assert_eq!(f.progress.load(Ordering::SeqCst), 0);
        assert!(s.progress_error.contains(OUT_OF_RANGE_MESSAGE));
        assert!(s.progress_status.is_empty());
    }

    #[test]
    fn attention_waits_for_focus_loss_and_fires_once() {
        let f = fake(|| Ok(()));
        let mut s = AppProgressAttentionState::default();
        arm_attention(&mut s);
        frame(&mut s, &f, true);
        assert_eq!(f.attention.load(Ordering::SeqCst), 0);
        assert_eq!(s.attention_phase, AttentionPhase::Armed);
        frame(&mut s, &f, false);
        assert_eq!(f.attention.load(Ordering::SeqCst), 1);
        assert_eq!(s.attention_status, ATTENTION_SUBMITTED_STATUS);
        frame(&mut s, &f, false);
        assert_eq!(f.attention.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_wait_never_calls_os() {
        let f = fake(|| Ok(()));
        let mut s = AppProgressAttentionState::default();
        arm_attention(&mut s);
        cancel_attention(&mut s);
        frame(&mut s, &f, false);
        assert_eq!(f.attention.load(Ordering::SeqCst), 0);
        assert_eq!(s.attention_status, ATTENTION_CANCELLED_STATUS);
        assert!(s.attention_error.is_empty());
    }

    #[test]
    fn unsupported_and_internal_go_to_error() {
        let mut s = with_input("5");
        set_progress(&mut s);
        arm_attention(&mut s);
        frame(&mut s, &UnsupportedAppProgressAttention, false);
        assert_eq!(s.progress_error, UNSUPPORTED_STATUS);
        assert_eq!(s.attention_error, UNSUPPORTED_STATUS);
        assert!(s.progress_status.is_empty() && s.attention_status.is_empty());

        let f = fake(|| Err(DeviceError::Internal("secret")));
        let mut s = with_input("5");
        set_progress(&mut s);
        arm_attention(&mut s);
        frame(&mut s, &f, false);
        assert!(!s.progress_error.is_empty() && !s.progress_error.contains("secret"));
        assert!(!s.attention_error.is_empty() && !s.attention_error.contains("secret"));
        assert!(s.progress_status.is_empty() && s.attention_status.is_empty());
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let mut s = with_input("5");
        set_progress(&mut s);
        mark_progress_setting(&mut s);
        let stale = s.progress_generation.wrapping_sub(1);
        finish_progress(&mut s, stale, Ok(ProgressOutcome::Submitted));
        assert_eq!(s.progress_phase, ProgressPhase::Setting);
        assert!(s.progress_status.is_empty());
    }
}
