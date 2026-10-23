//! Phase 11（端末機能の拡充ループ、親 #127）型サンプル「ウィンドウ管理」（Issue #636）。
//! ウィンドウの位置・大きさ・最前面を **OS から読み戻した値** で表示し、ボタンで左上へ移動・
//! 既定サイズへ復帰・最前面の ON / OFF を行う 1 画面。
//! `FANDHE_PROXY_APP=window_management_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod window_management_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の `device/window_management.rs`、Web は `fandhe-web-dom`）が
//! [`take_request`] で要求を観測し、[`finish`] / [`apply_observed`] / [`apply_support`] で読み戻した値を
//! 注入する（`fullscreen_toggle.rs` と同型）。契約の SSOT は `fandhe_device::window_management`。
//! 後続の OS 実装（Windows #637・Web #638）が共通コア画面を触らないよう、状態・定数・文言は
//! 全対象 OS 分をここに置く。
//!
//! ```text
//! Idle --ボタン--> Requested, generation += 1
//!   --effect: take_request → mark_applying--> Applying
//!   --effect: 読み戻しが目標と一致 / 期限切れ / エラー → finish--> Idle
//! ```

use crate::{Screen, WidgetId};

/// 「左上へ移動」ボタンの `WidgetId`。
pub const MOVE_TOP_LEFT_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「既定サイズに戻す」ボタンの `WidgetId`。
pub const RESET_SIZE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「最前面にする」ボタンの `WidgetId`。
pub const ALWAYS_ON_TOP_ON_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
/// 「最前面をやめる」ボタンの `WidgetId`。
pub const ALWAYS_ON_TOP_OFF_BUTTON_ID: WidgetId = WidgetId::from_raw(4);

/// 既定の内側サイズ（幅, 高さ）。起動時のウィンドウサイズと「既定サイズに戻す」の目標の SSOT。
/// `fandhe-egui-proxy` の `main.rs`・effect ドライバ、`fandhe-web-dom` が参照する
/// （`fandhe-core` は `fandhe-device` に依存できないため、契約側ではなくここに置く）。
pub const DEFAULT_INNER_SIZE: [f32; 2] = [480.0, 360.0];
/// 操作の確定を待つ期限（秒）。全 OS 共通の契約で、proxy と web-dom が参照する。
pub const CONFIRM_TIMEOUT_SECS: u64 = 3;

/// 左上へ移動するボタンの文言。
pub const MOVE_TOP_LEFT_LABEL: &str = "左上へ移動";
/// 既定サイズへ戻すボタンの文言。
pub const RESET_SIZE_LABEL: &str = "既定サイズに戻す";
/// 最前面にするボタンの文言。
pub const ALWAYS_ON_TOP_ON_LABEL: &str = "最前面にする";
/// 最前面をやめるボタンの文言。
pub const ALWAYS_ON_TOP_OFF_LABEL: &str = "最前面をやめる";

/// 位置・大きさを読み取れないときの案内（最小化中を含む）。
pub const FRAME_UNKNOWN_STATUS: &str = "位置と大きさを読み取れません";
/// 最前面の状態が ON のときの案内。
pub const TOP_ON_STATUS: &str = "最前面: オン";
/// 最前面の状態が OFF のときの案内。
pub const TOP_OFF_STATUS: &str = "最前面: オフ";
/// 最前面の状態を読み取れないときの案内。
pub const TOP_UNKNOWN_STATUS: &str = "最前面: 読み取れません";
/// 最前面を提供しない環境（Web 等）の案内。
pub const TOP_UNAVAILABLE_STATUS: &str = "この環境では最前面にできません";
/// 反映中の案内。
pub const APPLYING_STATUS: &str = "反映しています";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この端末ではウィンドウを動かせません";
/// ブラウザのタブで開いているときの案内（Web）。
pub const TAB_STATUS: &str =
    "ブラウザのタブでは位置と大きさを変えられません（インストールしたアプリの窓で開いてください）";
