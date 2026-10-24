//! Phase 11（端末機能の拡充ループ、親 #150）型サンプル「グローバルホットキー」（Issue #766）。
//! 固定のキー組み合わせ 1 つを「登録」ボタンで登録し、押されるたびに画面の回数を 1 増やす。
//! 「解除」ボタンとアプリ終了で登録を外す。`FANDHE_PROXY_APP=global_hotkey` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod global_hotkey;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。登録・解除・押下の受け取りは `fandhe-egui-proxy` の effect ドライバ
//! （`device/global_hotkey.rs`）が [`take_action`] で要求を観測して行い、[`finish_register`] /
//! [`finish_unregister`] / [`apply_presses`] で結果を注入する。本画面は二重押し防止・世代の照合・
//! 文言と、OS 別のキー表記（[`Platform`]、ドライバが一度だけ注入）だけを持つ。
//!
//! # 状態機械
//!
//! ```text
//! Unregistered --登録--> RegisterRequested --mark--> Registering --finish_register-->
//!     Ok(Registered)           -> Registered
//!     Ok(InUse)/Unsupported/Err -> Unregistered（理由の文言つき）
//! Registered --解除--> UnregisterRequested --mark--> Unregistering --finish_unregister-->
//!     Ok(*)  -> Unregistered
//!     Err    -> Registered のまま（再試行できる）
//! ```
//!
//! 表示は登録・解除の戻り値からだけ作る。押下は `phase == Registered` かつ登録時の世代と一致する
//! ときだけ数え、処理中・解除後・古い世代の押下は捨てる。

use crate::{Screen, WidgetId};

/// macOS のキー表記。
pub const MACOS_KEYS: &str = "Control + Shift + Option + K";
/// Windows のキー表記。
pub const WINDOWS_KEYS: &str = "Ctrl + Shift + Alt + K";

/// 未登録の表示。
pub const UNREGISTERED_STATUS: &str = "未登録";
/// 登録中の表示。
pub const REGISTERED_STATUS: &str = "登録中";
/// 処理中の表示。
pub const BUSY_STATUS: &str = "処理中…";
/// 使用中（OS が重複で拒否）の案内。
pub const IN_USE_ERROR: &str = "ほかのアプリが使っています";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では使えません";
/// 登録の失敗。
pub const REGISTER_FAILED_ERROR: &str = "登録できませんでした";
/// 解除の失敗（登録は残っている）。
pub const UNREGISTER_FAILED_ERROR: &str = "解除できませんでした（登録中のままです）";

const REGISTER_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const UNREGISTER_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 文言を出し分ける OS（ドライバが一度だけ注入する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Platform {
    /// 未注入・その他。
    #[default]
    Unknown,
    /// macOS。
    MacOs,
    /// Windows。
    Windows,
}

/// 進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HotkeyPhase {
    /// 未登録。
    #[default]
    Unregistered,
    /// 登録ボタンが押され、ドライバへまだ渡していない。
    RegisterRequested,
    /// ドライバが登録を処理中。
    Registering,
    /// 登録中。
    Registered,
    /// 解除ボタンが押され、ドライバへまだ渡していない。
    UnregisterRequested,
    /// ドライバが解除を処理中。
    Unregistering,
}

/// OS が判定した登録の結果（非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterResult {
    /// 登録できた。
    Registered,
    /// 使用中。
    InUse,
    /// この環境では非対応。
    Unsupported,
}

/// OS が判定した解除の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnregisterResult {
    /// 解除できた。
    Unregistered,
    /// もともと登録していなかった。
    NotRegistered,
}

/// 失敗（画面には固定文言だけを出し、OS のコードは持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyFailure;

/// ドライバが行う操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// 登録する。
    Register,
    /// 解除する。
    Unregister,
}

/// ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyRequestView {
    /// 完了・押下へそのまま渡す世代番号。
    pub generation: u64,
    /// 操作。
    pub action: HotkeyAction,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlobalHotkeyState {
    /// 進行状態。
    pub phase: HotkeyPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 登録が成功したときの世代（押下の照合に使う）。
    pub registered_generation: u64,
    /// 押された回数。
    pub count: u64,
    /// 注入済みの OS。
    pub platform: Platform,
    /// 非対応などの案内（通常色）。
    pub notice: String,
    /// 失敗・使用中（`TextRole::Error`）。
    pub error: String,
}

/// この OS を反映する（ドライバが一度だけ送る）。
pub fn apply_platform(state: &mut GlobalHotkeyState, platform: Platform) {
    state.platform = platform;
}

/// 「登録」ボタンのハンドラ。`Unregistered` 以外は二重要求防止のため no-op。
pub fn request_register(state: &mut GlobalHotkeyState) {
    if state.phase != HotkeyPhase::Unregistered {
        return;
    }
    state.phase = HotkeyPhase::RegisterRequested;
    state.generation = state.generation.wrapping_add(1);
    state.notice = String::new();
    state.error = String::new();
}

/// 「解除」ボタンのハンドラ。`Registered` 以外は no-op。
pub fn request_unregister(state: &mut GlobalHotkeyState) {
    if state.phase != HotkeyPhase::Registered {
        return;
    }
    state.phase = HotkeyPhase::UnregisterRequested;
    state.generation = state.generation.wrapping_add(1);
    state.notice = String::new();
    state.error = String::new();
}

/// ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_action(state: &GlobalHotkeyState) -> Option<HotkeyRequestView> {
    let action = match state.phase {
        HotkeyPhase::RegisterRequested => HotkeyAction::Register,
        HotkeyPhase::UnregisterRequested => HotkeyAction::Unregister,
        _ => return None,
    };
    Some(HotkeyRequestView {
        generation: state.generation,
        action,
    })
}

/// 要求を処理中へ進める。要求前のフェーズでは何もしない。
pub fn mark_in_flight(state: &mut GlobalHotkeyState) {
    state.phase = match state.phase {
        HotkeyPhase::RegisterRequested => HotkeyPhase::Registering,
        HotkeyPhase::UnregisterRequested => HotkeyPhase::Unregistering,
        other => other,
    };
}

/// 登録の結果を反映する。処理中でない・世代が違う完了は無視する。
pub fn finish_register(
    state: &mut GlobalHotkeyState,
    generation: u64,
    result: Result<RegisterResult, HotkeyFailure>,
) {
    if state.phase != HotkeyPhase::Registering || state.generation != generation {
        return;
    }
    state.notice = String::new();
    state.error = String::new();
    state.phase = HotkeyPhase::Unregistered;
    match result {
        Ok(RegisterResult::Registered) => {
            state.phase = HotkeyPhase::Registered;
            state.registered_generation = generation;
        }
        Ok(RegisterResult::InUse) => state.error = IN_USE_ERROR.to_string(),
        Ok(RegisterResult::Unsupported) => state.notice = UNSUPPORTED_STATUS.to_string(),
        Err(HotkeyFailure) => state.error = REGISTER_FAILED_ERROR.to_string(),
    }
}

/// 解除の結果を反映する。処理中でない・世代が違う完了は無視する。失敗なら登録中のまま残す。
pub fn finish_unregister(
    state: &mut GlobalHotkeyState,
    generation: u64,
    result: Result<UnregisterResult, HotkeyFailure>,
) {
    if state.phase != HotkeyPhase::Unregistering || state.generation != generation {
        return;
    }
    state.notice = String::new();
    state.error = String::new();
    match result {
        Ok(UnregisterResult::Unregistered | UnregisterResult::NotRegistered) => {
            state.phase = HotkeyPhase::Unregistered;
        }
        Err(HotkeyFailure) => {
            state.phase = HotkeyPhase::Registered;
            state.error = UNREGISTER_FAILED_ERROR.to_string();
        }
    }
}

