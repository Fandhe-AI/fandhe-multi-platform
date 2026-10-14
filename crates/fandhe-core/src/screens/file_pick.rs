//! Phase 11（端末機能の拡充ループ、親 #871）型サンプル「ファイル選択」（Issue #872）。
//! 「ファイルを選ぶ」ボタン → OS 標準の「開く」ダイアログ → 選択結果（名前とサイズ）を
//! 表示する 1 画面。`FANDHE_PROXY_APP=file_pick` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod file_pick;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際のダイアログ表示・読み取りは
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、`device/file_pick.rs`）が
//! [`take_request`] で要求を観測し [`finish_pick`] で結果を注入する。
//!
//! # 成功・キャンセル・失敗を混同しない理由
//!
//! `tts_speak`/`clipboard_text` と同じ再発防止方針（#859 P1 指摘と同型）: 選択できない
//! 環境やサイズ超過を黙って「選択しませんでした」に丸めると、ユーザーは自分のファイルが
//! 選ばれたのか単に開かなかったのか区別できない。本画面は成功・キャンセルを `status`
//! （通常色）、失敗を `error`（`error_text` で `TextRole::Error`）の別フィールドへ
//! 書き分ける。
//!
//! ```text
//! Idle --「ファイルを選ぶ」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_picking--> phase=Picking
//!   （OS のダイアログが開く。ユーザーが閉じるまで戻らない）
//!   --finish_pick（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```
//!
//! 世代番号（`generation`）は [`take_request`]/[`finish_pick`] の照合に使い、
//! `Idle` へ戻った後や新しい要求で上書きされた古い完了を無視する
//! （`tts_speak` の `speak_generation` と同じ判断）。

use crate::{Screen, WidgetId};

/// 「ファイルを選ぶ」ボタンの `WidgetId`。
const PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "「ファイルを選ぶ」で OS のダイアログが開きます";
/// キャンセルされたときの案内（失敗ではない、モジュール doc 参照）。
pub const CANCELLED_STATUS: &str = "選択しませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 選択操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバがダイアログを開いて処理中（[`mark_picking`] 済み）。
    Picking,
}

/// 選択されたファイルの表示用要約（内容そのものは画面状態に持たない。
/// バイト列を保持し続ける必要が無いうえ、パスと同様うっかり表示・ログ出力する
/// 経路を作らないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedSummary {
    /// ファイル名。
    pub name: String,
    /// ファイルサイズ（バイト）。
    pub size: usize,
}

/// ファイル選択画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilePickState {
    /// 選択操作の進行状態。
    pub phase: PickPhase,
    /// 直近に立てた要求の世代番号。要求のたびに進み、完了通知が「今の要求」を
    /// 指しているかの照合に使う（モジュール doc 参照）。
    pub generation: u64,
    /// 直近に選択できたファイルの要約（未選択・キャンセル直後も前回値を保持する）。
    pub picked: Option<PickedSummary>,
    /// 成功・キャンセルの案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする
    /// （成功と失敗を取り違えないための分離、モジュール doc 参照）。
    pub error: String,
}