/// 全画面中に断るときの理由。
pub const FULLSCREEN_REFUSED_REASON: &str = "全画面中は動かせません";
/// 要求は受け付けられたが期限内に反映されなかったときの理由。
pub const NOT_APPLIED_REASON: &str = "時間内に反映されませんでした";
/// 状態を読み取れず依頼を出さなかったときの理由。
pub const UNREADABLE_REASON: &str = "ウィンドウの状態を読み取れません";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// OS ごとの注意（後続の OS 実装が共通コア画面を触らないよう全 OS 分をここに置く）。
pub const HINT: &str = "注意: Windows は外枠の見えない縁のため左上へ寄せた見た目が数 px 内側になります。Web は Chromium 系でインストールしたアプリの窓のみ動かせます（Safari と Firefox は非対応）";

/// 画面が受け付ける操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// 今いる画面の作業領域の左上へ移動する。
    MoveTopLeft,
    /// 内側を [`DEFAULT_INNER_SIZE`] へ戻す。
    ResetSize,
    /// 最前面の ON（`true`）/ OFF（`false`）。
    SetAlwaysOnTop(bool),
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowPhase {
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
pub enum WindowFailure {
    /// 非対応。
    Unsupported,
    /// 全画面中のため依頼を出さなかった。
    FullscreenRefused,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// OS から読み戻した値（`fandhe_device` 型の写し。写す責務は effect 側）。`None` は読めない。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowObserved {
    /// 外枠の左上 `(x, y)`。
    pub position: Option<(f64, f64)>,
    /// 内側の大きさ `(幅, 高さ)`。
    pub inner_size: Option<(f64, f64)>,
    /// 最前面か。
    pub always_on_top: Option<bool>,
    /// 全画面か。
    pub fullscreen: Option<bool>,
}

/// 実装が提供できる操作の写し。`None` は未取得。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowSupportView {
    /// 位置・大きさを変えられる。
    pub move_resize: Option<bool>,
    /// 最前面を切り替えられる。
    pub always_on_top: Option<bool>,
    /// ブラウザのタブで開いている（Web のみ `true` になりうる）。
    pub in_browser_tab: bool,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRequest {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 依頼する操作。
    pub action: WindowAction,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WindowManagementState {
    /// 操作の進行状態。
    pub phase: WindowPhase,
    /// 直近に立てた要求の世代番号。
    pub generation: u64,
    /// 処理中の操作。
    pub requested: Option<WindowAction>,
    /// OS から読み戻した実際の状態。表示はこちらで決める。
    pub observed: WindowObserved,
    /// 提供できる操作。
    pub support: WindowSupportView,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列。
    pub error: String,
}

fn request(state: &mut WindowManagementState, action: WindowAction) {
    if state.phase != WindowPhase::Idle {
        return;
    }
    state.phase = WindowPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.requested = Some(action);
    state.error = String::new();
}

/// 「左上へ移動」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn move_top_left(state: &mut WindowManagementState) {
    request(state, WindowAction::MoveTopLeft);
}

/// 「既定サイズに戻す」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn reset_size(state: &mut WindowManagementState) {
    request(state, WindowAction::ResetSize);
}

/// 「最前面にする」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn always_on_top_on(state: &mut WindowManagementState) {
    request(state, WindowAction::SetAlwaysOnTop(true));
}

/// 「最前面をやめる」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn always_on_top_off(state: &mut WindowManagementState) {
    request(state, WindowAction::SetAlwaysOnTop(false));
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &WindowManagementState) -> Option<WindowRequest> {
    if state.phase != WindowPhase::Requested {
        return None;
    }
    state.requested.map(|action| WindowRequest {
        generation: state.generation,
        action,
    })
}

