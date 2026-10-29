//! Phase 11（端末機能の拡充ループ、親 #185）型サンプル「MDM の管理アプリ設定」（Issue #944）。
//! 「管理設定を読む」ボタン → MDM が配った管理アプリ設定のうち固定 3 キー（真偽・整数・文字列）を
//! 1 回読み、キーごとの状態を表示する 1 画面。`FANDHE_PROXY_APP=managed_app_config_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod managed_app_config_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/managed_app_config.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`device_info_show.rs` と同型の状態機械）。キー名は `fandhe-device` 側の定数が SSOT で、
//! 二重管理を避けるため画面には書かない。
//!
//! 全対象 OS（macOS / iOS / Android）の文言・状態をここで確定済みで、後続の OS 実装は本画面を
//! 触らない。
//!
//! # 設定なしと失敗の区別
//!
//! 「設定なし」は成功（通常色）で、失敗の文言とは別にする。MDM で配られていない端末でも同じに
//! なるため「未管理」とは言わない。文字列の値は文字数だけ表示し、中身は受け取る型も持たない
//! （`device-bridge-contracts.md`「6. 嘘の成功」「8. 秘密値」）。
//!
//! ```text
//! Idle --「管理設定を読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 読み取りボタンのラベル。
pub const READ_LABEL: &str = "管理設定を読む";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 設定ありの案内。
pub const CONFIG_STATUS: &str = "管理者の設定を読みました";
/// 設定なしの案内（失敗ではない）。
pub const NO_CONFIG_STATUS: &str =
    "管理者の設定はありません（MDM で配布されていない端末ではこう出ます）";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では管理アプリ設定を読めません";
/// 失敗時の案内の接頭辞（`{接頭辞}{固定文言}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 未設定の表示。
pub const UNSET_TEXT: &str = "未設定";
/// 型違いの表示。
pub const WRONG_TYPE_TEXT: &str = "型が違います（読めません）";
/// その他のキーを数えない OS の表示。
pub const OTHER_NOT_COUNTED_TEXT: &str = "数えません（キーごとに読む OS のため）";

/// 1 キーの表示用状態。文字列は文字数だけで中身を持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyView {
    /// 管理値が無い。
    Unset,
    /// 真偽。
    Bool(bool),
    /// 整数。
    Integer(i64),
    /// 文字列（文字数のみ）。
    Text {
        /// 文字数。
        chars: u32,
    },
    /// キーの型と違う値。
    WrongType,
}

/// 読み取れた結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadingView {
    /// 3 キーとも未設定。
    NoConfig,
    /// 設定あり。
    Config {
        /// 真偽のキー。
        boolean: KeyView,
        /// 整数のキー。
        integer: KeyView,
        /// 文字列のキー。
        text: KeyView,
        /// 固定外のキーの件数。数えない OS は `None`。
        other_keys: Option<u32>,
    },
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
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
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から読んでいる最中。
    Reading,
}

