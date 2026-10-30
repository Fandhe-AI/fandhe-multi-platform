//! Phase 11（端末機能の拡充ループ、親 #137）型サンプル「アプリの言語」（Issue #1073）。
//! 「言語を読む」ボタン → OS から「アプリが使う言語のタグ」と「設定の出どころ」を 1 回読んで表示し、
//! 設定の場所を文章で案内する 1 画面。`FANDHE_PROXY_APP=per_app_language_show` で選ばれ、
//! `fandhe-egui-proxy` の `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは
//! `device/per_app_language.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`device_identifier_show.rs` と同型。読み取りは全 OS で同期のため読み取り中の相は置かない）。
//!
//! 全対象 OS の文言・状態をここで確定済みで、後続の OS 実装（#1074・#1075）は本画面を触らない。

use crate::{Screen, WidgetId};

const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 読み取りボタンのラベル。
pub const READ_LABEL: &str = "言語を読む";
/// 読み取り完了の案内。
pub const DONE_STATUS: &str = "読み取りました";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境ではアプリの言語を取得できません";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 言語の行ラベル。
pub const TAG_LABEL: &str = "アプリの言語";
/// 出どころの行ラベル。
pub const ORIGIN_LABEL: &str = "設定の出どころ";
/// 出どころの文言: アプリ別。
pub const ORIGIN_PER_APP_TEXT: &str = "アプリ別";
/// 出どころの文言: OS 全体。
pub const ORIGIN_SYSTEM_TEXT: &str = "OS 全体";
/// 出どころの文言: 区別できない。
pub const ORIGIN_UNKNOWN_TEXT: &str = "不明";
/// 設定の場所の案内（全 OS 分の固定文。OS 分岐は持たない）。
pub const HINT: &str = "設定の場所: iOS は設定 → このアプリ → 言語、Android は設定 → システム → 言語 → アプリの言語、macOS はシステム設定 → 一般 → 言語と地域 → アプリケーション。変更の反映に再起動が要る場合があります";

/// 設定の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageOrigin {
    /// アプリ別。
    PerApp,
    /// OS 全体。
    System,
    /// 区別できない（Apple）。
    Unknown,
}

/// 読み取った言語。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageReading {
    /// 言語タグ。
    pub tag: String,
    /// 設定の出どころ。
    pub origin: LanguageOrigin,
}

/// 失敗の種別。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// 非対応。
    Unsupported,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadPhase {
    /// 未操作・完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PerAppLanguageShowState {
    /// 進行状態。
    pub phase: ReadPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 読み取れた言語。
    pub reading: Option<LanguageReading>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。
    pub error: String,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    /// [`finish_read`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// ボタンのハンドラ。`Idle` 以外では no-op。前回の結果は消す。
pub fn read(state: &mut PerAppLanguageShowState) {
    if state.phase != ReadPhase::Idle {
        return;
    }
    state.phase = ReadPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.reading = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &PerAppLanguageShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 結果を反映する。世代が一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut PerAppLanguageShowState,
    generation: u64,
    result: Result<LanguageReading, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.reading = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(r) => {
            state.reading = Some(r);
            state.status = DONE_STATUS.to_string();
        }
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 出どころの文言。
pub fn origin_label(origin: LanguageOrigin) -> &'static str {
    match origin {
        LanguageOrigin::PerApp => ORIGIN_PER_APP_TEXT,
        LanguageOrigin::System => ORIGIN_SYSTEM_TEXT,
        LanguageOrigin::Unknown => ORIGIN_UNKNOWN_TEXT,
    }
}

/// 言語の行。未読み取りのときは空。
pub fn tag_text(state: &PerAppLanguageShowState) -> String {
    state
        .reading
        .as_ref()
        .map(|r| format!("{TAG_LABEL}: {}", r.tag))
        .unwrap_or_default()
}

/// 出どころの行。未読み取りのときは空。
pub fn origin_text(state: &PerAppLanguageShowState) -> String {
    state
        .reading
        .as_ref()
        .map(|r| format!("{ORIGIN_LABEL}: {}", origin_label(r.origin)))
        .unwrap_or_default()
}

/// 案内文言。
pub fn status_text(state: &PerAppLanguageShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言。
pub fn error_text(state: &PerAppLanguageShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &PerAppLanguageShowState) -> Screen<PerAppLanguageShowState> {
    let screen = Screen::new("アプリの言語")
        .dynamic_text(status_text)
        .dynamic_text(tag_text)
        .dynamic_text(origin_text)
        .error_text(error_text)
        .text(HINT);
    if state.phase != ReadPhase::Idle {
        return screen;
    }
    screen.button(READ_BUTTON_ID, READ_LABEL, read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn requested() -> PerAppLanguageShowState {
        let mut s = PerAppLanguageShowState::default();
        read(&mut s);
        s
    }

    fn reading(origin: LanguageOrigin) -> LanguageReading {
        LanguageReading {
            tag: "ja".into(),
            origin,
        }
    }

    #[test]
    fn read_advances_generation_and_request() {
        let s = requested();
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), Some(ReadRequest { generation: 1 }));
    }

    #[test]
    fn double_press_is_noop() {
        let mut s = requested();
        read(&mut s);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn stale_and_idle_completions_ignored() {
        let mut s = requested();
        finish_read(&mut s, 99, Ok(reading(LanguageOrigin::Unknown)));
        assert_eq!(s.phase, ReadPhase::Requested);
        assert_eq!(s.reading, None);
        let mut idle = PerAppLanguageShowState::default();
        finish_read(&mut idle, 0, Ok(reading(LanguageOrigin::Unknown)));
        assert_eq!(idle.reading, None);
    }

    #[test]
    fn success_shows_two_rows_and_done() {
        for (o, t) in [
            (LanguageOrigin::PerApp, ORIGIN_PER_APP_TEXT),
            (LanguageOrigin::System, ORIGIN_SYSTEM_TEXT),
            (LanguageOrigin::Unknown, ORIGIN_UNKNOWN_TEXT),
        ] {
            let mut s = requested();
            finish_read(&mut s, 1, Ok(reading(o)));
            assert_eq!(s.status, DONE_STATUS);
            assert_eq!(tag_text(&s), format!("{TAG_LABEL}: ja"));
            assert_eq!(origin_text(&s), format!("{ORIGIN_LABEL}: {t}"));
        }
    }

    #[test]
    fn unsupported_and_failed_are_errors() {
        let mut s = requested();
        finish_read(&mut s, 1, Err(ReadFailure::Unsupported));
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        assert_eq!(s.reading, None);
        assert_eq!(s.status, "");
        read(&mut s);
        finish_read(&mut s, 2, Err(ReadFailure::Failed("x".into())));
        assert_eq!(s.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(s.reading, None);
    }

    #[test]
    fn rerun_clears_previous() {
        let mut s = requested();
        finish_read(&mut s, 1, Ok(reading(LanguageOrigin::System)));
        assert!(s.reading.is_some());
        read(&mut s);
        assert_eq!(s.reading, None);
        assert_eq!(s.status, "");
        assert_eq!(tag_text(&s), "");
    }

    #[test]
    fn button_hidden_while_requested() {
        let mut s = PerAppLanguageShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, READ_LABEL, &mut s));
        let screen = current_screen(&s);
        assert!(!click_button(&screen, READ_LABEL, &mut s));
    }
}