/// 要求を処理中（`Applying`）へ進める。`Requested` 以外では何もしない。
pub fn mark_applying(state: &mut WindowManagementState) {
    if state.phase == WindowPhase::Requested {
        state.phase = WindowPhase::Applying;
    }
}

/// 操作に対応する観測値が読めているか。
fn observed_for(action: WindowAction, o: &WindowObserved) -> bool {
    match action {
        WindowAction::MoveTopLeft => o.position.is_some(),
        WindowAction::ResetSize => o.inner_size.is_some(),
        WindowAction::SetAlwaysOnTop(_) => o.always_on_top.is_some(),
    }
}

/// 結果を反映する。`Idle`、または世代が一致しない完了は無視する。
///
/// `observed` は完了時に読み戻した値で、成否に関わらず反映する。`Ok` でも操作に対応する観測値が
/// `None` なら失敗にする（嘘の成功の禁止。目標との一致判定は effect 側の `reached` が担う）。
pub fn finish(
    state: &mut WindowManagementState,
    generation: u64,
    result: Result<(), WindowFailure>,
    observed: WindowObserved,
) {
    if state.phase == WindowPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = WindowPhase::Idle;
    state.observed = observed;
    let readable = state
        .requested
        .map(|a| observed_for(a, &observed))
        .unwrap_or(false);
    let result = match result {
        Ok(()) if !readable => Err(WindowFailure::Failed(UNREADABLE_REASON.to_string())),
        other => other,
    };
    state.requested = None;
    state.error = match result {
        Ok(()) => String::new(),
        Err(WindowFailure::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        Err(WindowFailure::FullscreenRefused) => {
            format!("{FAILED_PREFIX}{FULLSCREEN_REFUSED_REASON}")
        }
        Err(WindowFailure::Failed(m)) => format!("{FAILED_PREFIX}{m}"),
    };
}

/// OS 側で起きた外部変化（ドラッグ・外部からの最前面変更等）を反映する。世代は進めない。
pub fn apply_observed(state: &mut WindowManagementState, observed: WindowObserved) {
    state.observed = observed;
}

/// 提供できる操作を反映する。世代は進めない。
pub fn apply_support(state: &mut WindowManagementState, support: WindowSupportView) {
    state.support = support;
}

fn round(v: f64) -> i64 {
    v.round() as i64
}

/// 位置と大きさの案内（整数に丸めて表示）。読めない項目があれば [`FRAME_UNKNOWN_STATUS`]。
pub fn frame_text(state: &WindowManagementState) -> String {
    match (state.observed.position, state.observed.inner_size) {
        (Some((x, y)), Some((w, h))) if [x, y, w, h].iter().all(|v| v.is_finite()) => format!(
            "位置: {}, {} / 大きさ: {} × {}",
            round(x),
            round(y),
            round(w),
            round(h)
        ),
        _ => FRAME_UNKNOWN_STATUS.to_string(),
    }
}

/// 最前面の案内。
pub fn top_status_text(state: &WindowManagementState) -> String {
    if state.support.always_on_top == Some(false) {
        return TOP_UNAVAILABLE_STATUS.to_string();
    }
    match state.observed.always_on_top {
        Some(true) => TOP_ON_STATUS,
        Some(false) => TOP_OFF_STATUS,
        None => TOP_UNKNOWN_STATUS,
    }
    .to_string()
}

/// 全体の案内。反映中は [`APPLYING_STATUS`]、非対応・タブは理由を出す。
pub fn status_text(state: &WindowManagementState) -> String {
    if state.phase != WindowPhase::Idle {
        return APPLYING_STATUS.to_string();
    }
    if state.support.in_browser_tab {
        return TAB_STATUS.to_string();
    }
    if state.support.move_resize == Some(false) && state.support.always_on_top == Some(false) {
        return UNSUPPORTED_STATUS.to_string();
    }
    frame_text(state)
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &WindowManagementState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみ、提供できる操作のボタンを出す。
pub fn current_screen(state: &WindowManagementState) -> Screen<WindowManagementState> {
    let mut screen = Screen::new("ウィンドウ管理")
        .dynamic_text(status_text)
        .dynamic_text(top_status_text)
        .error_text(error_text)
        .text(HINT);
    if state.phase != WindowPhase::Idle || state.support.in_browser_tab {
        return screen;
    }
    if state.support.move_resize == Some(true) {
        screen = screen
            .button(MOVE_TOP_LEFT_BUTTON_ID, MOVE_TOP_LEFT_LABEL, move_top_left)
            .button(RESET_SIZE_BUTTON_ID, RESET_SIZE_LABEL, reset_size);
    }
    if state.support.always_on_top == Some(true) {
        screen = if state.observed.always_on_top == Some(true) {
            screen.button(
                ALWAYS_ON_TOP_OFF_BUTTON_ID,
                ALWAYS_ON_TOP_OFF_LABEL,
                always_on_top_off,
            )
        } else {
            screen.button(
                ALWAYS_ON_TOP_ON_BUTTON_ID,
                ALWAYS_ON_TOP_ON_LABEL,
                always_on_top_on,
            )
        };
    }
    screen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn full_observed() -> WindowObserved {
        WindowObserved {
            position: Some((10.0, 20.0)),
            inner_size: Some((480.0, 360.0)),
            always_on_top: Some(false),
            fullscreen: Some(false),
        }
    }

    fn supported() -> WindowManagementState {
        WindowManagementState {
            support: WindowSupportView {
                move_resize: Some(true),
                always_on_top: Some(true),
                in_browser_tab: false,
            },
            observed: full_observed(),
            ..Default::default()
        }
    }

    fn applying(action: fn(&mut WindowManagementState)) -> WindowManagementState {
        let mut state = supported();
        action(&mut state);
        mark_applying(&mut state);
        state
    }

    #[test]
    fn default_inner_size_is_fixed() {
        assert_eq!(DEFAULT_INNER_SIZE, [480.0, 360.0]);
    }

    #[test]
    fn request_advances_generation_and_is_noop_in_flight() {
        let mut state = supported();
        assert_eq!(take_request(&state), None);
        move_top_left(&mut state);
        assert_eq!(
            take_request(&state),
            Some(WindowRequest {
                generation: 1,
                action: WindowAction::MoveTopLeft
            })
        );
        mark_applying(&mut state);
        assert_eq!(take_request(&state), None);
        reset_size(&mut state);
        always_on_top_on(&mut state);
        assert_eq!(state.generation, 1);
        assert_eq!(state.requested, Some(WindowAction::MoveTopLeft));
    }

    #[test]
    fn finish_ok_shows_observed_values() {
        let mut state = applying(move_top_left);
        let o = WindowObserved {
            position: Some((0.0, 25.0)),
            ..full_observed()
        };
        finish(&mut state, 1, Ok(()), o);
        assert_eq!(state.phase, WindowPhase::Idle);
        assert_eq!(state.error, "");
        assert_eq!(frame_text(&state), "位置: 0, 25 / 大きさ: 480 × 360");
    }

    #[test]
    fn finish_ok_with_unreadable_value_is_failure() {
        for (action, o) in [
            (
                move_top_left as fn(&mut WindowManagementState),
                WindowObserved {
                    position: None,
                    ..full_observed()
                },
            ),
            (
                reset_size,
                WindowObserved {
                    inner_size: None,
                    ..full_observed()
                },
            ),
            (
                always_on_top_on,
                WindowObserved {
                    always_on_top: None,
                    ..full_observed()
                },
            ),
        ] {
            let mut state = applying(action);
            finish(&mut state, 1, Ok(()), o);
            assert_eq!(state.error, format!("{FAILED_PREFIX}{UNREADABLE_REASON}"));
        }
    }

    #[test]
    fn finish_failures_map_to_messages() {
        let mut state = applying(reset_size);
        finish(
            &mut state,
            1,
            Err(WindowFailure::Unsupported),
            full_observed(),
        );
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        let mut state = applying(reset_size);
        finish(
            &mut state,
            1,
            Err(WindowFailure::FullscreenRefused),
            full_observed(),
        );
        assert_eq!(
            state.error,
            format!("{FAILED_PREFIX}{FULLSCREEN_REFUSED_REASON}")
        );
        let mut state = applying(reset_size);
        finish(
            &mut state,
            1,
            Err(WindowFailure::Failed(NOT_APPLIED_REASON.to_string())),
            full_observed(),
        );
        assert_eq!(state.error, format!("{FAILED_PREFIX}{NOT_APPLIED_REASON}"));
        assert_eq!(state.phase, WindowPhase::Idle);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = applying(move_top_left);
        finish(&mut state, 99, Ok(()), full_observed());
        assert_eq!(state.phase, WindowPhase::Applying);
        let mut idle = WindowManagementState::default();
        finish(&mut idle, 0, Ok(()), full_observed());
        assert_eq!(idle.observed, WindowObserved::default());
    }

    #[test]
    fn apply_observed_and_support_do_not_advance_generation() {
        let mut state = applying(move_top_left);
        finish(&mut state, 1, Ok(()), full_observed());
        apply_observed(&mut state, WindowObserved::default());
        assert_eq!(frame_text(&state), FRAME_UNKNOWN_STATUS);
        apply_support(&mut state, WindowSupportView::default());
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn frame_text_never_shows_non_finite() {
        let mut state = supported();
        state.observed.position = Some((f64::NAN, 0.0));
        assert_eq!(frame_text(&state), FRAME_UNKNOWN_STATUS);
    }

    #[test]
    fn buttons_follow_support_and_hide_in_flight() {
        let mut state = supported();
        let screen = current_screen(&state);
        assert!(!click_button(&screen, ALWAYS_ON_TOP_OFF_LABEL, &mut state));
        assert!(click_button(&screen, ALWAYS_ON_TOP_ON_LABEL, &mut state));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, MOVE_TOP_LEFT_LABEL, &mut state));
        mark_applying(&mut state);
        finish(
            &mut state,
            1,
            Ok(()),
            WindowObserved {
                always_on_top: Some(true),
                ..full_observed()
            },
        );
        let screen = current_screen(&state);
        assert!(click_button(&screen, ALWAYS_ON_TOP_OFF_LABEL, &mut state));
    }

    #[test]
    fn unsupported_or_tab_or_unknown_support_shows_no_buttons() {
        for support in [
            WindowSupportView {
                move_resize: Some(false),
                always_on_top: Some(false),
                in_browser_tab: false,
            },
            WindowSupportView {
                move_resize: Some(true),
                always_on_top: Some(false),
                in_browser_tab: true,
            },
            WindowSupportView::default(),
        ] {
            let mut state = WindowManagementState {
                support,
                ..Default::default()
            };
            let screen = current_screen(&state);
            for label in [
                MOVE_TOP_LEFT_LABEL,
                RESET_SIZE_LABEL,
                ALWAYS_ON_TOP_ON_LABEL,
                ALWAYS_ON_TOP_OFF_LABEL,
            ] {
                assert!(!click_button(&screen, label, &mut state), "{label}");
            }
        }
    }

    #[test]
    fn status_explains_unsupported_tab_and_unavailable_top() {
        let mut state = WindowManagementState {
            support: WindowSupportView {
                move_resize: Some(false),
                always_on_top: Some(false),
                in_browser_tab: false,
            },
            ..Default::default()
        };
        assert_eq!(status_text(&state), UNSUPPORTED_STATUS);
        state.support.in_browser_tab = true;
        assert_eq!(status_text(&state), TAB_STATUS);
        assert_eq!(top_status_text(&state), TOP_UNAVAILABLE_STATUS);
    }
}