/// 押下 `n` 回を反映する。登録中かつ登録時の世代と一致するときだけ数える（処理中・解除後・
/// 古い世代の押下は捨てる）。
pub fn apply_presses(state: &mut GlobalHotkeyState, generation: u64, n: u64) {
    if state.phase == HotkeyPhase::Registered && state.registered_generation == generation {
        state.count = state.count.saturating_add(n);
    }
}

/// 押下 1 回を反映する。
pub fn apply_press(state: &mut GlobalHotkeyState, generation: u64) {
    apply_presses(state, generation, 1);
}

/// キー表記（OS ごと）。
pub fn keys_label(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => MACOS_KEYS,
        Platform::Windows => WINDOWS_KEYS,
        Platform::Unknown => "固定の組み合わせ",
    }
}

fn status_label(phase: HotkeyPhase) -> &'static str {
    match phase {
        HotkeyPhase::Unregistered => UNREGISTERED_STATUS,
        HotkeyPhase::Registered => REGISTERED_STATUS,
        _ => BUSY_STATUS,
    }
}

/// 登録状態の文言（`DynamicText` の `get`）。
pub fn state_text(state: &GlobalHotkeyState) -> String {
    status_label(state.phase).to_string()
}

/// 回数の文言。
pub fn count_text(state: &GlobalHotkeyState) -> String {
    format!("押された回数: {}", state.count)
}

/// 案内文言。
pub fn notice_text(state: &GlobalHotkeyState) -> String {
    state.notice.clone()
}

