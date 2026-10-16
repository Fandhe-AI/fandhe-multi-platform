//! Phase 11（端末機能の拡充ループ、親 #1141）型サンプル「バッジの設定」（Issue #1360）。
//! 数値入力欄と「設定する」「消す（0）」ボタンだけの 1 画面。`FANDHE_PROXY_APP=app_badge_set` で
//! 選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod app_badge_set;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。値の範囲検証と OS 呼び出しは effect ドライバ
//! （`device/app_badge.rs`）が [`take_request`] で要求を観測して行い、[`finish_set`] で結果を注入する。
//! 全 OS 分の文言は (a) で確定し、後続の OS 別 PR は共通ファイルに触れない。
//!
//! # 結果を混同しない理由
//!
//! 権限拒否・非対応・キャンセルを「OS へ渡しました」に丸めると、利用者はバッジが出たと誤解する。
//! 成功文言は `Submitted` / `Cleared` のときだけ出し、それも「OS へ渡した」までで表示は保証しない。
//! 世代番号（`generation`）で、古い完了や `Idle` で届いた完了を無視する。

use crate::{Screen, WidgetId};

/// 「設定する」ボタンの `WidgetId`。
const SET_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「消す（0）」ボタンの `WidgetId`。
const CLEAR_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 値を OS へ渡せたときの案内（表示は OS・ランチャー次第で保証しない）。
pub const SUBMITTED_STATUS: &str = "OS へ渡しました（表示されるかは OS・ランチャー次第です）";
/// 0 を OS へ渡せたときの案内。
pub const CLEARED_STATUS: &str = "バッジの消去を OS へ依頼しました";
/// 許可の確認が閉じられたときの案内。
pub const CANCELLED_STATUS: &str = "キャンセルしました（バッジは変わっていません）";
/// 権限が無いときの案内。
pub const DENIED_STATUS: &str = "通知（バッジ）が許可されていません。設定アプリで変更できます";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境ではバッジを設定できません";
/// 数値でない入力の案内。
pub const INVALID_NUMBER_STATUS: &str = "0 以上の整数を入力してください";
/// 範囲外（件数）の失敗文言。
pub const OUT_OF_RANGE_MESSAGE: &str = "件数は 0〜9999 で入力してください";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "設定できませんでした: ";
/// 固定の注記。
pub const NOTE_TEXT: &str = "件数は 0〜9999、0 で消します。Windows はタスクバーボタンの右下に数字を重ねます。\
Android は無音の通知が 1 件増え、ドットと長押しメニューに件数が出ます（アイコン上の数字は保証しません）。\
Web は PWA としてインストールしたときだけ表示されます。iOS は通知（バッジ）の許可が要ります";

/// 設定操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgePhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_setting`] 済み）。
    Setting,
}

