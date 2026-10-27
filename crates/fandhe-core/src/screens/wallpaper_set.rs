//! Phase 11（端末機能の拡充ループ、親 #188）型サンプル「壁紙・ロック画面の設定」（Issue #970）。
//! 説明文と「ホーム画面の壁紙にする」「ロック画面の壁紙にする」ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=wallpaper_set` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod wallpaper_set;` と対）。
//! 設定する画像は effect ドライバ（`device/wallpaper.rs`）が自前で生成するため入力欄を持たない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。PNG の検査と OS 呼び出しは effect ドライバが
//! [`take_request`] で要求を観測して行い、[`finish_set`] で結果を注入する。本画面は
//! 二重押し防止と文言だけを持つ。全 OS 分の文言と選択肢は (a) で確定しておき、後続の OS 別 PR
//! （Android、#971）は共通ファイルに触れない。
//!
//! # 結果を混同しない理由
//!
//! 禁止・非対応・失敗を黙って「設定しました」に丸めると、利用者は壁紙が変わったと誤解する。
//! 設定済みの文言は [`Ok(SetOutcome::Set)`] のときだけ出す。OS が判定した正常な結果は
//! `status`、失敗は `error`（`TextRole::Error`）へ書き分ける（`photo_save` と同型）。
//! 世代番号（`generation`）で、古い完了や `Idle` で届いた完了を無視する。

use crate::{Screen, WidgetId};

/// 「ホーム画面」ボタンの `WidgetId`。
const HOME_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「ロック画面」ボタンの `WidgetId`。
const LOCK_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// ホーム画面の壁紙を設定できたときの案内。
pub const SET_HOME_STATUS: &str = "ホーム画面の壁紙を設定しました";
/// ロック画面の壁紙を設定できたときの案内。
pub const SET_LOCK_STATUS: &str = "ロック画面の壁紙を設定しました";
/// 管理者の設定で禁止されているときの案内。
pub const DENIED_STATUS: &str = "管理者の設定で禁止されています";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "設定できませんでした: ";

/// 設定操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_setting`] 済み）。
    Setting,
}

/// 設定先（`fandhe_device::wallpaper::WallpaperTarget` の写し。core は device に依存しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WallpaperChoice {
    /// ホーム画面。
    #[default]
    Home,
    /// ロック画面。
    Lock,
}

/// OS が判定した結果（写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOutcome {
    /// OS が受け付けた。
    Set(WallpaperChoice),
    /// 管理者設定などで禁止。
    Denied,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperSetRequest {
    /// [`finish_set`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 設定先。
    pub choice: WallpaperChoice,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WallpaperSetState {
    /// 進行状態。
    pub phase: SetPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 直近の要求の設定先。
    pub choice: WallpaperChoice,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

fn request(state: &mut WallpaperSetState, choice: WallpaperChoice) {
    if state.phase != SetPhase::Idle {
        return;
    }
    state.phase = SetPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.choice = choice;
    state.status = String::new();
    state.error = String::new();
}

/// 「ホーム画面」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn set_home(state: &mut WallpaperSetState) {
    request(state, WallpaperChoice::Home);
}

/// 「ロック画面」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn set_lock(state: &mut WallpaperSetState) {
    request(state, WallpaperChoice::Lock);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &WallpaperSetState) -> Option<WallpaperSetRequest> {
    (state.phase == SetPhase::Requested).then_some(WallpaperSetRequest {
        generation: state.generation,
        choice: state.choice,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_setting(state: &mut WallpaperSetState) {
    if state.phase == SetPhase::Requested {
        state.phase = SetPhase::Setting;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_set(
    state: &mut WallpaperSetState,
    generation: u64,
    result: Result<SetOutcome, String>,
) {
    if state.phase == SetPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = SetPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(SetOutcome::Set(WallpaperChoice::Home)) => state.status = SET_HOME_STATUS.to_string(),
        Ok(SetOutcome::Set(WallpaperChoice::Lock)) => state.status = SET_LOCK_STATUS.to_string(),
        Ok(SetOutcome::Denied) => state.status = DENIED_STATUS.to_string(),
        Ok(SetOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &WallpaperSetState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &WallpaperSetState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外して二重押しを防ぐ。
pub fn current_screen(state: &WallpaperSetState) -> Screen<WallpaperSetState> {
    let screen = Screen::new("壁紙・ロック画面の設定")
        .text("アプリが作った画像を 1 枚、端末の壁紙に設定します")
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != SetPhase::Idle {
        return screen;
    }
    screen
        .button(HOME_BUTTON_ID, "見本画像をホーム画面の壁紙にする", set_home)
        .button(LOCK_BUTTON_ID, "見本画像をロック画面の壁紙にする", set_lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(choice: WallpaperChoice) -> WallpaperSetState {
        let mut s = WallpaperSetState::default();
        match choice {
            WallpaperChoice::Home => set_home(&mut s),
            WallpaperChoice::Lock => set_lock(&mut s),
        }
        mark_setting(&mut s);
        s
    }

    fn shown(r: Result<SetOutcome, String>) -> (String, String) {
        let mut s = setting(WallpaperChoice::Home);
        let g = s.generation;
        finish_set(&mut s, g, r);
        assert_eq!(s.phase, SetPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn results_are_distinct_and_only_set_says_set() {
        let all = [
            shown(Ok(SetOutcome::Set(WallpaperChoice::Home))),
            shown(Ok(SetOutcome::Set(WallpaperChoice::Lock))),
            shown(Ok(SetOutcome::Denied)),
            shown(Ok(SetOutcome::Unsupported)),
            shown(Err("x".into())),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let said_set = format!("{}{}", a.0, a.1).contains("設定しました");
            assert_eq!(said_set, i < 2, "index {i}");
        }
        assert!(!FAILED_PREFIX.contains("設定しました"));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = setting(WallpaperChoice::Home);
        let g = s.generation + 1;
        finish_set(&mut s, g, Ok(SetOutcome::Set(WallpaperChoice::Home)));
        assert_eq!(s.phase, SetPhase::Setting);
        assert!(s.status.is_empty());

        let mut idle = WallpaperSetState::default();
        finish_set(&mut idle, 0, Ok(SetOutcome::Set(WallpaperChoice::Home)));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = setting(WallpaperChoice::Home);
        let g = s.generation;
        set_lock(&mut s);
        assert_eq!(
            (s.phase, s.generation, s.choice),
            (SetPhase::Setting, g, WallpaperChoice::Home)
        );
    }

    #[test]
    fn choice_is_carried_into_request_and_clears_previous() {
        let mut s = WallpaperSetState {
            status: SET_HOME_STATUS.into(),
            error: "e".into(),
            ..Default::default()
        };
        set_lock(&mut s);
        assert!(s.status.is_empty() && s.error.is_empty());
        let r = take_request(&s).expect("requested");
        assert_eq!(
            (r.generation, r.choice),
            (s.generation, WallpaperChoice::Lock)
        );
        mark_setting(&mut s);
        assert!(take_request(&s).is_none());
    }
}
