//! Phase 11（端末機能の拡充ループ、親 #896）型サンプル「共有」（Issue #897）。
//! テキスト欄 + URL 欄 + 「共有」ボタン → OS の共有シートが開き、結果（共有した/
//! キャンセル/共有先なし/失敗）を表示する 1 画面。`FANDHE_PROXY_APP=share_text` で
//! 選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から
//! 呼ばれる（`screens/mod.rs` の `pub mod share_text;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の共有シート提示・入力検証は
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、`device/share.rs`）が
//! [`take_request`] で要求を観測し [`finish_share`] で結果を注入する。
//!
//! # 成功・キャンセル・共有先なし・失敗を混同しない理由
//!
//! `file_pick`/`tts_speak` と同じ再発防止方針: キャンセルや共有先なしを黙って
//! 「失敗しました」に丸めると、利用者は自分の共有が本当に失敗したのか単に閉じた/
//! 共有先が無かっただけなのか区別できない。本画面は成功・キャンセル・共有先なしを
//! `status`（通常色）、失敗を `error`（`error_text` で `TextRole::Error`）の
//! 別フィールドへ書き分ける（`file_pick::FilePickState` と同型）。
//!
//! ```text
//! Idle --「共有」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_sharing--> phase=Sharing
//!   （OS の共有 UI が開く。利用者が閉じる/選ぶまで戻らない）
//!   --finish_share（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```
//!
//! 世代番号（`generation`）は [`take_request`]/[`finish_share`] の照合に使い、
//! `Idle` へ戻った後や新しい要求で上書きされた古い完了を無視する
//! （`file_pick::PickPhase` と同じ判断）。

use crate::{Screen, WidgetId};

/// 「共有」ボタンの `WidgetId`。
const SHARE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 入力欄が空のまま「共有」を押したときの案内。
pub const EMPTY_INPUT_STATUS: &str = "共有する文字列を入力してください";
/// 共有できたときの案内。
pub const SHARED_STATUS: &str = "共有しました";
/// キャンセルされたときの案内（失敗ではない、モジュール doc 参照）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 共有先が無かったときの案内（失敗ではない、モジュール doc 参照）。
pub const NO_TARGET_STATUS: &str = "共有先がありません";
/// 結果不明だったときの案内（失敗ではない。Android の `Intent.createChooser` が
/// 選択完了を通知できなかったケースで使う、Issue #899）。「共有しました」を含めない
/// （#859 型の事故防止: 共有できたと誤解させる文言を混ぜない）。
pub const UNKNOWN_STATUS: &str = "共有シートを閉じました（共有したかは確認できません）";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 共有操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SharePhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが共有 UI を開いて処理中（[`mark_sharing`] 済み）。
    Sharing,
}

/// 共有結果（画面表示用。`fandhe_device::share::ShareOutcome` と 3 腕対応する。
/// `fandhe-core` は `fandhe-device` に依存しないため、写像は `fandhe-egui-proxy`
/// 側の責務にする、モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareOutcome {
    /// 利用者が共有先を選んだ。
    Shared,
    /// 利用者が共有 UI を閉じるだけで何も選ばなかった。
    Cancelled,
    /// 共有できる先が端末に 1 つも無い。
    NoTarget,
    /// 共有 UI は閉じたが、共有したかどうかを判定できない（Android のみ発生し得る、
    /// Issue #899）。
    Unknown,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareTextRequest {
    /// [`finish_share`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 共有するテキスト（trim 前。検証・trim は effect ドライバ側の
    /// `fandhe_device::share::ShareRequest::new` に一本化する、SSOT 方針）。
    pub text: String,
    /// 共有する URL（空文字列は「URL 無し」として effect ドライバ側が `None` 扱いする）。
    pub url: Option<String>,
}

/// 共有画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShareTextState {
    /// テキスト欄の入力値。
    pub input: String,
    /// URL 欄の入力値（空文字列なら「URL 無し」）。
    pub url: String,
    /// 共有操作の進行状態。
    pub phase: SharePhase,
    /// 直近に立てた要求の世代番号。要求のたびに進み、完了通知が「今の要求」を
    /// 指しているかの照合に使う（モジュール doc 参照）。
    pub generation: u64,
    /// 成功・キャンセル・共有先なしの案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空文字列にする
    /// （成功と失敗を取り違えないための分離、モジュール doc 参照）。
    pub error: String,
}

