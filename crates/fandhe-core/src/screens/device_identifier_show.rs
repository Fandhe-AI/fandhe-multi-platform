//! Phase 11（端末機能の拡充ループ、親 #181）型サンプル「アプリ・端末の識別子」（Issue #903）。
//! 「識別子を読む」ボタン → OS の識別子を 1 回読み、**末尾 4 文字と文字数だけ**を表示する 1 画面。
//! `FANDHE_PROXY_APP=device_identifier_show` で選ばれ、`fandhe-egui-proxy` の `screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは
//! `device/device_identifier.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`device_info_show.rs` と同型。読み取りは全 OS で同期のため読み取り中の相は置かない）。
//!
//! 全対象 OS の文言・状態をここで確定済みで、後続の OS 実装（#904）は本画面を触らない。
//! 識別子の全体は状態に持たない（[`IdentifierReading`] は末尾と文字数のみ）。全体を表示するボタンも置かない。

use crate::{Screen, WidgetId};

const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 読み取りボタンのラベル。
pub const READ_LABEL: &str = "識別子を読む";
/// 読み取り完了の案内。
pub const DONE_STATUS: &str = "読み取りました";
/// iOS の nil。失敗ではないので通常色の案内に出す。
pub const NOT_YET_STATUS: &str = "今は取れません。ロック解除後にもう一度押してください";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この OS では取れません";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 範囲の文言: iOS。
pub const SCOPE_VENDOR_TEXT: &str = "範囲: 同じ提供元のアプリで共通・この端末だけ";
/// 範囲の文言: Android。
pub const SCOPE_SIGNING_TEXT: &str =
    "範囲: この署名鍵・ユーザー・端末の組だけ（Android 7 は端末・ユーザーで共通）";
/// 値の行ラベル。
pub const VALUE_LABEL: &str = "識別子（末尾 4 文字のみ）";

/// 識別子の範囲（OS ごとに違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdScope {
    /// iOS: 提供元ごと。
    Vendor,
    /// Android: 署名鍵・ユーザー・端末の組ごと。
    SigningKeyUserDevice,
}

/// 読み取った識別子の伏せ表示用の写し（全体は持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierReading {
    /// 末尾 4 文字（短すぎる値では空）。
    pub tail: String,
    /// 全体の文字数。
    pub char_count: usize,
    /// 範囲。
    pub scope: IdScope,
}

/// 読み取りの成功結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    /// 取れた。
    Identifier(IdentifierReading),
    /// 今は取れない（失敗ではない）。
    NotYetAvailable,
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
pub struct DeviceIdentifierShowState {
    /// 進行状態。
    pub phase: ReadPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 読み取れた識別子（伏せ済み）。
    pub reading: Option<IdentifierReading>,
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

/// ボタンのハンドラ。`Idle` 以外では no-op。
pub fn read(state: &mut DeviceIdentifierShowState) {
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
pub fn take_request(state: &DeviceIdentifierShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 結果を反映する。世代が一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut DeviceIdentifierShowState,
    generation: u64,
    result: Result<ReadOutcome, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.reading = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(ReadOutcome::Identifier(r)) => {
            state.reading = Some(r);
            state.status = DONE_STATUS.to_string();
        }
        Ok(ReadOutcome::NotYetAvailable) => state.status = NOT_YET_STATUS.to_string(),
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 伏せ表示。`•` を（文字数 − 末尾の文字数）個並べて末尾を付ける。
pub fn masked_text(r: &IdentifierReading) -> String {
    let hidden = r.char_count.saturating_sub(r.tail.chars().count());
    format!("{}{}", "•".repeat(hidden), r.tail)
}

/// 値の行。未読み取りのときは空。
pub fn value_text(state: &DeviceIdentifierShowState) -> String {
    state
        .reading
        .as_ref()
        .map(|r| format!("{VALUE_LABEL}: {}", masked_text(r)))
        .unwrap_or_default()
}

/// 範囲の行。
pub fn scope_text(state: &DeviceIdentifierShowState) -> String {
    match state.reading.as_ref().map(|r| r.scope) {
        Some(IdScope::Vendor) => SCOPE_VENDOR_TEXT.to_string(),
        Some(IdScope::SigningKeyUserDevice) => SCOPE_SIGNING_TEXT.to_string(),
        None => String::new(),
    }
}

/// 案内文言。
pub fn status_text(state: &DeviceIdentifierShowState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言。
pub fn error_text(state: &DeviceIdentifierShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &DeviceIdentifierShowState) -> Screen<DeviceIdentifierShowState> {
    let screen = Screen::new("アプリ・端末の識別子")
        .dynamic_text(status_text)
        .dynamic_text(value_text)
        .dynamic_text(scope_text)
        .error_text(error_text);
    if state.phase != ReadPhase::Idle {
        return screen;
    }
    screen.button(READ_BUTTON_ID, READ_LABEL, read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn requested() -> DeviceIdentifierShowState {
        let mut s = DeviceIdentifierShowState::default();
        read(&mut s);
        s
    }

    fn reading(scope: IdScope) -> IdentifierReading {
        IdentifierReading {
            tail: "ABCD".into(),
            char_count: 36,
            scope,
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
        finish_read(&mut s, 99, Ok(ReadOutcome::NotYetAvailable));
        assert_eq!(s.phase, ReadPhase::Requested);
        let mut idle = DeviceIdentifierShowState::default();
        finish_read(
            &mut idle,
            0,
            Ok(ReadOutcome::Identifier(reading(IdScope::Vendor))),
        );
        assert_eq!(idle.reading, None);
    }

    #[test]
    fn not_yet_is_status_not_error() {
        let mut s = requested();
        finish_read(&mut s, 1, Ok(ReadOutcome::NotYetAvailable));
        assert_eq!(s.error, "");
        assert_eq!(s.status, NOT_YET_STATUS);
        assert_eq!(s.reading, None);
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
        finish_read(
            &mut s,
            1,
            Ok(ReadOutcome::Identifier(reading(IdScope::Vendor))),
        );
        assert!(s.reading.is_some());
        read(&mut s);
        assert_eq!(s.reading, None);
        assert_eq!(s.status, "");
    }

    #[test]
    fn masked_text_hides_all_but_tail() {
        let t = masked_text(&reading(IdScope::Vendor));
        assert_eq!(t.chars().filter(|c| *c == '•').count(), 32);
        assert!(t.ends_with("ABCD"));
    }

    #[test]
    fn scope_text_per_scope() {
        let mut s = requested();
        finish_read(
            &mut s,
            1,
            Ok(ReadOutcome::Identifier(reading(IdScope::Vendor))),
        );
        assert_eq!(scope_text(&s), SCOPE_VENDOR_TEXT);
        read(&mut s);
        finish_read(
            &mut s,
            2,
            Ok(ReadOutcome::Identifier(reading(
                IdScope::SigningKeyUserDevice,
            ))),
        );
        assert_eq!(scope_text(&s), SCOPE_SIGNING_TEXT);
    }

    #[test]
    fn button_hidden_while_requested() {
        let mut s = DeviceIdentifierShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, READ_LABEL, &mut s));
        let screen = current_screen(&s);
        assert!(!click_button(&screen, READ_LABEL, &mut s));
    }
}
