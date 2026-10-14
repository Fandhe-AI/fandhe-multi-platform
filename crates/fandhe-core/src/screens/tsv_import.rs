//! Phase 11 の拡充ループ型サンプル「TSV 読み込み」（親 #979、Issue #980）。
//! 「TSV を選ぶ」→ OS 標準の「開く」ダイアログ（`screens::file_pick` と同じ
//! `PickPhase`/世代番号の状態機械）→ [`crate::tsv::parse_tsv`] で解析 → 行数・列数・
//! 先頭数行を表示する 1 画面。ファイル選択が使えない環境向けに、同梱のサンプル TSV
//! （[`SAMPLE_TSV`]）を解析するボタンも置く。`FANDHE_PROXY_APP=tsv_import` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod tsv_import;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。実際のダイアログ表示・
//! バイト列の取得は本モジュール外（`fandhe-egui-proxy` の
//! `device/tsv_import.rs`）が [`take_request`] で要求を観測し [`finish_import`] で
//! 結果を注入する。橋渡しの実体（`bridge`・effect ドライバ）は `screens::file_pick` と
//! 共有する（`device/file_pick.rs` の `PickHooks` 参照）。`FANDHE_PROXY_APP` で起動する
//! 画面は 1 つだけのため、同じプロセスで file_pick と tsv_import の両方が動くことはない
//! （bridge の static スロットを共有できる前提、`device/file_pick.rs` モジュール doc 参照）。
//!
//! # 成功・キャンセル・失敗・0 行を混同しない理由
//!
//! `file_pick`/`tts_speak` と同じ再発防止方針: 0 行（データが無いだけ）・
//! UTF-8 でない・使えない環境を「成功」に丸めると、ユーザーは自分の TSV が
//! 読めているのか区別できない（A08 対策）。本画面は成功（1 行以上）・0 行・
//! キャンセル・失敗を別々の文言・別々のフィールド（`status`/`error`）に書き分ける。
//!
//! ```text
//! Idle --「TSV を選ぶ」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_picking--> phase=Picking
//!   （OS のダイアログが開く。ユーザーが閉じるまで戻らない）
//!   --finish_import（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! Idle --「サンプルを読み込む」--> load_sample がその場で反映（ダイアログを介さない）
//! ```

use crate::tsv::{TsvError, parse_tsv};
use crate::{Node, Screen, WidgetId};

pub use crate::screens::file_pick::PickPhase;

/// 「TSV を選ぶ」ボタンの `WidgetId`。
const PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「サンプルを読み込む」ボタンの `WidgetId`。
const SAMPLE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// プレビューに出す先頭データ行数。
const PREVIEW_ROWS: usize = 5;
/// プレビュー 1 行あたりの最大文字数（`chars().take`。巨大なセルで画面が崩れないため）。
const PREVIEW_CHARS: usize = 80;

/// 未操作時の案内。
pub const IDLE_HINT: &str = "「TSV を選ぶ」で OS のダイアログが開きます";
/// キャンセルされたときの案内（失敗ではない、モジュール doc 参照）。
pub const CANCELLED_STATUS: &str = "選択しませんでした";
/// 0 行だったときの案内（成功の文言を含めない、モジュール doc 参照）。
pub const EMPTY_STATUS: &str = "データ行がありません（0 行）";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文言}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 同梱のサンプル TSV（ユーザー辞書風の 4 列: 読み・単語・品詞・コメント）。
pub const SAMPLE_TSV: &str = "\
よみ\tたんご\tひんし\tコメント\n\
ふぁんで\tfandhe\t名詞\tフレームワーク名\n\
たすく\tTASK\t名詞\tスペック用語\n\
いしゅー\tissue\t名詞\tGitHub 用語\n";

/// TSV 読み込み画面の状態。ファイルのバイト列や解析済み表全体は持たない
/// （うっかり表示・ログ出力する経路を作らないため、`FilePickState.picked` の設計と同型）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TsvImportState {
    /// 選択操作の進行状態（`screens::file_pick::PickPhase` を再利用）。
    pub phase: PickPhase,
    /// 直近に立てた要求の世代番号。要求のたびに進み、完了通知が「今の要求」を
    /// 指しているかの照合に使う（`file_pick` と同じ判断）。
    pub generation: u64,
    /// 成功・0 行・キャンセルの案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功・0 行・キャンセル時は空文字列。
    pub error: String,
    /// 先頭 [`PREVIEW_ROWS`] 行のプレビュー（` | ` 連結・[`PREVIEW_CHARS`] 文字で切り詰め）。
    pub preview: Vec<String>,
}

