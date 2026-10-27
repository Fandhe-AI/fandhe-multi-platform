//! Phase 11（端末機能の拡充ループ、親 #193）型サンプル「最近使った項目への登録」（Issue #988）。
//! 一時フォルダーのサンプルファイルを OS の「最近使った項目」へ登録する / 一覧に載っているかを
//! 確かめる 2 ボタンだけの 1 画面。`FANDHE_PROXY_APP=recent_documents_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod recent_documents_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。サンプルの用意・パスの検査・OS 呼び出しは `fandhe-egui-proxy` の
//! effect ドライバ（`device/recent_documents.rs`）が [`take_request`] で要求を観測して行い、
//! [`finish_action`] で結果を注入する。本画面は二重押し防止と文言だけを持つ。OS 別の表示差は無い。
//!
//! # 結果を混同しない理由
//!
//! 登録 API は成否を返さない。「OS へ渡した」だけを「載りました」と言うと、設定で無効なときや
//! 反映待ちのときに利用者が誤解する。一覧を読み戻して載っていたときだけ `Listed`、載っていない
//! ときは `NotListed`（成功の色にも失敗の色にもしない）、読み戻せなかったときは失敗にする。
//! 世代番号（`generation`）で古い完了や `Idle` で届いた完了を無視する。

use crate::{Screen, WidgetId};

/// サンプルを置く一時フォルダー内のフォルダー名。
pub const SAMPLE_DIR: &str = "fandhe-recent-documents";
/// サンプルのファイル名。
pub const SAMPLE_FILE: &str = "sample.txt";
/// サンプルの中身（ASCII のみ）。
pub const SAMPLE_CONTENT: &str = "fandhe recent-documents sample\n";

/// 一覧に載っていたときの案内。
pub const LISTED_STATUS: &str = "一覧に載りました";
/// 登録したが一覧に載っていないときの案内。
pub const NOT_LISTED_AFTER_REGISTER_STATUS: &str =
    "OS へ渡しましたが、一覧には載っていません（設定で無効・反映待ちの可能性）";
/// 確かめただけで一覧に載っていないときの案内。
pub const NOT_LISTED_STATUS: &str = "一覧には載っていません（設定で無効・反映待ちの可能性）";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では使えません";
/// サンプルを用意できなかった。
pub const SAMPLE_UNAVAILABLE_ERROR: &str =
    "確かめられませんでした: サンプルファイルを用意できませんでした";
/// 読み戻しの失敗。
pub const CHECK_FAILED_ERROR: &str = "確かめられませんでした";

