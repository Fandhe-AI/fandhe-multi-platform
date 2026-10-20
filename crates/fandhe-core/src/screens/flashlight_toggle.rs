//! Phase 11（端末機能の拡充ループ、親 #106）型サンプル「ライト（トーチ）」（Issue #458）。
//! ボタンでライトを点灯 / 消灯し、**実際に点いているか**を表示する 1 画面。
//! `FANDHE_PROXY_APP=flashlight_toggle` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod flashlight_toggle;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない。時刻も持たず、期限は effect 側が測る）。
//! OS 呼び出しは本モジュール外（`fandhe-egui-proxy` の `device/flashlight.rs`、Web は
//! `fandhe-web-dom`）が [`take_request`] / [`confirm_target`] で状態を観測し、[`finish`] /
//! [`apply_read`] で結果を注入する（`wake_lock_toggle.rs` と同型）。
//!
//! # 表示は読み戻した値で決める
//!
//! 表示は要求ではなく OS から読み戻した [`FlashlightToggleState::on`] で決める
//! （`device-bridge-contracts.md`「6. 嘘の成功」）。`None` は「まだ分からない」で、消灯とは出さない。
//!
//! ```text
//! Idle --「点ける/消す」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_applying--> Applying
//!   --set 成功 → mark_confirming（世代一致のみ）--> Confirming
//!   --読み戻しが要求どおり／失敗／期限切れ → finish（世代一致のみ）--> Idle + 結果表示
//! Idle 中は apply_read（定期 pull）で on を更新する。世代・phase は動かさない。
//! ```

use crate::{Screen, WidgetId};

/// 「ライトを点ける」ボタンの `WidgetId`。
pub const ON_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「ライトを消す」ボタンの `WidgetId`。Web は `Screen` を再構築しないため両ボタンを常時出す
/// （`fandhe-web-dom` の demo が使うので `pub`）。
pub const OFF_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 画面タイトル。
pub const TITLE: &str = "ライト（トーチ）";
/// 点灯ボタンの文言。
pub const ON_LABEL: &str = "ライトを点ける";
/// 消灯ボタンの文言。
pub const OFF_LABEL: &str = "ライトを消す";
/// 点灯を読み戻せたときの案内。
pub const ON_STATUS: &str = "点灯中です";
/// 消灯を読み戻せたときの案内。
pub const OFF_STATUS: &str = "消灯しています";
/// まだ状態が分からないときの案内。
pub const READING_STATUS: &str = "状態を読み取っています";
/// 要求後に反映を確かめている間の案内。
pub const CONFIRMING_STATUS: &str = "状態を確かめています";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末ではライトを使えません";
/// 失敗時の案内の接頭辞（`{接頭辞}{理由}`）。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// 成功と返ったのに要求どおりになっていなかったときの理由（点灯・消灯の両方に使う）。
pub const NOT_CONFIRMED_REASON: &str = "点灯を確かめられませんでした";
/// 定期読み取りが失敗したときの固定文言。
pub const READ_FAILED: &str = "状態を読み取れませんでした";
/// 最小化時の消灯操作が失敗したときの固定文言。読み取り成功では消さず、消灯を読み戻せたときか
/// 次の明示操作の完了まで保持する。
pub const OFF_FAILED: &str = "消灯できませんでした";
/// 注記。
pub const NOTE: &str =
    "他のアプリがカメラを使っている間は点けられません。アプリを背景へ回すと消えます";
/// Web 向けの注記。
pub const WEB_NOTE: &str = "Android の Chrome などで動きます。カメラの許可を求めます";

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlashlightPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ要求を渡している最中。
    Applying,
    /// 要求は成功と返った。読み戻しが要求どおりになるのを待っている。
    Confirming,
}

/// 失敗の種別（`DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlashlightFailure {
    /// 非対応。
    Unsupported,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashlightRequest {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// `true` で点灯、`false` で消灯。
    pub on: bool,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlashlightToggleState {
    /// 操作の進行状態。
    pub phase: FlashlightPhase,
    /// 直近に立てた要求の世代番号。
    pub generation: u64,
    /// 処理中の要求の向き（`true` = 点灯）。
    pub requested_on: bool,
    /// OS から読み戻した点灯状態。`None` は未取得・読み取り失敗。
    pub on: Option<bool>,
    /// 非対応と分かったか。
    pub unsupported: bool,
    /// 失敗理由（`TextRole::Error` で表示）。空なら無し。
    pub error: String,
}

fn request(state: &mut FlashlightToggleState, on: bool) {
    if state.phase != FlashlightPhase::Idle {
        return;
    }
    state.phase = FlashlightPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.requested_on = on;
    state.error = String::new();
}

/// 「点ける」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn turn_on(state: &mut FlashlightToggleState) {
    request(state, true);
}