/// 「TSV を選ぶ」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op。
pub fn request_import(state: &mut TsvImportState) {
    if state.phase != PickPhase::Idle {
        return;
    }
    state.phase = PickPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない、
/// `screens::file_pick::take_request` と同型）。
pub fn take_request(state: &TsvImportState) -> Option<u64> {
    (state.phase == PickPhase::Requested).then_some(state.generation)
}

/// 要求をダイアログ表示中（`Picking`）へ進める。`Requested` 以外では何もしない。
pub fn mark_picking(state: &mut TsvImportState) {
    if state.phase == PickPhase::Requested {
        state.phase = PickPhase::Picking;
    }
}

/// 選択されたバイト列の解析結果を反映する。`generation` は [`take_request`] が
/// 返した世代番号。現在の世代と一致しない、または `Idle`（要求が無い）状態で
/// 届いた完了は無視する（`screens::file_pick::finish_pick` と同じ照合）。
pub fn finish_import(
    state: &mut TsvImportState,
    generation: u64,
    result: Result<Option<Vec<u8>>, String>,
) {
    if state.phase == PickPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PickPhase::Idle;
    apply_result(state, result.map(|opt| opt.map(|bytes| parse_tsv(&bytes))));
}

/// 「サンプルを読み込む」ボタンのハンドラ。`Idle` のときだけ同梱サンプルを解析して
/// 反映する（ダイアログを介さない、`phase`/`generation` は変えない）。
pub fn load_sample(state: &mut TsvImportState) {
    if state.phase != PickPhase::Idle {
        return;
    }
    apply_result(state, Ok(Some(parse_tsv(SAMPLE_TSV.as_bytes()))));
}

/// [`finish_import`]/[`load_sample`] の共通反映処理。どの結果でも最初に preview を
/// クリアしてから書き分ける（成功・0 行・キャンセル・失敗の 4 分岐、モジュール doc 参照）。
fn apply_result(
    state: &mut TsvImportState,
    result: Result<Option<Result<crate::tsv::TsvTable, TsvError>>, String>,
) {
    state.preview.clear();
    match result {
        Ok(Some(Ok(table))) if !table.is_empty() => {
            state.status = format!(
                "{} 行 × {} 列を読み込みました",
                table.row_count(),
                table.column_count()
            );
            state.error = String::new();
            state.preview = table
                .rows
                .iter()
                .take(PREVIEW_ROWS)
                .map(|row| truncate_preview(&row.join(" | ")))
                .collect();
        }
        Ok(Some(Ok(_))) => {
            state.status = EMPTY_STATUS.to_string();
            state.error = String::new();
        }
        Ok(Some(Err(parse_error))) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{parse_error}");
        }
        Ok(None) => {
            state.status = CANCELLED_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// [`PREVIEW_CHARS`] 文字（`chars().take`）で切り詰める。
fn truncate_preview(line: &str) -> String {
    line.chars().take(PREVIEW_CHARS).collect()
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &TsvImportState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &TsvImportState) -> String {
    state.error.clone()
}

/// プレビュー行を `Node::Text` 一覧へ展開する（`.list()` の `expand`）。
fn preview_nodes(state: &TsvImportState) -> Vec<Node<TsvImportState>> {
    state.preview.iter().cloned().map(Node::Text).collect()
}

/// TSV 読み込み画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。`Idle` のときのみボタン 2 つを出し、
/// 処理中（`Requested`/`Picking`）は二重押しを防ぐため外す（`file_pick` と同型）。
pub fn current_screen(state: &TsvImportState) -> Screen<TsvImportState> {
    let screen = Screen::new("TSV 読み込み")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(preview_nodes);
    if state.phase != PickPhase::Idle {
        return screen;
    }
    screen
        .button(PICK_BUTTON_ID, "TSV を選ぶ", request_import)
        .button(SAMPLE_BUTTON_ID, "サンプルを読み込む", load_sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_import_advances_generation_and_phase() {
        let mut state = TsvImportState::default();
        request_import(&mut state);
        assert_eq!(state.phase, PickPhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn request_import_is_noop_while_in_flight() {
        let mut state = TsvImportState {
            phase: PickPhase::Picking,
            generation: 1,
            ..Default::default()
        };
        request_import(&mut state);
        assert_eq!(state.phase, PickPhase::Picking);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn take_request_returns_generation_only_when_requested() {
        let mut state = TsvImportState::default();
        assert_eq!(take_request(&state), None);
        request_import(&mut state);
        assert_eq!(take_request(&state), Some(1));
        mark_picking(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn finish_import_success_fills_status_and_preview() {
        let mut state = TsvImportState::default();
        request_import(&mut state);
        mark_picking(&mut state);
        finish_import(&mut state, 1, Ok(Some(b"a\tb\nc\td\n".to_vec())));
        assert_eq!(state.phase, PickPhase::Idle);
        assert!(state.status.contains("2 行"));
        assert!(state.status.contains("2 列"));
        assert_eq!(state.error, "");
        assert_eq!(state.preview, vec!["a | b", "c | d"]);
    }

    #[test]
    fn finish_import_cancel_sets_cancelled_status_without_success_wording() {
        let mut state = TsvImportState::default();
        request_import(&mut state);
        mark_picking(&mut state);
        finish_import(&mut state, 1, Ok(None));
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_eq!(state.error, "");
        assert!(!state.status.contains("読み込みました"));
    }

    #[test]
    fn finish_import_failure_sets_error_without_success_status() {
        let mut state = TsvImportState::default();
        request_import(&mut state);
        mark_picking(&mut state);
        finish_import(
            &mut state,
            1,
            Err("8 MiB を超えるファイルは選べません".to_string()),
        );
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn finish_import_parse_error_sets_error_without_success_status() {
        let mut state = TsvImportState::default();
        request_import(&mut state);
        mark_picking(&mut state);
        finish_import(&mut state, 1, Ok(Some(vec![0xFF, 0xFE])));
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
        assert!(state.error.contains("UTF-8"));
    }

    #[test]
    fn finish_import_zero_rows_uses_empty_status_without_error() {
        let mut state = TsvImportState::default();
        request_import(&mut state);
        mark_picking(&mut state);
        finish_import(&mut state, 1, Ok(Some(Vec::new())));
        assert_eq!(state.status, EMPTY_STATUS);
        assert_eq!(state.error, "");
        assert!(!state.status.contains("読み込みました"));
    }

    #[test]
    fn finish_import_ignores_stale_generation() {
        let mut state = TsvImportState::default();
        request_import(&mut state); // generation = 1
        mark_picking(&mut state);
        finish_import(&mut state, 1, Ok(None)); // Idle に戻る

        request_import(&mut state); // generation = 2
        finish_import(&mut state, 1, Ok(Some(b"old\n".to_vec())));
        // 古い世代の完了は無視され、Requested のまま
        assert_eq!(state.phase, PickPhase::Requested);
        assert_eq!(state.status, CANCELLED_STATUS);
    }

    #[test]
    fn finish_import_ignores_completion_when_idle() {
        let mut state = TsvImportState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_import(&mut state, 0, Ok(None));
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn load_sample_fills_preview_when_idle() {
        let mut state = TsvImportState::default();
        load_sample(&mut state);
        assert!(state.status.contains("行"));
        assert!(!state.preview.is_empty());
        assert_eq!(state.phase, PickPhase::Idle);
    }

    #[test]
    fn load_sample_is_noop_while_in_flight() {
        let mut state = TsvImportState {
            phase: PickPhase::Picking,
            ..Default::default()
        };
        load_sample(&mut state);
        assert!(state.preview.is_empty());
        assert_eq!(state.status, "");
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = TsvImportState {
            phase: PickPhase::Picking,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "TSV を選ぶ", &mut state));
        assert!(!click_button(&screen, "サンプルを読み込む", &mut state));
    }

    #[test]
    fn buttons_are_clickable_when_idle() {
        use crate::click_button;
        let mut state = TsvImportState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "サンプルを読み込む", &mut state));
        assert!(!state.preview.is_empty());
    }
}
