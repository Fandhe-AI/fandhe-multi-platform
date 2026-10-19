//! Phase 11 の拡充ループ型サンプル「集中モードの状態」（親 #104、Issue #438）。
//! `FANDHE_PROXY_APP=focus_mode_status_show` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod focus_mode_status_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `fandhe-egui-proxy` の `device/focus_mode_status.rs` が一定間隔で行い、[`apply_read`] で
//! 結果を注入する。
//!
//! 取得中・非対応・失敗では「オフ」を出さない（嘘の成功の禁止）。失敗時は最後に読めた値を残さない。

use crate::Screen;

/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 読めなかった理由。`DeviceError` からの写像は呼び出し元（proxy）の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出す集中モードの状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FocusView {
    /// 最初の値が届く前。
    #[default]
    Reading,
    /// 集中モードがオン。
    On,
    /// 集中モードがオフ。
    Off,
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「集中モードの状態」画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FocusModeStatusShowState {
    /// 現在の表示状態。
    pub view: FocusView,
}

/// 読み取り結果を反映する。毎回 view を置き換えるため、失敗後に前の値は残らない。
pub fn apply_read(state: &mut FocusModeStatusShowState, result: Result<bool, ReadFailure>) {
    state.view = match result {
        Ok(true) => FocusView::On,
        Ok(false) => FocusView::Off,
        Err(ReadFailure::Unsupported) => FocusView::Unsupported,
        Err(ReadFailure::Failed(m)) => FocusView::Failed(m),
    };
}

/// 状態の文言（ヘッドレス検証と a11y の両方を担う）。
pub fn status_text(state: &FocusModeStatusShowState) -> String {
    match &state.view {
        FocusView::Reading => "集中モードの状態を読み取っています".to_string(),
        FocusView::Unsupported => "この環境では集中モードの状態を取得できません".to_string(),
        FocusView::Failed(_) => "集中モード: 不明".to_string(),
        FocusView::On => "集中モード: オン".to_string(),
        FocusView::Off => "集中モード: オフ".to_string(),
    }
}

/// 失敗時のみ非空。
pub fn error_text(state: &FocusModeStatusShowState) -> String {
    match &state.view {
        FocusView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 注記。Android は「おやすみモード」（割り込みフィルタ）、Windows は集中セッションを見る。
pub const NOTE: &str = "Android ではおやすみモード、Windows では集中セッションの状態を表示します。変更は最大 1 秒ほどで反映されます。";

/// 「集中モードの状態」画面。
pub fn focus_mode_status_show_screen() -> Screen<FocusModeStatusShowState> {
    Screen::new("集中モードの状態")
        .dynamic_text(status_text)
        .error_text(error_text)
        .text(NOTE)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &FocusModeStatusShowState) -> Screen<FocusModeStatusShowState> {
    focus_mode_status_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn state(view: FocusView) -> FocusModeStatusShowState {
        FocusModeStatusShowState { view }
    }

    #[test]
    fn initial_is_reading() {
        assert_eq!(FocusModeStatusShowState::default().view, FocusView::Reading);
    }

    #[test]
    fn on_and_off_texts() {
        assert_eq!(status_text(&state(FocusView::On)), "集中モード: オン");
        assert_eq!(status_text(&state(FocusView::Off)), "集中モード: オフ");
    }

    #[test]
    fn non_read_states_never_show_off() {
        for view in [
            FocusView::Reading,
            FocusView::Unsupported,
            FocusView::Failed("集中モードの取得".to_string()),
        ] {
            let s = state(view);
            assert!(!status_text(&s).contains("オフ"), "{:?}", s.view);
            assert!(!status_text(&s).contains("オン"), "{:?}", s.view);
        }
    }

    #[test]
    fn failure_drops_previous_value() {
        for r in [
            Err(ReadFailure::Unsupported),
            Err(ReadFailure::Failed("集中モードの取得".to_string())),
        ] {
            let mut s = FocusModeStatusShowState::default();
            apply_read(&mut s, Ok(true));
            apply_read(&mut s, r);
            assert!(!matches!(s.view, FocusView::On | FocusView::Off));
        }
    }

    #[test]
    fn error_text_only_when_failed() {
        let mut s = FocusModeStatusShowState::default();
        assert_eq!(error_text(&s), "");
        apply_read(
            &mut s,
            Err(ReadFailure::Failed("集中モードの取得".to_string())),
        );
        assert_eq!(error_text(&s), "失敗しました: 集中モードの取得");
        apply_read(&mut s, Ok(false));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn render_headless_shows_reading() {
        let s = FocusModeStatusShowState::default();
        let joined = render_headless(&focus_mode_status_show_screen(), &s).join("\n");
        assert!(joined.contains("読み取っています"));
    }
}
