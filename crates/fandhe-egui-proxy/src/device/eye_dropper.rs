//! Phase 11 の拡充ループ型サンプル「画面のスポイト」（親 #148、Issue #762 macOS）の effect ドライバ。
//! `fandhe_core::screens::eye_dropper_pick`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::eye_dropper` のモジュール doc。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # macOS: pull 型 `bridge`
//!
//! `NSColorSampler` のハンドラは非同期に届くため、`EyeDropper` トレイトを通さず
//! `device/request_slot.rs::RequestSlot` の facade で受ける。画面への反映は毎フレーム
//! [`drive_callback`] が行う。
//!
//! # macOS 以外
//!
//! `UnsupportedEyeDropper` 経由で `Unsupported` を画面へ出す（成功にしない）。Web は
//! `fandhe-web-dom` が持つ（#763）。後続の子はこのファイルの dispatch・cfg・fallback を触らない。

use fandhe_core::screens::eye_dropper_pick::{EyeDropperOutcome, EyeDropperState};
use fandhe_device::eye_dropper::PickedColor;
use fandhe_device::DeviceError;

#[cfg(not(target_os = "macos"))]
use fandhe_core::screens::eye_dropper_pick::{finish_pick, mark_picking, take_request};
#[cfg(not(target_os = "macos"))]
use fandhe_device::eye_dropper::EyeDropper;

use crate::app::CompletionSender;

/// `Result<PickedColor, DeviceError>` を画面用の結果へ畳む。権限拒否が来ても成功にせず
/// `Failed` へ畳む（この機能は権限を要さない）。表示文字列は内部識別子のみで色は入らない。
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn to_outcome(result: Result<PickedColor, DeviceError>) -> EyeDropperOutcome {
    match result {
        Ok(c) => EyeDropperOutcome::Picked(c.hex()),
        Err(DeviceError::Cancelled(_)) => EyeDropperOutcome::Cancelled,
        Err(DeviceError::Unsupported(_)) => EyeDropperOutcome::Unsupported,
        Err(e) => EyeDropperOutcome::Failed(e.to_string()),
    }
}

/// macOS: OS コールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
#[cfg(target_os = "macos")]
pub(crate) mod bridge {
    use fandhe_core::screens::eye_dropper_pick::EyeDropperOutcome;
    use fandhe_device::eye_dropper::PickedColor;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) static SLOT: RequestSlot<EyeDropperOutcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く（main queue から届く契約）。`request_id` 不一致・
    /// 未登録・既に結果ありは捨てて `false`（古い世代・二重通知が成功へ化けない）。
    pub(crate) fn deliver(request_id: u64, result: Result<PickedColor, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }
}

/// macOS の effect ドライバ本体（`contact_pick::drive_callback` と同型）。
#[cfg(target_os = "macos")]
fn drive_callback(state: &EyeDropperState, sender: &CompletionSender<EyeDropperState>) {
    use fandhe_core::screens::eye_dropper_pick::{
        finish_pick, mark_picking, take_request, PickPhase,
    };

    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_picking));
        let request_id = bridge::begin();
        if !super::macos_eye_dropper::present(request_id) {
            // 起動依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(DeviceError::Internal("eye_dropper present")),
            );
        }
        return;
    }

    if state.phase != PickPhase::Picking {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut EyeDropperState, outcome| {
            // `Idle` へ既に戻っていたら反映せず、`RequestSlot` を残す（clear しない）。
            if s.phase != PickPhase::Idle {
                finish_pick(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// macOS 以外: `EyeDropper` トレイト経由の同期呼び出し。
#[cfg(not(target_os = "macos"))]
fn drive(
    state: &EyeDropperState,
    sender: &CompletionSender<EyeDropperState>,
    dropper: &dyn EyeDropper,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_picking));
    let outcome = to_outcome(dropper.pick());
    let _ = sender.send(Box::new(move |s: &mut EyeDropperState| {
        finish_pick(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `eye_dropper` 画面用に配線される effect ドライバ。
pub fn eye_dropper_pick_effect(
    state: &EyeDropperState,
    sender: &CompletionSender<EyeDropperState>,
) {
    #[cfg(target_os = "macos")]
    drive_callback(state, sender);
    #[cfg(not(target_os = "macos"))]
    drive(
        state,
        sender,
        &fandhe_device::eye_dropper::UnsupportedEyeDropper,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(hex: &str) -> PickedColor {
        fandhe_device::eye_dropper::color_from_srgb_hex(hex).unwrap()
    }

    #[test]
    fn to_outcome_maps_all_branches() {
        assert_eq!(
            to_outcome(Ok(color("#AABBCC"))),
            EyeDropperOutcome::Picked("#aabbcc".into())
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Cancelled("eye_dropper"))),
            EyeDropperOutcome::Cancelled
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("eye_dropper"))),
            EyeDropperOutcome::Unsupported
        );
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("x"))),
            EyeDropperOutcome::Failed(_)
        ));
        // 権限拒否が来ても成功にならない。
        assert!(matches!(
            to_outcome(Err(DeviceError::PermissionDenied("eye_dropper"))),
            EyeDropperOutcome::Failed(_)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stale_and_duplicate_delivery_never_becomes_success() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id.wrapping_add(1), Ok(color("#aabbcc"))));
        assert!(bridge::deliver(
            id,
            Err(DeviceError::Cancelled("eye_dropper"))
        ));
        // 二重通知は first-wins で、成功へ化けない。
        assert!(!bridge::deliver(id, Ok(color("#aabbcc"))));
        assert_eq!(
            bridge::SLOT.snapshot(),
            Some((id, Some(EyeDropperOutcome::Cancelled)))
        );
        // 新しい begin 後の旧 id の遅延完了も成功へ化けない。
        let old = bridge::begin();
        let _new = bridge::begin();
        assert!(!bridge::deliver(old, Ok(color("#aabbcc"))));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_is_reflected_not_success() {
        use fandhe_core::screens::eye_dropper_pick::{request_pick, PickPhase, UNSUPPORTED_TEXT};
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, ctx);
        let mut state = EyeDropperState::default();
        request_pick(&mut state);
        eye_dropper_pick_effect(&state, &sender);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut state);
        }
        assert_eq!(state.phase, PickPhase::Idle);
        assert!(state.picked.is_none());
        assert_eq!(state.error, UNSUPPORTED_TEXT);
    }
}
