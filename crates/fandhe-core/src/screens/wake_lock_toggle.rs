//! Phase 11（端末機能の拡充ループ、親 #1129）型サンプル「画面スリープの抑止」（Issue #1278）。
//! ボタンで抑止を ON / OFF し、**実際に効いているか**を表示する 1 画面。
//! `FANDHE_PROXY_APP=wake_lock_toggle` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod wake_lock_toggle;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の `device/screen_wake_lock.rs`、Web は `fandhe-web-dom`）が
//! [`take_request`] で要求を観測し [`finish`] で結果を注入する（`haptics_tap.rs` と同型）。
//!
//! # 「要求した」と「効いている」を分ける
//!
//! 表示は要求（[`WakeLockToggleState::desired`]）ではなく、OS から読み戻した
//! [`WakeLockToggleState::held`] で決める（`device-bridge-contracts.md`「6. 嘘の成功」）。
//! 要求したのに効いていない間（Web のタブが隠れている間・Android の再生成直後など）は
//! [`PENDING_STATUS`] を出す。OS 側で非同期に起きる変化（Web の `release` イベント等）は
//! 後続の端末別実装が [`apply_held`] で注入する。
//!
//! ```text
//! Idle --「抑止する/やめる」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_applying--> phase=Applying
//!   --finish（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 「抑止する」ボタンの `WidgetId`。
pub const ON_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「抑止をやめる」ボタンの `WidgetId`。Web は `Screen` を再構築しないため両ボタンを常時出す
/// （`fandhe-web-dom` の `demo.rs` が使うので `pub`）。
pub const OFF_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 抑止が効いているときの案内。
pub const HELD_STATUS: &str = "スリープを抑止しています";
/// 抑止していないときの案内。
pub const RELEASED_STATUS: &str = "スリープを抑止していません";
/// 要求したが今は効いていないときの案内（Web / Android 向け）。
pub const PENDING_STATUS: &str = "抑止を要求していますが、今は効いていません（画面やタブが隠れている間など。戻ると取り直します）";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この端末ではスリープを抑止できません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// 取得は成功と返ったのに効いていなかったときの理由。
pub const NOT_HELD_REASON: &str = "抑止が効いていません";
/// 抑止を取るボタンの文言。
pub const ON_LABEL: &str = "スリープを抑止する";
/// 抑止を外すボタンの文言。
pub const OFF_LABEL: &str = "抑止をやめる";

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WakeLockPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ要求を渡している最中（[`mark_applying`] 済み）。
    Applying,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeLockFailure {
    /// 非対応。
    Unsupported,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeLockRequest {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// `true` で抑止を取り、`false` で外す。
    pub keep: bool,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WakeLockToggleState {
    /// 操作の進行状態。
    pub phase: WakeLockPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 処理中の要求の向き（`true` = 取る）。
    pub requested_keep: bool,
    /// 利用者が望んでいる状態（要求が成功したあとの向き）。
    pub desired: bool,
    /// OS から読み戻した「実際に効いているか」。表示はこちらで決める。
    pub held: bool,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする。
    pub error: String,
}

fn request(state: &mut WakeLockToggleState, keep: bool) {
    if state.phase != WakeLockPhase::Idle {
        return;
    }
    state.phase = WakeLockPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.requested_keep = keep;
    // 前回の失敗表示を残さない。
    state.error = String::new();
}

/// 「抑止する」ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn turn_on(state: &mut WakeLockToggleState) {
    request(state, true);
}

/// 「抑止をやめる」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn turn_off(state: &mut WakeLockToggleState) {
    request(state, false);
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &WakeLockToggleState) -> Option<WakeLockRequest> {
    (state.phase == WakeLockPhase::Requested).then_some(WakeLockRequest {
        generation: state.generation,
        keep: state.requested_keep,
    })
}