/// テキスト欄の表示値（`Screen::text_field` の getter）。
pub fn get_input(state: &ShareTextState) -> String {
    state.input.clone()
}

/// テキスト欄の入力を反映する（`Screen::text_field` の setter）。
pub fn set_input(state: &mut ShareTextState, value: String) {
    state.input = value;
}

/// URL 欄の表示値（`Screen::text_field` の getter）。
pub fn get_url(state: &ShareTextState) -> String {
    state.url.clone()
}

/// URL 欄の入力を反映する（`Screen::text_field` の setter）。
pub fn set_url(state: &mut ShareTextState, value: String) {
    state.url = value;
}

/// 「共有」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。空白のみの入力は要求を立てず
/// 案内だけ出す（`clipboard_text::copy` と同型）。
pub fn share(state: &mut ShareTextState) {
    if state.phase != SharePhase::Idle {
        return;
    }
    if state.input.trim().is_empty() {
        state.status = EMPTY_INPUT_STATUS.to_string();
        // 前回の共有失敗エラーを残さない（Bugbot 指摘: 空入力パスでの error 取り残し）。
        state.error = String::new();
        return;
    }
    state.phase = SharePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果表示（status/error）を残さない（codex-review 指摘: 再共有中に
    // 前回の「共有しました」やエラーが新しい操作の前に表示され続ける）。
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない。
/// 実際に `Sharing` へ進めるのは [`mark_sharing`]）。
pub fn take_request(state: &ShareTextState) -> Option<ShareTextRequest> {
    (state.phase == SharePhase::Requested).then(|| ShareTextRequest {
        generation: state.generation,
        text: state.input.clone(),
        url: (!state.url.is_empty()).then(|| state.url.clone()),
    })
}

/// 要求を共有 UI 表示中（`Sharing`）へ進める。`Requested` 以外では何もしない
/// （`take_request` が `None` を返した後に誤って呼ばれても状態を壊さない）。
pub fn mark_sharing(state: &mut ShareTextState) {
    if state.phase == SharePhase::Requested {
        state.phase = SharePhase::Sharing;
    }
}

/// 共有結果を反映する。`generation` は [`take_request`] が返した世代番号（呼び出し元が
/// 共有 UI 表示中も保持し続ける契約）。現在の世代と一致しない（＝新しい要求で
/// 既に上書きされた）完了は無視する。`Idle`（要求が無い）状態で届いた完了も無視する。
pub fn finish_share(
    state: &mut ShareTextState,
    generation: u64,
    result: Result<ShareOutcome, String>,
) {
    if state.phase == SharePhase::Idle || state.generation != generation {
        return;
    }
    state.phase = SharePhase::Idle;
    match result {
        Ok(ShareOutcome::Shared) => {
            state.status = SHARED_STATUS.to_string();
            state.error = String::new();
        }
        Ok(ShareOutcome::Cancelled) => {
            state.status = CANCELLED_STATUS.to_string();
            state.error = String::new();
        }
        Ok(ShareOutcome::NoTarget) => {
            state.status = NO_TARGET_STATUS.to_string();
            state.error = String::new();
        }
        Ok(ShareOutcome::Unknown) => {
            state.status = UNKNOWN_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &ShareTextState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &ShareTextState) -> String {
    state.error.clone()
}

/// 共有画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) -> Screen<S>` として
/// 呼ばれる契約。`Idle` のときのみボタンを出し、処理中（`Requested`/`Sharing`）は
/// 二重押しを防ぐため外す（`file_pick::current_screen` と同型）。
pub fn current_screen(state: &ShareTextState) -> Screen<ShareTextState> {
    let screen = Screen::new("共有")
        .text_field("共有する文字列", get_input, set_input)
        .text_field("URL（任意）", get_url, set_url)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != SharePhase::Idle {
        return screen;
    }
    screen.button(SHARE_BUTTON_ID, "共有", share)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_advances_generation_and_phase() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state);
        assert_eq!(state.phase, SharePhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn share_rejects_blank_input_without_advancing() {
        let mut state = ShareTextState {
            input: "   ".to_string(),
            ..Default::default()
        };
        share(&mut state);
        assert_eq!(state.phase, SharePhase::Idle);
        assert_eq!(state.generation, 0);
        assert_eq!(state.status, EMPTY_INPUT_STATUS);
    }

    #[test]
    fn share_rejects_blank_input_clears_previous_error() {
        // 直前の共有失敗エラーが、空入力での再押下後まで残らないことを確認する
        // （Bugbot 指摘: 空入力パスで error が取り残される）。
        let mut state = ShareTextState {
            input: "   ".to_string(),
            error: format!("{FAILED_PREFIX}前回の失敗"),
            ..Default::default()
        };
        share(&mut state);
        assert_eq!(state.status, EMPTY_INPUT_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn share_clears_previous_status_and_error_on_new_request() {
        // codex-review 指摘: 成功後に入力を変えて再共有すると前回の「共有しました」が
        // 新しい要求中も表示され続ける。失敗後の再共有でも前回エラーが残らないことを確認する。
        let mut state = ShareTextState {
            input: "hello again".to_string(),
            status: SHARED_STATUS.to_string(),
            error: format!("{FAILED_PREFIX}前回の失敗"),
            ..Default::default()
        };
        share(&mut state);
        assert_eq!(state.phase, SharePhase::Requested);
        assert_eq!(state.status, "");
        assert_eq!(state.error, "");
    }

    #[test]
    fn share_is_noop_while_in_flight() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            phase: SharePhase::Sharing,
            generation: 1,
            ..Default::default()
        };
        share(&mut state);
        assert_eq!(state.phase, SharePhase::Sharing);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn take_request_returns_generation_text_and_url_only_when_requested() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            url: "https://example.com".to_string(),
            ..Default::default()
        };
        assert_eq!(take_request(&state), None);
        share(&mut state);
        assert_eq!(
            take_request(&state),
            Some(ShareTextRequest {
                generation: 1,
                text: "hello".to_string(),
                url: Some("https://example.com".to_string()),
            })
        );
        mark_sharing(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn take_request_maps_empty_url_to_none() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state);
        assert_eq!(take_request(&state).unwrap().url, None);
    }

    #[test]
    fn finish_share_shared_sets_status_without_error() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state);
        mark_sharing(&mut state);
        finish_share(&mut state, 1, Ok(ShareOutcome::Shared));
        assert_eq!(state.phase, SharePhase::Idle);
        assert_eq!(state.status, SHARED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_share_cancelled_does_not_say_shared() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state);
        mark_sharing(&mut state);
        finish_share(&mut state, 1, Ok(ShareOutcome::Cancelled));
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_ne!(state.status, SHARED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_share_no_target_does_not_say_shared() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state);
        mark_sharing(&mut state);
        finish_share(&mut state, 1, Ok(ShareOutcome::NoTarget));
        assert_eq!(state.status, NO_TARGET_STATUS);
        assert_ne!(state.status, SHARED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_share_unknown_sets_unknown_status_without_success_wording() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state);
        mark_sharing(&mut state);
        finish_share(&mut state, 1, Ok(ShareOutcome::Unknown));
        assert_eq!(state.status, UNKNOWN_STATUS);
        assert_ne!(state.status, SHARED_STATUS);
        assert!(!state.status.contains("共有しました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_share_error_sets_error_without_success_status() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state);
        mark_sharing(&mut state);
        finish_share(
            &mut state,
            1,
            Err("この環境では共有シートを使えません".to_string()),
        );
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
        assert_ne!(state.status, SHARED_STATUS);
    }

    #[test]
    fn finish_share_ignores_stale_generation() {
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        share(&mut state); // generation = 1
        mark_sharing(&mut state);
        finish_share(&mut state, 1, Ok(ShareOutcome::Cancelled)); // Idle に戻る

        share(&mut state); // generation = 2（新しい要求で前回の status はクリアされる）
        finish_share(&mut state, 1, Ok(ShareOutcome::Shared));
        // 古い世代の完了は無視され、Requested のまま
        assert_eq!(state.phase, SharePhase::Requested);
        assert_eq!(state.status, "");
    }

    #[test]
    fn finish_share_ignores_completion_when_idle() {
        let mut state = ShareTextState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_share(&mut state, 0, Ok(ShareOutcome::Shared));
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = ShareTextState {
            phase: SharePhase::Sharing,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "共有", &mut state));
    }

    #[test]
    fn button_is_clickable_when_idle() {
        use crate::click_button;
        let mut state = ShareTextState {
            input: "hello".to_string(),
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(click_button(&screen, "共有", &mut state));
        assert_eq!(state.phase, SharePhase::Requested);
    }
}
