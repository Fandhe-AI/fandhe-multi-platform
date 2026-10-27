//! Phase 11（親 #183、Issue #939）「システムの効果音・ビープ」の effect ドライバ。
//! `fandhe_core::screens::system_sound_beep`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::system_sound::SystemSound` で OS 標準の警告音 1 回を要求して
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約）。契約の SSOT は
//! `fandhe_device::system_sound`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる（`device.rs` 参照）。
//!
//! # スレッド
//!
//! 全 OS で呼ばれたスレッド（egui の update ループ = main）から同期で呼ぶ。macOS の `.m` は
//! main 以外なら触らず失敗を返す。Windows の `PlaySoundW` は `SND_ASYNC` で即座に戻るため
//! ワーカーは置かない。期限・タイマー・完了待ちは持たない。
//!
//! # 対応 OS
//!
//! macOS: [`super::macos_system_sound`]、Windows: [`super::windows_system_sound`]、
//! Android: 骨格のスタブ [`super::android_system_sound`]（#940 が実体にする）。
//! iOS（OS 標準の音を鳴らせない）・Linux は `UnsupportedSystemSound`。

use std::sync::Arc;

use fandhe_core::screens::system_sound_beep::{
    finish_beep, mark_sending, take_request, BeepFailure, BeepResult, SystemSoundBeepState,
};
use fandhe_device::system_sound::{BeepOutcome, SystemSound};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `SystemSound` 実装を返す（状態を持たないため毎回作る）。
fn implementation() -> Arc<dyn SystemSound> {
    #[cfg(target_os = "macos")]
    {
        super::macos_system_sound::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_system_sound::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_system_sound::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    {
        Arc::new(fandhe_device::system_sound::UnsupportedSystemSound)
    }
}

/// `DeviceError` を画面の失敗種別へ写す。`Unsupported` 以外は `Display` の固定文言のみ転送する
/// （security.md A09）。
fn to_failure(e: DeviceError) -> BeepFailure {
    match e {
        DeviceError::Unsupported(_) => BeepFailure::Unsupported,
        other => BeepFailure::Failed(other.to_string()),
    }
}

/// 成功系の結果を画面側の型へ 1:1 で写す。
fn to_result(outcome: BeepOutcome) -> BeepResult {
    match outcome {
        BeepOutcome::Requested => BeepResult::Requested,
        BeepOutcome::NotPlayed => BeepResult::NotPlayed,
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &SystemSoundBeepState,
    sender: &CompletionSender<SystemSoundBeepState>,
    sound: Arc<dyn SystemSound>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    // 受信側のドロップ（アプリ終了中）による送信失敗は無視してよい。
    let _ = sender.send(Box::new(mark_sending));

    let result = sound.beep().map(to_result).map_err(to_failure);
    let _ = sender.send(Box::new(move |s: &mut SystemSoundBeepState| {
        finish_beep(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `system_sound_beep` 画面用に配線される effect ドライバ。
pub fn system_sound_beep_effect(
    state: &SystemSoundBeepState,
    sender: &CompletionSender<SystemSoundBeepState>,
) {
    drive(state, sender, implementation());
}

/// Windows の `PlaySoundW` の戻り値（`TRUE` = OS に渡した / `FALSE` = OS は鳴らさなかった）を
/// 結果へ写す。`GetLastError` は見ず、OS の答えをそのまま返す。ホストでも検証できる純関数。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn outcome_from_play_sound(ok: bool) -> BeepOutcome {
    if ok {
        BeepOutcome::Requested
    } else {
        BeepOutcome::NotPlayed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::system_sound_beep::{
        beep, BeepPhase, FAILED_PREFIX, NOT_PLAYED_STATUS, REQUESTED_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::system_sound::UnsupportedSystemSound;

    type Sender = CompletionSender<SystemSoundBeepState>;

    fn sender_with_channel() -> (
        Sender,
        std::sync::mpsc::Receiver<crate::app::Completion<SystemSoundBeepState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake(fn() -> Result<BeepOutcome, DeviceError>);

    impl SystemSound for Fake {
        fn beep(&self) -> Result<BeepOutcome, DeviceError> {
            (self.0)()
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_sending と結果）をすべて適用した状態を返す。
    fn run(sound: Arc<dyn SystemSound>) -> SystemSoundBeepState {
        let (sender, rx) = sender_with_channel();
        let mut state = SystemSoundBeepState::default();
        beep(&mut state);
        drive(&state, &sender, sound);
        for _ in 0..2 {
            let c = rx.try_recv().expect("mark_sending と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        drive(
            &SystemSoundBeepState::default(),
            &sender,
            Arc::new(Fake(|| Ok(BeepOutcome::Requested))),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn requested_is_shown_as_handed_to_os() {
        let state = run(Arc::new(Fake(|| Ok(BeepOutcome::Requested))));
        assert_eq!(state.status, REQUESTED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn not_played_is_not_success() {
        let state = run(Arc::new(Fake(|| Ok(BeepOutcome::NotPlayed))));
        assert_eq!(state.status, NOT_PLAYED_STATUS);
        assert_ne!(state.status, REQUESTED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn unsupported_is_error_not_success() {
        let state = run(Arc::new(UnsupportedSystemSound));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.status, "");
        assert_eq!(state.phase, BeepPhase::Idle);
    }

    #[test]
    fn other_errors_never_show_success() {
        let cases: [fn() -> Result<BeepOutcome, DeviceError>; 3] = [
            || Err(DeviceError::Internal("system_sound")),
            || Err(DeviceError::PermissionDenied("system_sound")),
            || Err(DeviceError::Cancelled("system_sound")),
        ];
        for case in cases {
            let state = run(Arc::new(Fake(case)));
            assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
            assert_eq!(state.status, "");
        }
    }

    #[test]
    fn stale_generation_completion_is_discarded() {
        let (sender, rx) = sender_with_channel();
        let mut state = SystemSoundBeepState::default();
        beep(&mut state);
        drive(
            &state,
            &sender,
            Arc::new(Fake(|| Ok(BeepOutcome::Requested))),
        );
        // 完了が届く前に世代が進んだ状態を作る。
        let mut newer = SystemSoundBeepState {
            phase: BeepPhase::Sending,
            generation: 2,
            ..Default::default()
        };
        for _ in 0..2 {
            rx.try_recv().expect("届く")(&mut newer);
        }
        assert_eq!(newer.phase, BeepPhase::Sending);
        assert_eq!(newer.status, "");
    }

    #[test]
    fn play_sound_result_maps_both_values() {
        assert_eq!(outcome_from_play_sound(true), BeepOutcome::Requested);
        assert_eq!(outcome_from_play_sound(false), BeepOutcome::NotPlayed);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            implementation().beep(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