/// 「ファイルを選ぶ」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。
pub fn request_pick(state: &mut FilePickState) {
    if state.phase != PickPhase::Idle {
        return;
    }
    state.phase = PickPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない。
/// 実際に `Picking` へ進めるのは [`mark_picking`]）。戻り値の世代番号は
/// [`finish_pick`] へそのまま引き渡す契約。
pub fn take_request(state: &FilePickState) -> Option<u64> {
    (state.phase == PickPhase::Requested).then_some(state.generation)
}

/// 要求をダイアログ表示中（`Picking`）へ進める。`Requested` 以外では何もしない
/// （`take_request` が `None` を返した後に誤って呼ばれても状態を壊さない）。
pub fn mark_picking(state: &mut FilePickState) {
    if state.phase == PickPhase::Requested {
        state.phase = PickPhase::Picking;
    }
}

/// 選択結果を反映する。`generation` は [`take_request`] が返した世代番号（呼び出し元が
/// ダイアログ表示中も保持し続ける契約）。現在の世代と一致しない（＝新しい要求で
/// 既に上書きされた）完了は無視する。`Idle`（要求が無い）状態で届いた完了も無視する。
pub fn finish_pick(
    state: &mut FilePickState,
    generation: u64,
    result: Result<Option<PickedSummary>, String>,
) {
    if state.phase == PickPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PickPhase::Idle;
    match result {
        Ok(Some(summary)) => {
            state.status = format!("{}（{} バイト）を選びました", summary.name, summary.size);
            state.error = String::new();
            state.picked = Some(summary);
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

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &FilePickState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &FilePickState) -> String {
    state.error.clone()
}

/// ファイル選択画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。`Idle` のときのみボタンを出し、
/// 処理中（`Requested`/`Picking`）は二重押しを防ぐため外す
/// （`clipboard_text::current_screen` と同型）。
pub fn current_screen(state: &FilePickState) -> Screen<FilePickState> {
    let screen = Screen::new("ファイル選択")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != PickPhase::Idle {
        return screen;
    }
    screen.button(PICK_BUTTON_ID, "ファイルを選ぶ", request_pick)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_pick_advances_generation_and_phase() {
        let mut state = FilePickState::default();
        request_pick(&mut state);
        assert_eq!(state.phase, PickPhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn request_pick_is_noop_while_in_flight() {
        let mut state = FilePickState {
            phase: PickPhase::Picking,
            generation: 1,
            ..Default::default()
        };
        request_pick(&mut state);
        assert_eq!(state.phase, PickPhase::Picking);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn take_request_returns_generation_only_when_requested() {
        let mut state = FilePickState::default();
        assert_eq!(take_request(&state), None);
        request_pick(&mut state);
        assert_eq!(take_request(&state), Some(1));
        mark_picking(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn finish_pick_success_sets_status_and_picked() {
        let mut state = FilePickState::default();
        request_pick(&mut state);
        mark_picking(&mut state);
        finish_pick(
            &mut state,
            1,
            Ok(Some(PickedSummary {
                name: "memo.txt".to_string(),
                size: 42,
            })),
        );
        assert_eq!(state.phase, PickPhase::Idle);
        assert!(state.status.contains("memo.txt"));
        assert!(state.status.contains("42"));
        assert_eq!(state.error, "");
        assert_eq!(
            state.picked,
            Some(PickedSummary {
                name: "memo.txt".to_string(),
                size: 42
            })
        );
    }

    #[test]
    fn finish_pick_cancel_sets_cancelled_status_without_success_wording() {
        let mut state = FilePickState::default();
        request_pick(&mut state);
        mark_picking(&mut state);
        finish_pick(&mut state, 1, Ok(None));
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_eq!(state.error, "");
        assert!(!state.status.contains("選びました"));
    }

    #[test]
    fn finish_pick_error_sets_error_without_success_status() {
        let mut state = FilePickState::default();
        request_pick(&mut state);
        mark_picking(&mut state);
        finish_pick(
            &mut state,
            1,
            Err("8 MiB を超えるファイルは選べません".to_string()),
        );
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn finish_pick_ignores_stale_generation() {
        let mut state = FilePickState::default();
        request_pick(&mut state); // generation = 1
        mark_picking(&mut state);
        finish_pick(&mut state, 1, Ok(None)); // Idle に戻る、generation は 1 のまま

        request_pick(&mut state); // generation = 2
        finish_pick(
            &mut state,
            1,
            Ok(Some(PickedSummary {
                name: "old".to_string(),
                size: 1,
            })),
        );
        // 古い世代の完了は無視され、Requested のまま
        assert_eq!(state.phase, PickPhase::Requested);
        assert_eq!(state.status, CANCELLED_STATUS);
    }

    #[test]
    fn finish_pick_ignores_completion_when_idle() {
        let mut state = FilePickState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_pick(&mut state, 0, Ok(None));
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = FilePickState {
            phase: PickPhase::Picking,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "ファイルを選ぶ", &mut state));
    }

    #[test]
    fn button_is_clickable_when_idle() {
        use crate::click_button;
        let mut state = FilePickState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "ファイルを選ぶ", &mut state));
        assert_eq!(state.phase, PickPhase::Requested);
    }
}