/// 管理アプリ設定画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedAppConfigShowState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた結果。失敗・読み取り中は `None`。
    pub reading: Option<ReadingView>,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空。
    pub error: String,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    /// [`finish_read`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn read(state: &mut ManagedAppConfigShowState) {
    if state.phase != ReadPhase::Idle {
        return;
    }
    state.phase = ReadPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.reading = None;
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &ManagedAppConfigShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut ManagedAppConfigShowState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut ManagedAppConfigShowState,
    generation: u64,
    result: Result<ReadingView, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.reading = None;
    state.error = String::new();
    match result {
        Ok(v) => state.reading = Some(v),
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &ManagedAppConfigShowState) -> String {
    match (&state.reading, state.phase) {
        (Some(ReadingView::NoConfig), _) => NO_CONFIG_STATUS.to_string(),
        (Some(ReadingView::Config { .. }), _) => CONFIG_STATUS.to_string(),
        (None, ReadPhase::Reading) => READING_STATUS.to_string(),
        (None, _) => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &ManagedAppConfigShowState) -> String {
    state.error.clone()
}

fn key_text(k: KeyView) -> String {
    match k {
        KeyView::Unset => UNSET_TEXT.to_string(),
        KeyView::Bool(true) => "オン".to_string(),
        KeyView::Bool(false) => "オフ".to_string(),
        KeyView::Integer(n) => n.to_string(),
        KeyView::Text { chars } => format!("{chars} 文字（中身は表示しません）"),
        KeyView::WrongType => WRONG_TYPE_TEXT.to_string(),
    }
}

/// 設定ありのときだけ `{ラベル}: {値}`、それ以外は空。
fn row(
    state: &ManagedAppConfigShowState,
    label: &str,
    pick: fn(&ReadingView) -> Option<String>,
) -> String {
    state
        .reading
        .as_ref()
        .and_then(pick)
        .map_or_else(String::new, |v| format!("{label}: {v}"))
}

/// 真偽の行。
pub fn bool_text(state: &ManagedAppConfigShowState) -> String {
    row(state, "真偽の設定", |r| match r {
        ReadingView::Config { boolean, .. } => Some(key_text(*boolean)),
        _ => None,
    })
}

/// 整数の行。
pub fn integer_text(state: &ManagedAppConfigShowState) -> String {
    row(state, "整数の設定", |r| match r {
        ReadingView::Config { integer, .. } => Some(key_text(*integer)),
        _ => None,
    })
}

/// 文字列の行（文字数のみ）。
pub fn text_text(state: &ManagedAppConfigShowState) -> String {
    row(state, "文字列の設定", |r| match r {
        ReadingView::Config { text, .. } => Some(key_text(*text)),
        _ => None,
    })
}

/// その他のキーの行。
pub fn other_text(state: &ManagedAppConfigShowState) -> String {
    row(state, "その他のキー", |r| match r {
        ReadingView::Config { other_keys, .. } => Some(match other_keys {
            Some(n) => format!("{n} 件（名前と値は表示しません）"),
            None => OTHER_NOT_COUNTED_TEXT.to_string(),
        }),
        _ => None,
    })
}

/// 画面を組み立てる。`Idle` のときのみボタンを出す。
pub fn current_screen(state: &ManagedAppConfigShowState) -> Screen<ManagedAppConfigShowState> {
    let screen = Screen::new("MDM の管理アプリ設定")
        .dynamic_text(status_text)
        .dynamic_text(bool_text)
        .dynamic_text(integer_text)
        .dynamic_text(text_text)
        .dynamic_text(other_text)
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

    fn reading() -> ManagedAppConfigShowState {
        let mut s = ManagedAppConfigShowState::default();
        read(&mut s);
        mark_reading(&mut s);
        s
    }

    fn config() -> ReadingView {
        ReadingView::Config {
            boolean: KeyView::Bool(true),
            integer: KeyView::Integer(-3),
            text: KeyView::Text { chars: 4 },
            other_keys: None,
        }
    }

    #[test]
    fn startup_does_not_request() {
        let s = ManagedAppConfigShowState::default();
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn read_is_noop_while_in_flight() {
        let mut s = ManagedAppConfigShowState::default();
        read(&mut s);
        assert_eq!(take_request(&s), Some(ReadRequest { generation: 1 }));
        mark_reading(&mut s);
        assert_eq!(take_request(&s), None);
        read(&mut s);
        assert_eq!(s.phase, ReadPhase::Reading);
        assert_eq!(s.generation, 1);
        assert_eq!(status_text(&s), READING_STATUS);
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = reading();
        finish_read(&mut s, 99, Ok(config()));
        assert_eq!(s.phase, ReadPhase::Reading);
        assert_eq!(s.reading, None);
        let mut idle = ManagedAppConfigShowState::default();
        finish_read(&mut idle, 0, Ok(config()));
        assert_eq!(idle.reading, None);
    }

    #[test]
    fn no_config_is_success_not_error() {
        let mut s = reading();
        finish_read(&mut s, 1, Ok(ReadingView::NoConfig));
        assert_eq!(status_text(&s), NO_CONFIG_STATUS);
        assert_eq!(error_text(&s), "");
        assert_eq!(bool_text(&s), "");
        assert_eq!(other_text(&s), "");
    }

    #[test]
    fn config_rows() {
        let mut s = reading();
        finish_read(&mut s, 1, Ok(config()));
        assert_eq!(status_text(&s), CONFIG_STATUS);
        assert_eq!(bool_text(&s), "真偽の設定: オン");
        assert_eq!(integer_text(&s), "整数の設定: -3");
        assert_eq!(text_text(&s), "文字列の設定: 4 文字（中身は表示しません）");
        assert_eq!(
            other_text(&s),
            format!("その他のキー: {OTHER_NOT_COUNTED_TEXT}")
        );
    }

    #[test]
    fn unset_wrong_type_and_other_count() {
        let mut s = reading();
        finish_read(
            &mut s,
            1,
            Ok(ReadingView::Config {
                boolean: KeyView::Unset,
                integer: KeyView::WrongType,
                text: KeyView::Bool(false),
                other_keys: Some(2),
            }),
        );
        assert_eq!(bool_text(&s), format!("真偽の設定: {UNSET_TEXT}"));
        assert_eq!(integer_text(&s), format!("整数の設定: {WRONG_TYPE_TEXT}"));
        assert_eq!(text_text(&s), "文字列の設定: オフ");
        assert_eq!(
            other_text(&s),
            "その他のキー: 2 件（名前と値は表示しません）"
        );
    }

    #[test]
    fn failure_clears_previous_rows() {
        let mut s = reading();
        finish_read(&mut s, 1, Ok(config()));
        read(&mut s);
        mark_reading(&mut s);
        finish_read(&mut s, 2, Err(ReadFailure::Failed("x".to_string())));
        assert_eq!(s.reading, None);
        assert_eq!(bool_text(&s), "");
        assert_eq!(status_text(&s), "");
        assert_eq!(error_text(&s), format!("{FAILED_PREFIX}x"));
    }

    #[test]
    fn unsupported_shows_error_only() {
        let mut s = reading();
        finish_read(&mut s, 1, Err(ReadFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_STATUS);
        assert_eq!(status_text(&s), "");
    }

    #[test]
    fn button_hidden_while_busy() {
        let mut s = ManagedAppConfigShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, READ_LABEL, &mut s));
        assert_eq!(s.phase, ReadPhase::Requested);
        let busy = current_screen(&s);
        assert!(!click_button(&busy, READ_LABEL, &mut s));
    }
}
