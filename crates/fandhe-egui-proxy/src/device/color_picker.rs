//! Phase 11 の拡充ループ型サンプル「システムの色選択ダイアログ」（親 #184、Issue #941 (a)）の
//! effect ドライバ。`fandhe_core::screens::color_picker_pick`（端末依存ゼロ）の要求を観測し、
//! 結果を `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::color_picker` のモジュール doc。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # dispatch（最終形）
//!
//! - macOS・iOS: コールバック型。pull 型 `bridge`（`RequestSlot` の facade）と [`drive_callback`]。
//!   OS 別の `present` は macOS が `macos_color_picker`、iOS が `ios_color_picker`
//! - Windows: `ColorPicker` トレイトを UI スレッドで同期に呼ぶ [`drive`]（`windows_color_picker`、
//!   print と同じ同期モーダル）
//! - Android・Linux: `UnsupportedColorPicker` で `Unsupported` を画面へ出す（成功にしない）
//! - Web は対象外。`fandhe-web-dom` が持つ（#943）
//!
//! 後続の子（#942・#943）はこのファイルの dispatch・cfg・fallback を触らない。

use fandhe_core::screens::color_picker_pick::{ColorPickerOutcome, ColorPickerState};
use fandhe_device::eye_dropper::PickedColor;
use fandhe_device::DeviceError;

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
use fandhe_core::screens::color_picker_pick::{finish_pick, mark_picking, take_request};
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
use fandhe_device::color_picker::ColorPicker;

use crate::app::CompletionSender;

/// `Result<PickedColor, DeviceError>` を画面用の結果へ畳む。権限拒否が来ても成功にせず
/// `Failed` へ畳む（この機能は権限を要さない）。表示文字列は内部識別子のみで色は入らない。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
fn to_outcome(result: Result<PickedColor, DeviceError>) -> ColorPickerOutcome {
    match result {
        Ok(c) => ColorPickerOutcome::Picked(c.rgb()),
        Err(DeviceError::Cancelled(_)) => ColorPickerOutcome::Cancelled,
        Err(DeviceError::Unsupported(_)) => ColorPickerOutcome::Unsupported,
        Err(e) => ColorPickerOutcome::Failed(e.to_string()),
    }
}

/// macOS・iOS: OS コールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod bridge {
    use fandhe_core::screens::color_picker_pick::ColorPickerOutcome;
    use fandhe_device::eye_dropper::PickedColor;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) static SLOT: RequestSlot<ColorPickerOutcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは捨てて
    /// `false`（古い世代・二重通知が成功へ化けない）。
    pub(crate) fn deliver(request_id: u64, result: Result<PickedColor, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }
}

/// 提示を OS 別の実装へ依頼する。`Err` は同期的な失敗（コールバックは二度と来ない）。
#[cfg(target_os = "macos")]
fn present(request_id: u64) -> Result<(), DeviceError> {
    super::macos_color_picker::present(request_id)
}

/// 提示を OS 別の実装へ依頼する。
#[cfg(target_os = "ios")]
fn present(request_id: u64) -> Result<(), DeviceError> {
    super::ios_color_picker::present(request_id)
}

/// macOS・iOS の effect ドライバ本体（`eye_dropper::drive_callback` と同型）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn drive_callback(state: &ColorPickerState, sender: &CompletionSender<ColorPickerState>) {
    use fandhe_core::screens::color_picker_pick::{
        finish_pick, mark_picking, take_request, PickPhase,
    };

    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_picking));
        let request_id = bridge::begin();
        if let Err(e) = present(request_id) {
            bridge::deliver(request_id, Err(e));
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
        move |s: &mut ColorPickerState, outcome| {
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

/// macOS・iOS 以外: `ColorPicker` トレイト経由の同期呼び出し（モーダル中は描画が止まる）。
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn drive(
    state: &ColorPickerState,
    sender: &CompletionSender<ColorPickerState>,
    picker: &dyn ColorPicker,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_picking));
    let outcome = to_outcome(picker.pick());
    let _ = sender.send(Box::new(move |s: &mut ColorPickerState| {
        finish_pick(s, generation, outcome);
    }));
}

/// Windows は実装、それ以外（Android・Linux）は `Unsupported` 固定。
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn platform_picker() -> Box<dyn ColorPicker> {
    #[cfg(target_os = "windows")]
    return super::windows_color_picker::implementation();
    #[cfg(not(target_os = "windows"))]
    Box::new(fandhe_device::color_picker::UnsupportedColorPicker)
}

/// `registry.rs` の `effect` 区画から `color_picker` 画面用に配線される effect ドライバ。
pub fn color_picker_pick_effect(
    state: &ColorPickerState,
    sender: &CompletionSender<ColorPickerState>,
) {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    drive_callback(state, sender);
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    drive(state, sender, platform_picker().as_ref());
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
            ColorPickerOutcome::Picked(0xAABBCC)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Cancelled("color_picker"))),
            ColorPickerOutcome::Cancelled
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("color_picker"))),
            ColorPickerOutcome::Unsupported
        );
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("x"))),
            ColorPickerOutcome::Failed(_)
        ));
        // 権限拒否が来ても成功にならない。
        assert!(matches!(
            to_outcome(Err(DeviceError::PermissionDenied("color_picker"))),
            ColorPickerOutcome::Failed(_)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stale_and_duplicate_delivery_never_becomes_success() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id.wrapping_add(1), Ok(color("#aabbcc"))));
        assert!(bridge::deliver(
            id,
            Err(DeviceError::Cancelled("color_picker"))
        ));
        // 二重通知は first-wins で、成功へ化けない。
        assert!(!bridge::deliver(id, Ok(color("#aabbcc"))));
        assert_eq!(
            bridge::SLOT.snapshot(),
            Some((id, Some(ColorPickerOutcome::Cancelled)))
        );
        // 新しい begin 後の旧 id の遅延完了も成功へ化けない。
        let old = bridge::begin();
        let _new = bridge::begin();
        assert!(!bridge::deliver(old, Ok(color("#aabbcc"))));
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
    #[test]
    fn unsupported_is_reflected_not_success() {
        use fandhe_core::screens::color_picker_pick::{request_pick, PickPhase, UNSUPPORTED_TEXT};
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, ctx);
        let mut state = ColorPickerState::default();
        request_pick(&mut state);
        color_picker_pick_effect(&state, &sender);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut state);
        }
        assert_eq!(state.phase, PickPhase::Idle);
        assert!(state.picked.is_none());
        assert_eq!(state.error, UNSUPPORTED_TEXT);
    }
}