/// 要求を処理中（`Applying`）へ進める。`Requested` 以外では何もしない。
pub fn mark_applying(state: &mut WakeLockToggleState) {
    if state.phase == WakeLockPhase::Requested {
        state.phase = WakeLockPhase::Applying;
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
///
/// `held` は完了直後に OS から読み戻した値。取得が `Ok` でも `held` が偽なら失敗として
/// 扱う（嘘の成功の禁止）。失敗時の `desired` は実際の `held` に合わせる。
pub fn finish(
    state: &mut WakeLockToggleState,
    generation: u64,
    result: Result<(), WakeLockFailure>,
    held: bool,
) {
    if state.phase == WakeLockPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = WakeLockPhase::Idle;
    state.held = held;
    let result = match result {
        Ok(()) if state.requested_keep && !held => {
            Err(WakeLockFailure::Failed(NOT_HELD_REASON.to_string()))
        }
        other => other,
    };
    match result {
        Ok(()) => {
            state.desired = state.requested_keep;
            state.error = String::new();
        }
        Err(WakeLockFailure::Unsupported) => {
            state.desired = held;
            state.error = UNSUPPORTED_STATUS.to_string();
        }
        Err(WakeLockFailure::Failed(message)) => {
            state.desired = held;
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// OS 側で非同期に起きた変化（Web の `release` / `visibilitychange`、Android の Activity
/// 再生成など）を反映する。後続の端末別実装が呼ぶ口。世代は進めない。
pub fn apply_held(state: &mut WakeLockToggleState, held: bool) {
    state.held = held;
}

/// 案内文言（`DynamicText` の `get`）。表示は `held` で決める。
pub fn status_text(state: &WakeLockToggleState) -> String {
    if state.held {
        HELD_STATUS
    } else if state.desired {
        PENDING_STATUS
    } else {
        RELEASED_STATUS
    }
    .to_string()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &WakeLockToggleState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) -> Screen<S>` として
/// 呼ばれる契約。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &WakeLockToggleState) -> Screen<WakeLockToggleState> {
    let screen = Screen::new("画面スリープの抑止")
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != WakeLockPhase::Idle {
        return screen;
    }
    if state.held || state.desired {
        screen.button(OFF_BUTTON_ID, OFF_LABEL, turn_off)
    } else {
        screen.button(ON_BUTTON_ID, ON_LABEL, turn_on)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn applying(keep: bool) -> WakeLockToggleState {
        let mut state = WakeLockToggleState::default();
        request(&mut state, keep);
        mark_applying(&mut state);
        state
    }

    #[test]
    fn request_advances_generation_and_carries_direction() {
        let mut state = WakeLockToggleState::default();
        assert_eq!(take_request(&state), None);
        turn_on(&mut state);
        assert_eq!(
            take_request(&state),
            Some(WakeLockRequest {
                generation: 1,
                keep: true
            })
        );
        mark_applying(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut state = applying(true);
        turn_off(&mut state);
        assert_eq!(state.phase, WakeLockPhase::Applying);
        assert_eq!(state.generation, 1);
        assert!(state.requested_keep);
    }

    #[test]
    fn finish_ok_held_shows_held() {
        let mut state = applying(true);
        finish(&mut state, 1, Ok(()), true);
        assert_eq!(state.phase, WakeLockPhase::Idle);
        assert_eq!(status_text(&state), HELD_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_ok_but_not_held_is_failure() {
        let mut state = applying(true);
        finish(&mut state, 1, Ok(()), false);
        assert_eq!(status_text(&state), RELEASED_STATUS);
        assert_eq!(state.error, format!("{FAILED_PREFIX}{NOT_HELD_REASON}"));
        assert!(!state.desired);
    }

    #[test]
    fn finish_release_ok_shows_released() {
        let mut state = applying(true);
        finish(&mut state, 1, Ok(()), true);
        request(&mut state, false);
        mark_applying(&mut state);
        finish(&mut state, 2, Ok(()), false);
        assert_eq!(status_text(&state), RELEASED_STATUS);
        assert!(!state.desired);
    }

    #[test]
    fn finish_acquire_failure_never_says_held() {
        let mut state = applying(true);
        finish(
            &mut state,
            1,
            Err(WakeLockFailure::Failed("x".into())),
            false,
        );
        assert_eq!(status_text(&state), RELEASED_STATUS);
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
    }

    #[test]
    fn finish_unsupported_is_error_not_success() {
        let mut state = applying(true);
        finish(&mut state, 1, Err(WakeLockFailure::Unsupported), false);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_ne!(status_text(&state), HELD_STATUS);
    }

    #[test]
    fn finish_release_failure_stays_held() {
        let mut state = applying(true);
        finish(&mut state, 1, Ok(()), true);
        request(&mut state, false);
        mark_applying(&mut state);
        finish(
            &mut state,
            2,
            Err(WakeLockFailure::Failed("y".into())),
            true,
        );
        assert_eq!(status_text(&state), HELD_STATUS);
        assert!(state.desired);
        assert!(state.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = applying(true);
        finish(&mut state, 99, Ok(()), true);
        assert_eq!(state.phase, WakeLockPhase::Applying);
        assert!(!state.held);
        let mut idle = WakeLockToggleState::default();
        finish(&mut idle, 0, Ok(()), true);
        assert!(!idle.held);
    }

    #[test]
    fn apply_held_shows_pending_when_desired_but_released() {
        let mut state = applying(true);
        finish(&mut state, 1, Ok(()), true);
        apply_held(&mut state, false);
        assert_eq!(status_text(&state), PENDING_STATUS);
        assert_eq!(state.generation, 1);
        apply_held(&mut state, true);
        assert_eq!(status_text(&state), HELD_STATUS);
    }

    #[test]
    fn button_flips_with_state_and_hides_in_flight() {
        let mut state = WakeLockToggleState::default();
        let screen = current_screen(&state);
        assert!(!click_button(&screen, OFF_LABEL, &mut state));
        assert!(click_button(&screen, ON_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, ON_LABEL, &mut state));
        mark_applying(&mut state);
        finish(&mut state, 1, Ok(()), true);
        let screen = current_screen(&state);
        assert!(click_button(&screen, OFF_LABEL, &mut state));
    }
}