/// エラー文言。
pub fn error_text(state: &GlobalHotkeyState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外し、登録中は「解除」、未登録は「登録」だけを出す。
pub fn current_screen(state: &GlobalHotkeyState) -> Screen<GlobalHotkeyState> {
    let screen = Screen::new("グローバルホットキー")
        .text(format!("キー: {}", keys_label(state.platform)))
        .dynamic_text(state_text)
        .dynamic_text(count_text)
        .dynamic_text(notice_text)
        .error_text(error_text);
    match state.phase {
        HotkeyPhase::Unregistered => screen.button(REGISTER_BUTTON_ID, "登録", request_register),
        HotkeyPhase::Registered => screen.button(UNREGISTER_BUTTON_ID, "解除", request_unregister),
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registering() -> GlobalHotkeyState {
        let mut s = GlobalHotkeyState::default();
        request_register(&mut s);
        mark_in_flight(&mut s);
        s
    }

    fn registered() -> GlobalHotkeyState {
        let mut s = registering();
        let g = s.generation;
        finish_register(&mut s, g, Ok(RegisterResult::Registered));
        s
    }

    #[test]
    fn double_press_makes_one_request() {
        let mut s = GlobalHotkeyState::default();
        request_register(&mut s);
        let first = take_action(&s).unwrap();
        request_register(&mut s);
        assert_eq!(take_action(&s), Some(first));
        assert_eq!(s.generation, 1);
        mark_in_flight(&mut s);
        assert!(take_action(&s).is_none());
        request_register(&mut s);
        assert_eq!(s.phase, HotkeyPhase::Registering);
    }

    #[test]
    fn stale_and_unrequested_completions_are_ignored() {
        let mut s = registering();
        let g = s.generation + 1;
        finish_register(&mut s, g, Ok(RegisterResult::Registered));
        assert_eq!(s.phase, HotkeyPhase::Registering);

        let mut idle = GlobalHotkeyState::default();
        finish_register(&mut idle, 0, Ok(RegisterResult::Registered));
        assert_eq!(idle.phase, HotkeyPhase::Unregistered);
        finish_unregister(&mut idle, 0, Ok(UnregisterResult::Unregistered));
        assert_eq!(idle.phase, HotkeyPhase::Unregistered);

        let mut r = registered();
        let g = r.generation;
        finish_register(&mut r, g, Err(HotkeyFailure));
        assert_eq!(r.phase, HotkeyPhase::Registered);
    }

    #[test]
    fn in_use_and_unsupported_and_failure_never_become_registered() {
        for (result, error, notice) in [
            (Ok(RegisterResult::InUse), IN_USE_ERROR, ""),
            (Ok(RegisterResult::Unsupported), "", UNSUPPORTED_STATUS),
            (Err(HotkeyFailure), REGISTER_FAILED_ERROR, ""),
        ] {
            let mut s = registering();
            let g = s.generation;
            finish_register(&mut s, g, result);
            assert_eq!(s.phase, HotkeyPhase::Unregistered);
            assert_eq!(s.error, error);
            assert_eq!(s.notice, notice);
            apply_press(&mut s, g);
            assert_eq!(s.count, 0);
        }
    }

    #[test]
    fn unregister_failure_keeps_registered_and_allows_retry() {
        let mut s = registered();
        request_unregister(&mut s);
        mark_in_flight(&mut s);
        let g = s.generation;
        finish_unregister(&mut s, g, Err(HotkeyFailure));
        assert_eq!(s.phase, HotkeyPhase::Registered);
        assert_eq!(s.error, UNREGISTER_FAILED_ERROR);
        request_unregister(&mut s);
        assert_eq!(take_action(&s).unwrap().action, HotkeyAction::Unregister);
    }

    #[test]
    fn presses_count_only_while_registered_with_matching_generation() {
        let mut s = GlobalHotkeyState::default();
        apply_press(&mut s, 0);
        assert_eq!(s.count, 0);

        let mut s = registering();
        let g = s.generation;
        apply_press(&mut s, g);
        assert_eq!(s.count, 0);
        finish_register(&mut s, g, Ok(RegisterResult::Registered));
        apply_press(&mut s, g);
        apply_presses(&mut s, g, 3);
        assert_eq!(s.count, 4);
        apply_press(&mut s, g + 1);
        assert_eq!(s.count, 4);

        request_unregister(&mut s);
        mark_in_flight(&mut s);
        apply_press(&mut s, g);
        assert_eq!(s.count, 4);
        let ug = s.generation;
        finish_unregister(&mut s, ug, Ok(UnregisterResult::Unregistered));
        apply_press(&mut s, g);
        assert_eq!(s.count, 4);

        request_register(&mut s);
        mark_in_flight(&mut s);
        let g2 = s.generation;
        finish_register(&mut s, g2, Ok(RegisterResult::Registered));
        apply_press(&mut s, g);
        assert_eq!(s.count, 4);
        apply_press(&mut s, g2);
        assert_eq!(s.count, 5);
    }

    #[test]
    fn count_saturates() {
        let mut s = registered();
        s.count = u64::MAX;
        let g = s.registered_generation;
        apply_press(&mut s, g);
        assert_eq!(s.count, u64::MAX);
    }

    #[test]
    fn buttons_follow_phase() {
        let idle = format!("{:?}", current_screen(&GlobalHotkeyState::default()));
        assert!(idle.contains("登録") && !idle.contains("解除"));
        let on = format!("{:?}", current_screen(&registered()));
        assert!(on.contains("解除"));
        let busy = format!("{:?}", current_screen(&registering()));
        assert!(!busy.contains("\"登録\"") && !busy.contains("解除"));
    }

    #[test]
    fn keys_follow_platform() {
        let mut s = GlobalHotkeyState::default();
        apply_platform(&mut s, Platform::MacOs);
        let mac = format!("{:?}", current_screen(&s));
        assert!(mac.contains(MACOS_KEYS) && !mac.contains("Ctrl"));
        apply_platform(&mut s, Platform::Windows);
        let win = format!("{:?}", current_screen(&s));
        assert!(win.contains(WINDOWS_KEYS) && !win.contains("Option"));
    }
}