const NOTE_TEXT: &str = "一時フォルダーの fandhe-recent-documents/sample.txt だけを扱います";
const REGISTER_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const CHECK_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// ボタンに対応する操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentAction {
    /// 登録して読み戻す。
    Register,
    /// 読み戻すだけ。
    Check,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecentPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_in_flight`] 済み）。
    InFlight,
}

/// OS が判定した結果（`RecentOutcome` に非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 一覧に載っている。
    Listed,
    /// 一覧に載っていない。
    NotListed,
    /// この環境では非対応。
    Unsupported,
}

/// 失敗の種類（画面には固定文言だけを出す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentFailure {
    /// サンプルを用意できない・検査に通らない。OS は呼ばない。
    SampleUnavailable,
    /// 読み戻しの失敗・その他。
    Failed,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentRequestView {
    /// [`finish_action`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 操作。
    pub action: RecentAction,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecentDocumentsState {
    /// 進行状態。
    pub phase: RecentPhase,
    /// 直近に押された操作。
    pub action: Option<RecentAction>,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

fn request(state: &mut RecentDocumentsState, action: RecentAction) {
    if state.phase != RecentPhase::Idle {
        return;
    }
    state.phase = RecentPhase::Requested;
    state.action = Some(action);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「登録する」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn register_sample(state: &mut RecentDocumentsState) {
    request(state, RecentAction::Register);
}

/// 「確かめる」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn check_listed(state: &mut RecentDocumentsState) {
    request(state, RecentAction::Check);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &RecentDocumentsState) -> Option<RecentRequestView> {
    if state.phase != RecentPhase::Requested {
        return None;
    }
    Some(RecentRequestView {
        generation: state.generation,
        action: state.action?,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut RecentDocumentsState) {
    if state.phase == RecentPhase::Requested {
        state.phase = RecentPhase::InFlight;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_action(
    state: &mut RecentDocumentsState,
    generation: u64,
    result: Result<CoreOutcome, RecentFailure>,
) {
    if state.phase == RecentPhase::Idle || state.generation != generation {
        return;
    }
    let action = state.action;
    state.phase = RecentPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::Listed) => state.status = LISTED_STATUS.to_string(),
        Ok(CoreOutcome::NotListed) => {
            state.status = match action {
                Some(RecentAction::Register) => NOT_LISTED_AFTER_REGISTER_STATUS,
                _ => NOT_LISTED_STATUS,
            }
            .to_string();
        }
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(RecentFailure::SampleUnavailable) => {
            state.error = SAMPLE_UNAVAILABLE_ERROR.to_string();
        }
        Err(RecentFailure::Failed) => state.error = CHECK_FAILED_ERROR.to_string(),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &RecentDocumentsState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &RecentDocumentsState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外し、同時に 2 件出さない。
pub fn current_screen(state: &RecentDocumentsState) -> Screen<RecentDocumentsState> {
    let screen = Screen::new("最近使った項目")
        .text(NOTE_TEXT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != RecentPhase::Idle {
        return screen;
    }
    screen
        .button(
            REGISTER_BUTTON_ID,
            "サンプルを最近使った項目に登録する",
            register_sample,
        )
        .button(CHECK_BUTTON_ID, "一覧に載っているか確かめる", check_listed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_flight(action: RecentAction) -> RecentDocumentsState {
        let mut s = RecentDocumentsState::default();
        request(&mut s, action);
        mark_in_flight(&mut s);
        s
    }

    fn shown(action: RecentAction, r: Result<CoreOutcome, RecentFailure>) -> (String, String) {
        let mut s = in_flight(action);
        let g = s.generation;
        finish_action(&mut s, g, r);
        assert_eq!(s.phase, RecentPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn all_results_are_distinct() {
        let all = [
            shown(RecentAction::Register, Ok(CoreOutcome::Listed)),
            shown(RecentAction::Register, Ok(CoreOutcome::NotListed)),
            shown(RecentAction::Check, Ok(CoreOutcome::NotListed)),
            shown(RecentAction::Check, Ok(CoreOutcome::Unsupported)),
            shown(RecentAction::Check, Err(RecentFailure::SampleUnavailable)),
            shown(RecentAction::Check, Err(RecentFailure::Failed)),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn only_listed_says_listed() {
        for r in [
            Ok(CoreOutcome::NotListed),
            Ok(CoreOutcome::Unsupported),
            Err(RecentFailure::SampleUnavailable),
            Err(RecentFailure::Failed),
        ] {
            let (status, _) = shown(RecentAction::Register, r);
            assert_ne!(status, LISTED_STATUS);
        }
        let (status, error) = shown(RecentAction::Check, Ok(CoreOutcome::NotListed));
        assert_eq!(status, NOT_LISTED_STATUS);
        assert!(error.is_empty() && !status.contains("OS へ渡しました"));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = in_flight(RecentAction::Check);
        let g = s.generation + 1;
        finish_action(&mut s, g, Ok(CoreOutcome::Listed));
        assert_eq!(s.phase, RecentPhase::InFlight);
        assert!(s.status.is_empty());

        let mut idle = RecentDocumentsState::default();
        finish_action(&mut idle, 0, Ok(CoreOutcome::Listed));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn request_is_noop_while_busy_and_take_request_does_not_change_state() {
        let mut s = RecentDocumentsState::default();
        assert!(take_request(&s).is_none());
        check_listed(&mut s);
        let r = take_request(&s).unwrap();
        assert_eq!(r.action, RecentAction::Check);
        assert_eq!(s.phase, RecentPhase::Requested);
        register_sample(&mut s);
        assert_eq!(s.action, Some(RecentAction::Check));
        mark_in_flight(&mut s);
        assert!(take_request(&s).is_none());
        register_sample(&mut s);
        assert_eq!(s.phase, RecentPhase::InFlight);
        assert_eq!(s.generation, r.generation);
    }

    #[test]
    fn buttons_hidden_while_busy() {
        let idle = format!("{:?}", current_screen(&RecentDocumentsState::default()));
        let busy = format!("{:?}", current_screen(&in_flight(RecentAction::Register)));
        assert!(idle.contains("一覧に載っているか確かめる"));
        assert!(!busy.contains("一覧に載っているか確かめる"));
    }
}
