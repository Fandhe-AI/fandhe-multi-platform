//! Phase 11（端末機能の拡充ループ、親 #1133）型サンプル「全画面表示」（Issue #1312）。
//! ボタンで全画面 / 通常を切り替え、**実際の状態**を表示する 1 画面。
//! `FANDHE_PROXY_APP=fullscreen_toggle` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod fullscreen_toggle;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の `device/fullscreen_immersive.rs`、Web は `fandhe-web-dom`）が
//! [`take_request`] で要求を観測し、[`finish`] / [`apply_observed`] で読み戻した値を注入する
//! （`wake_lock_toggle.rs` と同型）。契約の SSOT は `fandhe_device::fullscreen_immersive`。
//!
//! # 「要求した」と「実際に全画面」を分ける
//!
//! 表示は要求ではなく、OS から読み戻した [`FullscreenToggleState::fullscreen`] で決める。
//! 切り替えは非同期に終わるため、要求中は [`SWITCHING_STATUS`] を出し、読み戻しが目標に一致した
//! ときだけ成功にする。外部からの変化（緑ボタン等）は [`apply_observed`] が反映する。
//!
//! ```text
//! Idle --「全画面にする/戻す」--> Requested, generation += 1
//!   --effect: take_request → mark_applying--> Applying
//!   --effect: 読み戻しが目標と一致 / 期限切れ / エラー → finish--> Idle
//! ```

use crate::{Screen, WidgetId};

/// 「全画面にする」ボタンの `WidgetId`。
pub const ENTER_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「通常の表示に戻す」ボタンの `WidgetId`（Web は両ボタンを常時出すため `pub`）。
pub const EXIT_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 全画面のときの案内。
pub const FULLSCREEN_STATUS: &str = "全画面で表示しています";
/// 通常表示のときの案内。
pub const WINDOWED_STATUS: &str = "通常の表示です";
/// 状態を読み取れないときの案内。
pub const UNKNOWN_STATUS: &str = "表示の状態を読み取れません";
/// 切り替え中の案内。
pub const SWITCHING_STATUS: &str = "切り替えています";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この端末では全画面に切り替えられません";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// 要求は受け付けられたが期限内に切り替わらなかったときの理由。
pub const NOT_SWITCHED_REASON: &str = "時間内に切り替わりませんでした";
/// 全画面にするボタンの文言。
pub const ENTER_LABEL: &str = "全画面にする";
/// 通常へ戻すボタンの文言。
pub const EXIT_LABEL: &str = "通常の表示に戻す";
/// OS ごとの戻り方の案内（後続の OS 実装が共通コア画面を触らないよう全 OS 分をここに置く）。
pub const HINT: &str = "戻し方: macOS はウィンドウ左上の緑ボタン、Android は画面端のスワイプでバーが一時的に出ます、Web は Esc キー";
/// 切り替えの確定を待つ期限（秒）。全 OS 共通の契約で、proxy と web-dom が参照する。
pub const CONFIRM_TIMEOUT_SECS: u64 = 3;

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FullscreenPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ依頼し、読み戻しで確定を待っている。
    Applying,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullscreenFailure {
    /// 非対応。
    Unsupported,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenRequest {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// `true` で全画面にし、`false` で通常へ戻す。
    pub on: bool,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FullscreenToggleState {
    /// 操作の進行状態。
    pub phase: FullscreenPhase,
    /// 直近に立てた要求の世代番号。
    pub generation: u64,
    /// 処理中の要求の向き（`true` = 全画面にする）。
    pub requested_on: bool,
    /// OS から読み戻した実際の状態。`None` は読めない。表示はこちらで決める。
    pub fullscreen: Option<bool>,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列。
    pub error: String,
}

fn request(state: &mut FullscreenToggleState, on: bool) {
    if state.phase != FullscreenPhase::Idle {
        return;
    }
    state.phase = FullscreenPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.requested_on = on;
    state.error = String::new();
}

/// 「全画面にする」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn enter(state: &mut FullscreenToggleState) {
    request(state, true);
}

/// 「通常の表示に戻す」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn exit(state: &mut FullscreenToggleState) {
    request(state, false);
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &FullscreenToggleState) -> Option<FullscreenRequest> {
    (state.phase == FullscreenPhase::Requested).then_some(FullscreenRequest {
        generation: state.generation,
        on: state.requested_on,
    })
}

/// 要求を処理中（`Applying`）へ進める。`Requested` 以外では何もしない。
pub fn mark_applying(state: &mut FullscreenToggleState) {
    if state.phase == FullscreenPhase::Requested {
        state.phase = FullscreenPhase::Applying;
    }
}