/// 「消す」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn turn_off(state: &mut FlashlightToggleState) {
    request(state, false);
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &FlashlightToggleState) -> Option<FlashlightRequest> {
    (state.phase == FlashlightPhase::Requested).then_some(FlashlightRequest {
        generation: state.generation,
        on: state.requested_on,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_applying(state: &mut FlashlightToggleState) {
    if state.phase == FlashlightPhase::Requested {
        state.phase = FlashlightPhase::Applying;
    }
}

/// OS への要求が成功したので確認待ちへ進める。`Applying` で世代が一致するときだけ。
pub fn mark_confirming(state: &mut FlashlightToggleState, generation: u64) {
    if state.phase == FlashlightPhase::Applying && state.generation == generation {
        state.phase = FlashlightPhase::Confirming;
    }
}

/// 確認待ちの要求を観測する getter（状態は変えない）。
pub fn confirm_target(state: &FlashlightToggleState) -> Option<FlashlightRequest> {
    (state.phase == FlashlightPhase::Confirming).then_some(FlashlightRequest {
        generation: state.generation,
        on: state.requested_on,
    })
}

/// 結果を反映する。`Idle` / `Requested` で届いた完了、世代が一致しない完了は捨てる。
///
/// `on` は完了直後に読み戻した値で、常に反映する。`Ok` でも `on` が要求と違えば失敗にする
/// （嘘の成功の禁止）。
pub fn finish(
    state: &mut FlashlightToggleState,
    generation: u64,
    result: Result<(), FlashlightFailure>,
    on: Option<bool>,
) {
    if matches!(
        state.phase,
        FlashlightPhase::Idle | FlashlightPhase::Requested
    ) || state.generation != generation
    {
        return;
    }
    state.phase = FlashlightPhase::Idle;
    state.on = on;
    let result = match result {
        Ok(()) if on != Some(state.requested_on) => {
            Err(FlashlightFailure::Failed(NOT_CONFIRMED_REASON.to_string()))
        }
        other => other,
    };
    match result {
        Ok(()) => {
            state.unsupported = false;
            state.error = String::new();
        }
        Err(FlashlightFailure::Unsupported) => {
            state.unsupported = true;
            state.error = String::new();
        }
        Err(FlashlightFailure::Failed(message)) => {
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 定期 pull の結果を反映する。世代・phase は動かさない。
pub fn apply_read(
    state: &mut FlashlightToggleState,
    result: Result<Option<bool>, FlashlightFailure>,
) {
    match result {
        Ok(v) => {
            state.on = v;
            state.unsupported = false;
            let off_failed = format!("{FAILED_PREFIX}{OFF_FAILED}");
            if state.error == format!("{FAILED_PREFIX}{READ_FAILED}")
                || (state.error == off_failed && v == Some(false))
            {
                state.error = String::new();
            }
        }
        Err(FlashlightFailure::Unsupported) => {
            state.on = None;
            state.unsupported = true;
        }
        Err(FlashlightFailure::Failed(_)) => {
            // 前回の値を出し続けない。
            state.on = None;
            // 消灯失敗の表示は読み取り失敗で上書きしない。
            if state.error != format!("{FAILED_PREFIX}{OFF_FAILED}") {
                state.error = format!("{FAILED_PREFIX}{READ_FAILED}");
            }
        }
    }
}

/// 案内文言。表示は要求ではなく読み戻した値で決める。
pub fn status_text(state: &FlashlightToggleState) -> String {
    match (state.phase, state.unsupported, state.on) {
        (FlashlightPhase::Applying | FlashlightPhase::Confirming, _, _) => CONFIRMING_STATUS,
        (_, true, _) => UNSUPPORTED_STATUS,
        (_, _, Some(true)) => ON_STATUS,
        (_, _, Some(false)) => OFF_STATUS,
        (_, _, None) => READING_STATUS,
    }
    .to_string()
}

/// 失敗時の案内文言（`TextRole::Error`）。
pub fn error_text(state: &FlashlightToggleState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときだけ両ボタンを出す（Web は再構築しないため常に両方）。
pub fn current_screen(state: &FlashlightToggleState) -> Screen<FlashlightToggleState> {
    let screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .text(NOTE);
    if state.phase != FlashlightPhase::Idle {
        return screen;
    }
    screen
        .button(ON_BUTTON_ID, ON_LABEL, turn_on)
        .button(OFF_BUTTON_ID, OFF_LABEL, turn_off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn applying(on: bool) -> FlashlightToggleState {
        let mut s = FlashlightToggleState::default();
        request(&mut s, on);
        mark_applying(&mut s);
        s
    }

    #[test]
    fn request_advances_generation_and_carries_direction() {
        let mut s = FlashlightToggleState::default();
        assert_eq!(take_request(&s), None);
        turn_on(&mut s);
        assert_eq!(
            take_request(&s),
            Some(FlashlightRequest {
                generation: 1,
                on: true
            })
        );
        mark_applying(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = applying(true);
        turn_off(&mut s);
        assert_eq!(s.phase, FlashlightPhase::Applying);
        mark_confirming(&mut s, 1);
        turn_off(&mut s);
        assert_eq!(s.phase, FlashlightPhase::Confirming);
        assert!(s.requested_on);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn confirming_requires_matching_generation() {
        let mut s = applying(true);
        mark_confirming(&mut s, 9);
        assert_eq!(s.phase, FlashlightPhase::Applying);
        assert_eq!(confirm_target(&s), None);
        mark_confirming(&mut s, 1);
        assert_eq!(
            confirm_target(&s),
            Some(FlashlightRequest {
                generation: 1,
                on: true
            })
        );
    }

    #[test]
    fn finish_ok_shows_on_and_off() {
        let mut s = applying(true);
        finish(&mut s, 1, Ok(()), Some(true));
        assert_eq!(status_text(&s), ON_STATUS);
        request(&mut s, false);
        mark_applying(&mut s);
        finish(&mut s, 2, Ok(()), Some(false));
        assert_eq!(status_text(&s), OFF_STATUS);
        assert_eq!(s.error, "");
    }

    #[test]
    fn finish_ok_but_mismatch_is_failure() {
        for read in [Some(false), None] {
            let mut s = applying(true);
            finish(&mut s, 1, Ok(()), read);
            assert_eq!(s.error, format!("{FAILED_PREFIX}{NOT_CONFIRMED_REASON}"));
            assert_ne!(status_text(&s), ON_STATUS);
        }
        let mut s = applying(false);
        finish(&mut s, 1, Ok(()), Some(true));
        assert!(s.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn finish_unsupported_is_not_success() {
        let mut s = applying(true);
        finish(&mut s, 1, Err(FlashlightFailure::Unsupported), None);
        assert_eq!(status_text(&s), UNSUPPORTED_STATUS);
        assert_ne!(status_text(&s), ON_STATUS);
    }

    #[test]
    fn finish_failure_uses_read_back_value() {
        let mut s = applying(true);
        finish(
            &mut s,
            1,
            Err(FlashlightFailure::Failed("x".into())),
            Some(false),
        );
        assert_eq!(s.on, Some(false));
        assert_eq!(s.error, format!("{FAILED_PREFIX}x"));
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = applying(true);
        finish(&mut s, 99, Ok(()), Some(true));
        assert_eq!(s.phase, FlashlightPhase::Applying);
        assert_eq!(s.on, None);
        let mut idle = FlashlightToggleState::default();
        finish(&mut idle, 0, Ok(()), Some(true));
        assert_eq!(idle.on, None);
        let mut req = FlashlightToggleState::default();
        turn_on(&mut req);
        finish(&mut req, 1, Ok(()), Some(true));
        assert_eq!(req.phase, FlashlightPhase::Requested);
    }

    #[test]
    fn apply_read_none_is_reading_not_off() {
        let mut s = FlashlightToggleState::default();
        apply_read(&mut s, Ok(None));
        assert_eq!(status_text(&s), READING_STATUS);
        apply_read(&mut s, Ok(Some(true)));
        assert_eq!(status_text(&s), ON_STATUS);
        assert_eq!(s.generation, 0);
        assert_eq!(s.phase, FlashlightPhase::Idle);
    }

    #[test]
    fn apply_read_failure_drops_previous_value_and_recovers() {
        let mut s = FlashlightToggleState::default();
        apply_read(&mut s, Ok(Some(true)));
        apply_read(&mut s, Err(FlashlightFailure::Failed("x".into())));
        assert_eq!(s.on, None);
        assert_eq!(s.error, format!("{FAILED_PREFIX}{READ_FAILED}"));
        apply_read(&mut s, Ok(Some(false)));
        assert_eq!(s.error, "");
        assert_eq!(status_text(&s), OFF_STATUS);
    }

    #[test]
    fn apply_read_keeps_operation_error() {
        let mut s = applying(true);
        finish(
            &mut s,
            1,
            Err(FlashlightFailure::Failed("x".into())),
            Some(false),
        );
        apply_read(&mut s, Ok(Some(false)));
        assert_eq!(s.error, format!("{FAILED_PREFIX}x"));
    }

    #[test]
    fn apply_read_unsupported_shows_unsupported() {
        let mut s = FlashlightToggleState::default();
        apply_read(&mut s, Err(FlashlightFailure::Unsupported));
        assert_eq!(status_text(&s), UNSUPPORTED_STATUS);
    }

    #[test]
    fn buttons_only_when_idle() {
        let mut s = FlashlightToggleState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, ON_LABEL, &mut s));
        let screen = current_screen(&s);
        assert!(!click_button(&screen, OFF_LABEL, &mut s));
        mark_applying(&mut s);
        mark_confirming(&mut s, 1);
        let screen = current_screen(&s);
        assert!(!click_button(&screen, OFF_LABEL, &mut s));
    }
}
