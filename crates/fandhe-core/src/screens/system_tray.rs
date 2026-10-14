//! Phase 11（端末機能の拡充ループ、親 #1038）型サンプル「システムトレイ」
//! （Issue #1039）。「出す」ボタン → メニューバー / 通知領域にアイコンを表示し、
//! メニュー選択で画面の文言を切り替える 1 画面。`FANDHE_PROXY_APP=system_tray` で
//! 選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から
//! 呼ばれる（`screens/mod.rs` の `pub mod system_tray;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/system_tray.rs`）が
//! [`take_action`] で要求を観測し [`finish_show`]/[`finish_hide`] で結果を注入する
//! （`notification_send.rs` と同型の状態機械）。
//!
//! ```text
//! Hidden --「出す」--> ShowRequested, generation += 1
//!   --effect が take_action で観測 → mark_showing--> Showing --OS へ show--> （結果を待つ）
//!   --finish_show（世代一致のみ反映）--+-- Ok(Added/AlreadyShown) --> Shown
//!                                      '-- Err --> Hidden（失敗時は表示できていない）
//! Shown --「消す」/ メニューの「消す」項目--> HideRequested, generation += 1
//!   --effect が take_action で観測 → mark_hiding--> Hiding --OS へ hide--> （結果を待つ）
//!   --finish_hide（世代一致のみ反映）--+-- Ok(Removed/NotShown) --> Hidden
//!                                      '-- Err --> Shown（失敗時はまだ表示中）
//! Shown --メニュー選択（「消す」以外）--> apply_selection が message を書き換える
//! ```
//!
//! # 失敗時に Hidden/Shown のどちらへ戻すか（嘘の成功を出さない）
//!
//! `show` 失敗時は「表示できていない」が事実なので `Hidden` へ戻す。`hide` 失敗時は
//! OS 側のアイコンが残っている可能性が高いため `Shown` のままにする（利用者が
//! 「消す」を再度押せるようにする。`device-bridge-contracts.md`「6. 嘘の成功」参照）。
//!
//! # `Showing`/`Hiding`（処理中フェーズを分ける理由、codex-review 指摘 Issue #1039 是正）
//!
//! 以前は `take_action` が観測した時点の `phase`（`ShowRequested`/`HideRequested`）を
//! そのまま OS 呼び出し中も保持し、二重 dispatch の防止は `fandhe-egui-proxy` 側の
//! プロセス全体 `static Mutex<u64>`（「直近に dispatch した世代」）に頼っていた。
//! しかし `generation` は `SystemTrayState` インスタンスごとに 0 始まりで進むため、
//! 画面を作り直す（新しい `SystemTrayState` を作る）と世代が 1 から再び使われ、
//! 別インスタンスの dispatch 済み世代と衝突し、新しい「出す」要求が
//! `ShowRequested` のまま固着しうった（`device-bridge-contracts.md`「2. 並行性・
//! 世代」）。`notification_send::mark_sending` と同型の「要求済み→処理中」遷移を
//! 状態機械側に持たせることで、二重 dispatch 防止をプロセス全体の static ではなく
//! 各 `SystemTrayState` 自身の `phase` で表現する（インスタンス間で衝突しない）。

use crate::{Screen, WidgetId};

/// 「出す」ボタンの `WidgetId`。
const SHOW_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「消す」ボタンの `WidgetId`。
const HIDE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// メニュー項目（定義順。[`HIDE_ITEM_INDEX`] 番目が「消す」）。
pub const MENU_ITEMS: [&str; 3] = ["朝のあいさつ", "夜のあいさつ", "消す"];
/// [`MENU_ITEMS`] 中の「消す」項目の index。選択されたら [`request_hide`] と同じ
/// 経路（`HideRequested`）へ載せ、非表示の経路を 1 本にする。
pub const HIDE_ITEM_INDEX: usize = 2;
/// メニューバー・通知領域に出すタイトル。
pub const TRAY_TITLE: &str = "Fandhe";

/// 表示中に切り替わる大きな表示文言の初期値。
const INITIAL_MESSAGE: &str = "メニューから選んでください";
/// 「朝のあいさつ」選択時の文言。
const MORNING_MESSAGE: &str = "おはようございます";
/// 「夜のあいさつ」選択時の文言。
const EVENING_MESSAGE: &str = "こんばんは";