/// OS が判定した結果（写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 0 以外を OS へ渡した。
    Submitted,
    /// 0（消去）を OS へ渡した。
    Cleared,
    /// 許可の確認が閉じられた（iOS）。
    Cancelled,
    /// 権限拒否。
    Denied,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadgeRequest {
    /// [`finish_set`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 設定する値（範囲はドライバが検証する）。
    pub value: u32,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppBadgeSetState {
    /// 入力欄の文字列。
    pub input: String,
    /// 進行状態。
    pub phase: BadgePhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 直近の要求の値。
    pub value: u32,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

fn request(state: &mut AppBadgeSetState, value: u32) {
    state.phase = BadgePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.value = value;
    state.status = String::new();
    state.error = String::new();
}

/// 「設定する」ボタンのハンドラ。`Idle` 以外は no-op。数値でなければ要求を立てない。
pub fn set(state: &mut AppBadgeSetState) {
    if state.phase != BadgePhase::Idle {
        return;
    }
    match state.input.trim().parse::<u32>() {
        Ok(v) => request(state, v),
        Err(_) => {
            state.status = String::new();
            state.error = INVALID_NUMBER_STATUS.to_string();
        }
    }
}

/// 「消す（0）」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn clear(state: &mut AppBadgeSetState) {
    if state.phase != BadgePhase::Idle {
        return;
    }
    request(state, 0);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &AppBadgeSetState) -> Option<BadgeRequest> {
    (state.phase == BadgePhase::Requested).then_some(BadgeRequest {
        generation: state.generation,
        value: state.value,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_setting(state: &mut AppBadgeSetState) {
    if state.phase == BadgePhase::Requested {
        state.phase = BadgePhase::Setting;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_set(
    state: &mut AppBadgeSetState,
    generation: u64,
    result: Result<CoreOutcome, String>,
) {
    if state.phase == BadgePhase::Idle || state.generation != generation {
        return;
    }
    state.phase = BadgePhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::Submitted) => state.status = SUBMITTED_STATUS.to_string(),
        Ok(CoreOutcome::Cleared) => state.status = CLEARED_STATUS.to_string(),
        Ok(CoreOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CoreOutcome::Denied) => state.error = DENIED_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &AppBadgeSetState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &AppBadgeSetState) -> String {
    state.error.clone()
}

fn input_get(state: &AppBadgeSetState) -> String {
    state.input.clone()
}

fn input_set(state: &mut AppBadgeSetState, v: String) {
    state.input = v;
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &AppBadgeSetState) -> Screen<AppBadgeSetState> {
    let screen = Screen::new("バッジの設定")
        .text(NOTE_TEXT)
        .text_field("件数（0〜9999）", input_get, input_set)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != BadgePhase::Idle {
        return screen;
    }
    screen
        .button(SET_BUTTON_ID, "設定する", set)
        .button(CLEAR_BUTTON_ID, "消す（0）", clear)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(input: &str) -> AppBadgeSetState {
        let mut s = AppBadgeSetState {
            input: input.into(),
            ..Default::default()
        };
        set(&mut s);
        mark_setting(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, String>) -> (String, String) {
        let mut s = setting("3");
        let g = s.generation;
        finish_set(&mut s, g, r);
        assert_eq!(s.phase, BadgePhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn request_advances_generation_and_phase() {
        let mut s = AppBadgeSetState {
            input: " 12 ".into(),
            ..Default::default()
        };
        set(&mut s);
        assert_eq!(
            (s.phase, s.generation, s.value),
            (BadgePhase::Requested, 1, 12)
        );
        let r = take_request(&s).unwrap();
        assert_eq!((r.generation, r.value), (1, 12));
        mark_setting(&mut s);
        assert!(take_request(&s).is_none());
    }

    #[test]
    fn non_numeric_input_does_not_request() {
        for bad in ["", "abc", "-1", "1.5"] {
            let mut s = AppBadgeSetState {
                input: bad.into(),
                ..Default::default()
            };
            set(&mut s);
            assert_eq!(s.phase, BadgePhase::Idle, "{bad}");
            assert_eq!(s.error, INVALID_NUMBER_STATUS);
        }
    }

    #[test]
    fn clear_requests_zero_and_is_noop_in_flight() {
        let mut s = AppBadgeSetState::default();
        clear(&mut s);
        assert_eq!((s.phase, s.value), (BadgePhase::Requested, 0));
        mark_setting(&mut s);
        let g = s.generation;
        clear(&mut s);
        set(&mut s);
        assert_eq!((s.phase, s.generation), (BadgePhase::Setting, g));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = setting("3");
        let g = s.generation + 1;
        finish_set(&mut s, g, Ok(CoreOutcome::Submitted));
        assert_eq!(s.phase, BadgePhase::Setting);
        assert!(s.status.is_empty());

        let mut idle = AppBadgeSetState::default();
        finish_set(&mut idle, 0, Ok(CoreOutcome::Submitted));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn only_submitted_and_cleared_say_handed_over() {
        let all = [
            shown(Ok(CoreOutcome::Submitted)),
            shown(Ok(CoreOutcome::Cleared)),
            shown(Ok(CoreOutcome::Cancelled)),
            shown(Ok(CoreOutcome::Denied)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err("x".into())),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // 成功は status 側のみ、拒否・非対応・失敗は error 側のみ。
        assert!(!all[0].0.is_empty() && all[0].1.is_empty());
        assert!(!all[1].0.is_empty() && all[1].1.is_empty());
        assert!(!all[2].0.contains("渡しました") && !all[2].0.contains("依頼"));
        for a in &all[3..] {
            assert!(a.0.is_empty() && !a.1.is_empty());
            assert!(!a.1.contains("渡しました"));
        }
    }
}
