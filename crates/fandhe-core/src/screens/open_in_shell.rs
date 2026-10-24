//! Phase 11（端末機能の拡充ループ、親 #146）型サンプル「既定アプリで開く・ファイルマネージャーで
//! 表示」（Issue #749）。一時フォルダーのサンプルファイルを開く / Finder・Explorer で表示する
//! 2 ボタンだけの 1 画面。`FANDHE_PROXY_APP=open_in_shell` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod open_in_shell;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。サンプルファイルの用意・パスの検査・OS 呼び出しは `fandhe-egui-proxy`
//! の effect ドライバ（`device/open_in_shell.rs`）が [`take_request`] で要求を観測して行い、
//! [`finish_action`] で結果を注入する。本画面は二重押し防止と文言、OS 別の表示（[`Platform`]、
//! ドライバが一度だけ注入）だけを持つ。パスの入力欄は置かない。
//!
//! # 結果を混同しない理由
//!
//! 「OS に渡しただけ」「扱うアプリが無い」「非対応」を黙って「開きました」に丸めると、利用者は
//! 表示されたと誤解する。どの文言もアプリ・ウィンドウの表示完了を言わない。OS が判定した正常な
//! 結果は `status`、失敗は `error`（`TextRole::Error`）へ書き分ける。世代番号（`generation`）で
//! 古い完了や `Idle` で届いた完了を無視する（`compose_mail_sms` と同じ判断）。

use crate::{Screen, WidgetId};

/// サンプルを置く一時フォルダー内のフォルダー名。
pub const SAMPLE_DIR: &str = "fandhe-open-in-shell";
/// サンプルのファイル名。
pub const SAMPLE_FILE: &str = "sample.txt";
/// サンプルの中身（ASCII のみ）。
pub const SAMPLE_CONTENT: &str = "fandhe open-in-shell sample\n";

/// OS に渡したときの案内（アプリ・ウィンドウの表示は言わない）。
pub const HANDED_OFF_STATUS: &str =
    "OS に渡しました（アプリ・ウィンドウが開いたかは確認できません）";
/// 扱うアプリが無いときの案内。
pub const NO_HANDLER_ERROR: &str = "開けませんでした: 開けるアプリがありません";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では使えません";
/// パス不正。
pub const INVALID_PATH_ERROR: &str = "開けませんでした: ファイルの場所が正しくありません";
/// 実行できるファイル・アプリなどの拒否。
pub const REJECTED_TYPE_ERROR: &str = "開けませんでした: 実行できるファイル・アプリは開きません";
/// サンプルファイルを用意できなかった。
pub const SAMPLE_UNAVAILABLE_ERROR: &str =
    "開けませんでした: サンプルファイルを用意できませんでした";
/// OS の失敗。
pub const OS_FAILED_ERROR: &str = "開けませんでした: OS が受け付けませんでした";

const NOTE_TEXT: &str = "一時フォルダーの fandhe-open-in-shell/sample.txt だけを扱います";
const OPEN_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const REVEAL_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 文言を出し分ける OS（ドライバが一度だけ注入する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Platform {
    /// 未注入・その他。
    #[default]
    Unknown,
    /// macOS。
    MacOs,
    /// Windows。
    Windows,
}