/// 処理中（`ShowRequested`/`HideRequested`）の案内。
pub const PROCESSING_STATUS: &str = "処理中…";
/// 表示成功時の案内。「表示しました」ではなく「出しました」に留めるのは
/// `fandhe_device::system_tray` モジュール doc の嘘の成功対策と同じ方針だが、
/// 本機能は OS 側で確認できる操作のため断定してよい（`notification_send` の
/// `Submitted`/`Shown` の区別とは事情が異なる）。
pub const SHOWN_STATUS: &str = "メニューバー / 通知領域に出しました";
/// 既に表示中だったときの案内（エラーではない）。
pub const ALREADY_SHOWN_STATUS: &str = "既に表示しています";
/// 非表示にできたときの案内。
pub const HIDDEN_STATUS: &str = "消しました";
/// 元々表示していなかったときの案内（エラーではない）。
pub const NOT_SHOWN_STATUS: &str = "表示していません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// システムトレイ操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrayPhase {
    /// 非表示（初期状態）。
    #[default]
    Hidden,
    /// 「出す」が押され、effect ドライバへまだ渡していない。
    ShowRequested,
    /// effect ドライバが要求を受け取り OS へ `show` を依頼中（[`mark_showing`]
    /// 済み）。`take_action` はこの状態では要求を返さない（二重 dispatch 防止、
    /// モジュール doc「`Showing`/`Hiding`」参照）。
    Showing,
    /// 表示中。メニュー選択を受け付ける。
    Shown,
    /// 「消す」（ボタンまたはメニュー項目）が押され、effect ドライバへまだ
    /// 渡していない。
    HideRequested,
    /// effect ドライバが要求を受け取り OS へ `hide` を依頼中（[`mark_hiding`]
    /// 済み）。
    Hiding,
}

/// [`fandhe_device::system_tray::ShowOutcome`] と 1 対 1 対応するコア側の 2 値
/// （`fandhe-core` は `fandhe-device` に依存しないため、写像は `fandhe-egui-proxy`
/// 側の責務、モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowResult {
    /// 新規にアイコンを追加した。
    Added,
    /// 既に表示中だった（エラーではない）。
    AlreadyShown,
}

/// [`fandhe_device::system_tray::HideOutcome`] と 1 対 1 対応するコア側の 2 値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideResult {
    /// アイコンを削除した。
    Removed,
    /// 元々表示していなかった（エラーではない）。
    NotShown,
}

/// effect ドライバが観測する要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayActionKind {
    /// OS へアイコン表示を依頼する。
    Show,
    /// OS へアイコン非表示を依頼する。
    Hide,
}

/// [`take_action`] の戻り値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayAction {
    /// [`finish_show`]/[`finish_hide`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 要求の種類。
    pub kind: TrayActionKind,
}

/// システムトレイ画面の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTrayState {
    /// 進行状態。
    pub phase: TrayPhase,
    /// 直近に立てた要求の世代番号。要求のたびに進み、完了通知が「今の要求」を
    /// 指しているかの照合に使う（`notification_send` と同型）。
    pub generation: u64,
    /// メニュー選択で切り替わる大きな表示文言。
    pub message: String,
    /// 操作結果の案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする。
    pub error: String,
}

impl Default for SystemTrayState {
    fn default() -> Self {
        Self {
            phase: TrayPhase::default(),
            generation: 0,
            message: INITIAL_MESSAGE.to_string(),
            status: String::new(),
            error: String::new(),
        }
    }
}