/// 結果を反映する。`Idle`、または世代が一致しない完了は無視する。
///
/// `observed` は完了時に読み戻した値。`Ok` でも `observed` が目標と一致しなければ失敗にする
/// （嘘の成功の禁止）。`fullscreen` は成否に関わらず `observed` で更新する。
pub fn finish(
    state: &mut FullscreenToggleState,
    generation: u64,
    result: Result<(), FullscreenFailure>,
    observed: Option<bool>,
) {
    if state.phase == FullscreenPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = FullscreenPhase::Idle;
    state.fullscreen = observed;
    let result = match result {
        Ok(()) if observed != Some(state.requested_on) => {
            Err(FullscreenFailure::Failed(NOT_SWITCHED_REASON.to_string()))
        }
        other => other,
    };
    state.error = match result {
        Ok(()) => String::new(),
        Err(FullscreenFailure::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        Err(FullscreenFailure::Failed(m)) => format!("{FAILED_PREFIX}{m}"),
    };
}

/// OS 側で起きた外部変化（緑ボタン等）を反映する。世代は進めない。
pub fn apply_observed(state: &mut FullscreenToggleState, observed: Option<bool>) {
    state.fullscreen = observed;
}

/// 案内文言。表示は読み戻した `fullscreen` で決める。
pub fn status_text(state: &FullscreenToggleState) -> String {
    if state.phase != FullscreenPhase::Idle {
        return SWITCHING_STATUS.to_string();
    }
    match state.fullscreen {
        Some(true) => FULLSCREEN_STATUS,
        Some(false) => WINDOWED_STATUS,
        None => UNKNOWN_STATUS,
    }
    .to_string()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &FullscreenToggleState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &FullscreenToggleState) -> Screen<FullscreenToggleState> {
    let screen = Screen::new("全画面表示")
        .dynamic_text(status_text)
        .error_text(error_text)
        .text(HINT);
    if state.phase != FullscreenPhase::Idle {
        return screen;
    }
    if state.fullscreen == Some(true) {
        screen.button(EXIT_BUTTON_ID, EXIT_LABEL, exit)
    } else {
        screen.button(ENTER_BUTTON_ID, ENTER_LABEL, enter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn applying(on: bool) -> FullscreenToggleState {
        let mut state = FullscreenToggleState::default();
        request(&mut state, on);
        mark_applying(&mut state);
        state
    }

    #[test]
    fn request_advances_generation_and_is_noop_in_flight() {
        let mut state = FullscreenToggleState::default();
        assert_eq!(take_request(&state), None);
        enter(&mut state);
        assert_eq!(
            take_request(&state),
            Some(FullscreenRequest {
                generation: 1,
                on: true
            })
        );
        mark_applying(&mut state);
        assert_eq!(take_request(&state), None);
        exit(&mut state);
        assert_eq!(state.generation, 1);
        assert!(state.requested_on);
    }

    #[test]
    fn finish_ok_matching_shows_fullscreen() {
        let mut state = applying(true);
        finish(&mut state, 1, Ok(()), Some(true));
        assert_eq!(status_text(&state), FULLSCREEN_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_ok_but_mismatch_or_unreadable_is_failure() {
        for observed in [Some(false), None] {
            let mut state = applying(true);
            finish(&mut state, 1, Ok(()), observed);
            assert_eq!(state.error, format!("{FAILED_PREFIX}{NOT_SWITCHED_REASON}"));
            assert_ne!(status_text(&state), FULLSCREEN_STATUS);
        }
    }

    #[test]
    fn finish_unsupported_is_error_not_success() {
        let mut state = applying(true);
        finish(&mut state, 1, Err(FullscreenFailure::Unsupported), None);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(status_text(&state), UNKNOWN_STATUS);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = applying(true);
        finish(&mut state, 99, Ok(()), Some(true));
        assert_eq!(state.phase, FullscreenPhase::Applying);
        assert_eq!(state.fullscreen, None);
        let mut idle = FullscreenToggleState::default();
        finish(&mut idle, 0, Ok(()), Some(true));
        assert_eq!(idle.fullscreen, None);
    }

    #[test]
    fn apply_observed_follows_external_change_without_new_generation() {
        let mut state = applying(true);
        finish(&mut state, 1, Ok(()), Some(true));
        apply_observed(&mut state, Some(false));
        assert_eq!(status_text(&state), WINDOWED_STATUS);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn button_flips_with_state_and_hides_in_flight() {
        let mut state = FullscreenToggleState::default();
        let screen = current_screen(&state);
        assert!(!click_button(&screen, EXIT_LABEL, &mut state));
        assert!(click_button(&screen, ENTER_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, ENTER_LABEL, &mut state));
        mark_applying(&mut state);
        finish(&mut state, 1, Ok(()), Some(true));
        let screen = current_screen(&state);
        assert!(click_button(&screen, EXIT_LABEL, &mut state));
    }
}