/// ボタンに対応する操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAction {
    /// 既定のアプリで開く。
    Open,
    /// ファイルマネージャーで選択表示する。
    Reveal,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_in_flight`] 済み）。
    InFlight,
}

/// OS が判定した結果（`ShellOutcome` に非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// OS に渡した。
    HandedOff,
    /// 扱うアプリが無い。
    NoHandler,
    /// この環境では非対応。
    Unsupported,
}

/// 失敗の種類（画面には固定文言だけを出す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFailure {
    /// パス不正。
    InvalidPath,
    /// 実行できるファイル・アプリ・ディレクトリなど、開かない型。
    RejectedType,
    /// サンプルファイルを用意できなかった。
    SampleUnavailable,
    /// OS の失敗・その他。
    Failed,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellRequestView {
    /// [`finish_action`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 操作。
    pub action: ShellAction,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenInShellState {
    /// 進行状態。
    pub phase: ShellPhase,
    /// 直近に押された操作。
    pub action: Option<ShellAction>,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 注入済みの OS。
    pub platform: Platform,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

/// この OS を反映する（ドライバが一度だけ送る）。
pub fn apply_platform(state: &mut OpenInShellState, platform: Platform) {
    state.platform = platform;
}

fn request(state: &mut OpenInShellState, action: ShellAction) {
    if state.phase != ShellPhase::Idle {
        return;
    }
    state.phase = ShellPhase::Requested;
    state.action = Some(action);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「既定のアプリで開く」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn open_default(state: &mut OpenInShellState) {
    request(state, ShellAction::Open);
}

/// 「Finder / Explorer で表示」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn reveal_in_manager(state: &mut OpenInShellState) {
    request(state, ShellAction::Reveal);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &OpenInShellState) -> Option<ShellRequestView> {
    if state.phase != ShellPhase::Requested {
        return None;
    }
    Some(ShellRequestView {
        generation: state.generation,
        action: state.action?,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut OpenInShellState) {
    if state.phase == ShellPhase::Requested {
        state.phase = ShellPhase::InFlight;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_action(
    state: &mut OpenInShellState,
    generation: u64,
    result: Result<CoreOutcome, ShellFailure>,
) {
    if state.phase == ShellPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ShellPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::HandedOff) => state.status = HANDED_OFF_STATUS.to_string(),
        Ok(CoreOutcome::NoHandler) => state.error = NO_HANDLER_ERROR.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(ShellFailure::InvalidPath) => state.error = INVALID_PATH_ERROR.to_string(),
        Err(ShellFailure::RejectedType) => state.error = REJECTED_TYPE_ERROR.to_string(),
        Err(ShellFailure::SampleUnavailable) => {
            state.error = SAMPLE_UNAVAILABLE_ERROR.to_string();
        }
        Err(ShellFailure::Failed) => state.error = OS_FAILED_ERROR.to_string(),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &OpenInShellState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &OpenInShellState) -> String {
    state.error.clone()
}

/// 選択表示ボタンの名前（OS ごとのファイルマネージャー名）。
pub fn reveal_label(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "Finder で表示",
        Platform::Windows => "Explorer で表示",
        Platform::Unknown => "ファイルの場所を表示",
    }
}

/// 画面タイトル（OS ごとのファイルマネージャー名を含む）。
pub fn title(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "既定アプリで開く・Finder で表示",
        Platform::Windows => "既定アプリで開く・Explorer で表示",
        Platform::Unknown => "既定アプリで開く・ファイルの場所を表示",
    }
}

/// 画面を組み立てる。処理中はボタンを外し、同時に 2 件出さない。
pub fn current_screen(state: &OpenInShellState) -> Screen<OpenInShellState> {
    let screen = Screen::new(title(state.platform))
        .text(NOTE_TEXT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != ShellPhase::Idle {
        return screen;
    }
    screen
        .button(OPEN_BUTTON_ID, "既定のアプリで開く", open_default)
        .button(
            REVEAL_BUTTON_ID,
            reveal_label(state.platform),
            reveal_in_manager,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_flight(action: ShellAction) -> OpenInShellState {
        let mut s = OpenInShellState::default();
        request(&mut s, action);
        mark_in_flight(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, ShellFailure>) -> (String, String) {
        let mut s = in_flight(ShellAction::Open);
        let g = s.generation;
        finish_action(&mut s, g, r);
        assert_eq!(s.phase, ShellPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn all_results_are_distinct_and_none_claims_opened() {
        let all = [
            shown(Ok(CoreOutcome::HandedOff)),
            shown(Ok(CoreOutcome::NoHandler)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err(ShellFailure::InvalidPath)),
            shown(Err(ShellFailure::RejectedType)),
            shown(Err(ShellFailure::SampleUnavailable)),
            shown(Err(ShellFailure::Failed)),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let text = format!("{}{}", a.0, a.1);
            assert!(!text.contains("開きました") && !text.contains("表示しました"));
        }
    }

    #[test]
    fn no_handler_and_unsupported_are_not_handed_off() {
        let (status, error) = shown(Ok(CoreOutcome::NoHandler));
        assert_ne!(status, HANDED_OFF_STATUS);
        assert_eq!(error, NO_HANDLER_ERROR);
        let (status, error) = shown(Ok(CoreOutcome::Unsupported));
        assert_eq!(status, UNSUPPORTED_STATUS);
        assert!(error.is_empty());
        assert_ne!(status, HANDED_OFF_STATUS);
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = in_flight(ShellAction::Reveal);
        let g = s.generation + 1;
        finish_action(&mut s, g, Ok(CoreOutcome::HandedOff));
        assert_eq!(s.phase, ShellPhase::InFlight);
        assert!(s.status.is_empty());

        let mut idle = OpenInShellState::default();
        finish_action(&mut idle, 0, Ok(CoreOutcome::HandedOff));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn request_is_noop_while_busy_and_take_request_does_not_change_state() {
        let mut s = OpenInShellState::default();
        assert!(take_request(&s).is_none());
        reveal_in_manager(&mut s);
        let r = take_request(&s).unwrap();
        assert_eq!(r.action, ShellAction::Reveal);
        assert_eq!(s.phase, ShellPhase::Requested);
        open_default(&mut s);
        assert_eq!(s.action, Some(ShellAction::Reveal));
        mark_in_flight(&mut s);
        assert!(take_request(&s).is_none());
        open_default(&mut s);
        assert_eq!(s.phase, ShellPhase::InFlight);
        assert_eq!(s.generation, r.generation);
    }

    #[test]
    fn buttons_hidden_while_busy() {
        let idle = format!("{:?}", current_screen(&OpenInShellState::default()));
        let busy = format!("{:?}", current_screen(&in_flight(ShellAction::Open)));
        assert!(idle.contains("既定のアプリで開く"));
        assert!(!busy.contains("既定のアプリで開く"));
    }

    #[test]
    fn labels_follow_platform() {
        let mut s = OpenInShellState::default();
        apply_platform(&mut s, Platform::MacOs);
        let mac = format!("{:?}", current_screen(&s));
        assert!(mac.contains("Finder で表示") && !mac.contains("Explorer"));
        apply_platform(&mut s, Platform::Windows);
        let win = format!("{:?}", current_screen(&s));
        assert!(win.contains("Explorer で表示") && !win.contains("Finder"));
    }
}