/// 「出す」ボタンのハンドラ。`Hidden` 以外では二重要求を防ぐため no-op
/// （画面側・effect ドライバ側二重の防御、`notification_send::notify` と同型）。
pub fn request_show(state: &mut SystemTrayState) {
    if state.phase != TrayPhase::Hidden {
        return;
    }
    state.phase = TrayPhase::ShowRequested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「消す」ボタンのハンドラ。`Shown` 以外では no-op。メニュー項目「消す」選択
/// （[`apply_selection`]）からも同じ経路で呼ばれる。
pub fn request_hide(state: &mut SystemTrayState) {
    if state.phase != TrayPhase::Shown {
        return;
    }
    state.phase = TrayPhase::HideRequested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
/// `Showing`/`Hiding`（既に [`mark_showing`]/[`mark_hiding`] 済みで処理中）では
/// `None` を返し、同じ要求を二重に dispatch しない（モジュール doc
/// 「`Showing`/`Hiding`」参照）。
pub fn take_action(state: &SystemTrayState) -> Option<TrayAction> {
    match state.phase {
        TrayPhase::ShowRequested => Some(TrayAction {
            generation: state.generation,
            kind: TrayActionKind::Show,
        }),
        TrayPhase::HideRequested => Some(TrayAction {
            generation: state.generation,
            kind: TrayActionKind::Hide,
        }),
        TrayPhase::Hidden | TrayPhase::Shown | TrayPhase::Showing | TrayPhase::Hiding => None,
    }
}

/// 表示要求を「処理中」（`Showing`）へ進める。`ShowRequested` 以外では何もしない
/// （`notification_send::mark_sending` と同型。effect ドライバが OS 呼び出し前に
/// 呼び、同じフレーム内で `take_action` が同じ要求を再び返さないようにする、
/// モジュール doc「`Showing`/`Hiding`」参照）。
pub fn mark_showing(state: &mut SystemTrayState) {
    if state.phase == TrayPhase::ShowRequested {
        state.phase = TrayPhase::Showing;
    }
}

/// 非表示要求を「処理中」（`Hiding`）へ進める。`HideRequested` 以外では何もしない。
pub fn mark_hiding(state: &mut SystemTrayState) {
    if state.phase == TrayPhase::HideRequested {
        state.phase = TrayPhase::Hiding;
    }
}

/// 表示結果を反映する。`generation` は [`take_action`] が返した世代番号。現在の
/// 世代と一致しない、または `Showing`（[`mark_showing`] 済み）以外で届いた完了は
/// 無視する（`notification_send::finish_notify` と同型の世代照合）。失敗時は
/// `Hidden` へ戻す（モジュール doc「失敗時にどちらへ戻すか」参照）。
pub fn finish_show(
    state: &mut SystemTrayState,
    generation: u64,
    result: Result<ShowResult, String>,
) {
    if state.phase != TrayPhase::Showing || state.generation != generation {
        return;
    }
    match result {
        Ok(ShowResult::Added) => {
            state.phase = TrayPhase::Shown;
            state.status = SHOWN_STATUS.to_string();
            state.error = String::new();
        }
        Ok(ShowResult::AlreadyShown) => {
            state.phase = TrayPhase::Shown;
            state.status = ALREADY_SHOWN_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.phase = TrayPhase::Hidden;
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 非表示結果を反映する。[`finish_show`] と対になる世代照合（`Hiding`、
/// [`mark_hiding`] 済み以外で届いた完了は無視する）。失敗時は `Shown` の
/// ままにする（モジュール doc「失敗時にどちらへ戻すか」参照）。
pub fn finish_hide(
    state: &mut SystemTrayState,
    generation: u64,
    result: Result<HideResult, String>,
) {
    if state.phase != TrayPhase::Hiding || state.generation != generation {
        return;
    }
    match result {
        Ok(HideResult::Removed) => {
            state.phase = TrayPhase::Hidden;
            state.status = HIDDEN_STATUS.to_string();
            state.error = String::new();
        }
        Ok(HideResult::NotShown) => {
            state.phase = TrayPhase::Hidden;
            state.status = NOT_SHOWN_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.phase = TrayPhase::Shown;
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// OS から届いたメニュー選択を反映する。`Shown` 以外では無視する
/// （effect ドライバが `SelectionQueue` から取り出した index をそのまま渡す契約、
/// 範囲検証は `fandhe_device::system_tray::SelectionQueue::push` 側で既に
/// 済んでいるが、ここでも未知の index は無視して壊れた表示にしない）。
pub fn apply_selection(state: &mut SystemTrayState, index: usize) {
    if state.phase != TrayPhase::Shown {
        return;
    }
    if index == HIDE_ITEM_INDEX {
        request_hide(state);
        return;
    }
    match index {
        0 => state.message = MORNING_MESSAGE.to_string(),
        1 => state.message = EVENING_MESSAGE.to_string(),
        _ => {}
    }
}

/// 大きな表示文言（`DynamicText` の `get`）。
pub fn message_text(state: &SystemTrayState) -> String {
    state.message.clone()
}

/// 案内文言（`DynamicText` の `get`）。処理中は固定の [`PROCESSING_STATUS`] を
/// 返す（`device-bridge-contracts.md`「4. ポーリング・再描画」の「失敗状態のまま
/// 再描画し続けない」とは別観点だが、処理中の二重操作防止と同じ理由で
/// ボタンを消し、ここで状態を明示する）。
pub fn status_text(state: &SystemTrayState) -> String {
    match state.phase {
        TrayPhase::ShowRequested
        | TrayPhase::Showing
        | TrayPhase::HideRequested
        | TrayPhase::Hiding => PROCESSING_STATUS.to_string(),
        TrayPhase::Hidden | TrayPhase::Shown => state.status.clone(),
    }
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &SystemTrayState) -> String {
    state.error.clone()
}

/// システムトレイ画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。`Hidden` では「出す」、`Shown` では
/// 「消す」ボタンのみを出し、処理中（`ShowRequested`/`HideRequested`）はどちらも
/// 外す（二重要求防止、`notification_send::current_screen` と同型）。
pub fn current_screen(state: &SystemTrayState) -> Screen<SystemTrayState> {
    let screen = Screen::new("システムトレイ")
        .dynamic_text(message_text)
        .dynamic_text(status_text)
        .error_text(error_text);
    match state.phase {
        TrayPhase::Hidden => screen.button(SHOW_BUTTON_ID, "出す", request_show),
        TrayPhase::Shown => screen.button(HIDE_BUTTON_ID, "消す", request_hide),
        TrayPhase::ShowRequested
        | TrayPhase::Showing
        | TrayPhase::HideRequested
        | TrayPhase::Hiding => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    #[test]
    fn request_show_advances_phase_and_generation() {
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        assert_eq!(state.phase, TrayPhase::ShowRequested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn request_show_is_noop_while_in_flight() {
        let mut state = SystemTrayState {
            phase: TrayPhase::ShowRequested,
            generation: 1,
            ..Default::default()
        };
        request_show(&mut state);
        assert_eq!(state.phase, TrayPhase::ShowRequested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn request_show_is_noop_while_shown() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        request_show(&mut state);
        assert_eq!(state.phase, TrayPhase::Shown);
        assert_eq!(state.generation, 0);
    }

    #[test]
    fn request_hide_advances_phase_and_generation_only_when_shown() {
        let mut state = SystemTrayState::default();
        request_hide(&mut state);
        assert_eq!(state.phase, TrayPhase::Hidden, "Hidden では no-op");
        assert_eq!(state.generation, 0);

        state.phase = TrayPhase::Shown;
        request_hide(&mut state);
        assert_eq!(state.phase, TrayPhase::HideRequested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn take_action_returns_show_or_hide_only_when_requested() {
        let mut state = SystemTrayState::default();
        assert_eq!(take_action(&state), None);

        request_show(&mut state);
        assert_eq!(
            take_action(&state),
            Some(TrayAction {
                generation: 1,
                kind: TrayActionKind::Show,
            })
        );

        state.phase = TrayPhase::Shown;
        assert_eq!(take_action(&state), None);

        request_hide(&mut state);
        assert_eq!(
            take_action(&state),
            Some(TrayAction {
                generation: 2,
                kind: TrayActionKind::Hide,
            })
        );
    }

    #[test]
    fn finish_show_added_transitions_to_shown() {
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        mark_showing(&mut state);
        finish_show(&mut state, 1, Ok(ShowResult::Added));
        assert_eq!(state.phase, TrayPhase::Shown);
        assert_eq!(state.status, SHOWN_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_show_already_shown_transitions_to_shown_without_error() {
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        mark_showing(&mut state);
        finish_show(&mut state, 1, Ok(ShowResult::AlreadyShown));
        assert_eq!(state.phase, TrayPhase::Shown);
        assert_eq!(state.status, ALREADY_SHOWN_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_show_failure_returns_to_hidden_with_error() {
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        mark_showing(&mut state);
        finish_show(&mut state, 1, Err("この環境では使えません".to_string()));
        assert_eq!(state.phase, TrayPhase::Hidden);
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn finish_show_ignores_stale_generation() {
        let mut state = SystemTrayState::default();
        request_show(&mut state); // generation = 1
        mark_showing(&mut state);
        finish_show(&mut state, 1, Err("失敗".to_string())); // Hidden に戻る

        request_show(&mut state); // generation = 2（まだ mark_showing していない）
        finish_show(&mut state, 1, Ok(ShowResult::Added));
        assert_eq!(
            state.phase,
            TrayPhase::ShowRequested,
            "古い世代の完了は無視され、現在の要求は進行中のまま"
        );
    }

    #[test]
    fn mark_showing_is_noop_outside_show_requested() {
        let mut state = SystemTrayState::default();
        mark_showing(&mut state); // Hidden では no-op
        assert_eq!(state.phase, TrayPhase::Hidden);
    }

    #[test]
    fn take_action_returns_none_after_marked_showing() {
        // effect ドライバが OS 呼び出し前に mark_showing すると、同じフレーム内で
        // 再度 take_action されても dispatch 済みの要求を返さない（モジュール doc
        // 「`Showing`/`Hiding`」の二重 dispatch 防止、codex-review 指摘参照）。
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        mark_showing(&mut state);
        assert_eq!(take_action(&state), None);
    }

    #[test]
    fn take_action_returns_none_after_marked_hiding() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        request_hide(&mut state);
        mark_hiding(&mut state);
        assert_eq!(take_action(&state), None);
    }

    #[test]
    fn fresh_state_instances_dispatch_generation_one_independently() {
        // 画面を作り直した相当: 複数の独立した `SystemTrayState` がそれぞれ
        // generation=1 から正しく dispatch できることを確認する回帰テスト
        // （codex-review 指摘、Issue #1039 是正: 以前はプロセス全体の
        // `static Mutex<u64>`（`fandhe-egui-proxy` 側）で二重 dispatch を
        // 防いでいたため、別インスタンスが同じ世代番号を使うと衝突していた。
        // 二重 dispatch 防止を `phase` 自身（`Showing`/`Hiding`）へ移したことで、
        // インスタンス間の世代の重複は問題にならない）。
        let mut state_a = SystemTrayState::default();
        request_show(&mut state_a);
        assert_eq!(
            take_action(&state_a),
            Some(TrayAction {
                generation: 1,
                kind: TrayActionKind::Show,
            })
        );

        let mut state_b = SystemTrayState::default();
        request_show(&mut state_b);
        assert_eq!(
            take_action(&state_b),
            Some(TrayAction {
                generation: 1,
                kind: TrayActionKind::Show,
            }),
            "別インスタンスは世代 1 が既に使われていても独立して dispatch できる"
        );
    }

    #[test]
    fn finish_show_ignores_completion_when_not_show_requested() {
        let mut state = SystemTrayState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_show(&mut state, 0, Ok(ShowResult::Added));
        assert_eq!(state.phase, TrayPhase::Hidden);
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn finish_hide_removed_transitions_to_hidden() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        request_hide(&mut state);
        mark_hiding(&mut state);
        finish_hide(&mut state, 1, Ok(HideResult::Removed));
        assert_eq!(state.phase, TrayPhase::Hidden);
        assert_eq!(state.status, HIDDEN_STATUS);
    }

    #[test]
    fn finish_hide_not_shown_transitions_to_hidden_without_error() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        request_hide(&mut state);
        mark_hiding(&mut state);
        finish_hide(&mut state, 1, Ok(HideResult::NotShown));
        assert_eq!(state.phase, TrayPhase::Hidden);
        assert_eq!(state.status, NOT_SHOWN_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_hide_failure_stays_shown_with_error() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        request_hide(&mut state);
        mark_hiding(&mut state);
        finish_hide(&mut state, 1, Err("失敗".to_string()));
        assert_eq!(
            state.phase,
            TrayPhase::Shown,
            "hide 失敗時はまだ表示中のまま扱う"
        );
        assert!(state.error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn apply_selection_updates_message_for_known_indices() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        apply_selection(&mut state, 0);
        assert_eq!(state.message, MORNING_MESSAGE);
        apply_selection(&mut state, 1);
        assert_eq!(state.message, EVENING_MESSAGE);
    }

    #[test]
    fn apply_selection_hide_index_routes_to_hide_requested() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        apply_selection(&mut state, HIDE_ITEM_INDEX);
        assert_eq!(state.phase, TrayPhase::HideRequested);
    }

    #[test]
    fn apply_selection_is_ignored_when_not_shown() {
        let mut state = SystemTrayState::default();
        apply_selection(&mut state, 0);
        assert_eq!(state.message, INITIAL_MESSAGE);
    }

    #[test]
    fn apply_selection_ignores_unknown_index() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        apply_selection(&mut state, 99);
        assert_eq!(state.message, INITIAL_MESSAGE);
    }

    #[test]
    fn status_text_shows_processing_while_in_flight() {
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        assert_eq!(status_text(&state), PROCESSING_STATUS);
    }

    #[test]
    fn show_button_is_hidden_while_processing_and_while_shown() {
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "出す", &mut state));

        let mut shown = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        let screen = current_screen(&shown);
        assert!(!click_button(&screen, "出す", &mut shown));
    }

    #[test]
    fn hide_button_is_shown_only_when_shown() {
        let mut state = SystemTrayState {
            phase: TrayPhase::Shown,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(click_button(&screen, "消す", &mut state));
        assert_eq!(state.phase, TrayPhase::HideRequested);
    }

    #[test]
    fn show_button_is_clickable_when_hidden() {
        let mut state = SystemTrayState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "出す", &mut state));
        assert_eq!(state.phase, TrayPhase::ShowRequested);
    }
}
