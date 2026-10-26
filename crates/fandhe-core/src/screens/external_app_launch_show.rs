//! Phase 11（端末機能の拡充ループ、親 #176）型サンプル「電話発信・地図など他アプリの起動」（Issue #861）。
//! 固定の電話・固定の地点（東京駅）を OS に渡す 2 ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=external_app_launch_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/
//! registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod external_app_launch_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS 呼び出しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/external_app_launch.rs`）が [`take_request`] で要求を観測して行い、[`finish_launch`] で
//! 結果を注入する。本画面は二重押し防止と文言だけを持ち、全対象 OS（macOS・Windows・iOS・Android・Web）の
//! 文言をここで決める（後続の OS 実装は画面を触らない）。
//!
//! # 結果を混同しない理由
//!
//! 「OS に渡しただけ」「開けるアプリが無い」「取り消した」「この OS では開けない」「非対応」を黙って
//! 「開きました」「発信しました」に丸めると、利用者は発信した・地図が出たと誤解する。どの文言も
//! 開いた・発信したとは言わない。OS が判定した結果は `status`、内部失敗は `error`
//! （`TextRole::Error`）へ書き分ける。世代番号（`generation`）で古い完了や `Idle` で届いた完了を
//! 無視する（`open_system_settings_show` と同じ判断）。

use crate::{Screen, WidgetId};

/// OS に渡しただけのときの案内（開いた・発信したとは言わない）。
pub const HANDED_OFF_STATUS: &str =
    "OS に渡しました（相手のアプリが開いたか・発信したかは確認できません）";
/// 開けるアプリが無いときの案内（失敗扱いにしない）。
pub const NO_HANDLER_STATUS: &str = "開けるアプリが見つかりませんでした";
/// 利用者が取り消したときの案内（Windows で起こる。失敗扱いにしない）。
pub const CANCELLED_STATUS: &str = "取り消されました";
/// この OS では開けない操作のときの案内（Windows の地図で起こる）。
pub const TARGET_UNAVAILABLE_STATUS: &str = "この OS ではこの操作を開けません";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では他のアプリを開けません";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 2 つの操作の説明。固定値であること、OS ごとの差を添える。
pub const NOTE_TEXT: &str = "固定の電話番号と固定の地点（東京駅）を OS に渡します。Windows は電話のみで、地図は開けません。Web はブラウザに渡したことしか分からず、開けるアプリが無くても「OS に渡しました」と出ます";
/// 「電話」ボタンの文言。
pub const PHONE_BUTTON_LABEL: &str = "電話アプリへ番号を渡す";
/// 「地図」ボタンの文言。
pub const MAP_BUTTON_LABEL: &str = "地図アプリへ東京駅を渡す";

const PHONE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const MAP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 起動する操作（`fandhe_device::external_app_launch::LaunchTarget` に対応。写像は proxy 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// 電話。
    Phone,
    /// 地図。
    Map,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExternalLaunchPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_launching`] 済み）。
    Launching,
}

/// OS が判定した結果（`LaunchOutcome` に非対応を加えたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// OS に渡しただけ。
    HandedOff,
    /// 開けるアプリが無い。
    NoHandler,
    /// 利用者が取り消した。
    Cancelled,
    /// この OS では開けない操作。
    TargetUnavailable,
    /// この環境では非対応。
    Unsupported,
}

/// 失敗（画面には固定文言だけを出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalLaunchFailure {
    /// 内部失敗。
    Failed(String),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalLaunchRequestView {
    /// [`finish_launch`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 起動する操作。
    pub kind: LaunchKind,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExternalAppLaunchState {
    /// 進行状態。
    pub phase: ExternalLaunchPhase,
    /// 直近に押された操作。
    pub kind: Option<LaunchKind>,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

fn request(state: &mut ExternalAppLaunchState, kind: LaunchKind) {
    if state.phase != ExternalLaunchPhase::Idle {
        return;
    }
    state.phase = ExternalLaunchPhase::Requested;
    state.kind = Some(kind);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「電話」ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn open_phone(state: &mut ExternalAppLaunchState) {
    request(state, LaunchKind::Phone);
}

/// 「地図」ボタンのハンドラ。`Idle` 以外は no-op。
pub fn open_map(state: &mut ExternalAppLaunchState) {
    request(state, LaunchKind::Map);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &ExternalAppLaunchState) -> Option<ExternalLaunchRequestView> {
    if state.phase != ExternalLaunchPhase::Requested {
        return None;
    }
    Some(ExternalLaunchRequestView {
        generation: state.generation,
        kind: state.kind?,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_launching(state: &mut ExternalAppLaunchState) {
    if state.phase == ExternalLaunchPhase::Requested {
        state.phase = ExternalLaunchPhase::Launching;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish_launch(
    state: &mut ExternalAppLaunchState,
    generation: u64,
    result: Result<CoreOutcome, ExternalLaunchFailure>,
) {
    if state.phase == ExternalLaunchPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ExternalLaunchPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::HandedOff) => state.status = HANDED_OFF_STATUS.to_string(),
        Ok(CoreOutcome::NoHandler) => state.status = NO_HANDLER_STATUS.to_string(),
        Ok(CoreOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CoreOutcome::TargetUnavailable) => state.status = TARGET_UNAVAILABLE_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(ExternalLaunchFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &ExternalAppLaunchState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &ExternalAppLaunchState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外し、同時に 2 件出さない。
pub fn current_screen(state: &ExternalAppLaunchState) -> Screen<ExternalAppLaunchState> {
    let screen = Screen::new("電話・地図など他アプリの起動")
        .text(NOTE_TEXT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != ExternalLaunchPhase::Idle {
        return screen;
    }
    screen
        .button(PHONE_BUTTON_ID, PHONE_BUTTON_LABEL, open_phone)
        .button(MAP_BUTTON_ID, MAP_BUTTON_LABEL, open_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launching(kind: LaunchKind) -> ExternalAppLaunchState {
        let mut s = ExternalAppLaunchState::default();
        request(&mut s, kind);
        mark_launching(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, ExternalLaunchFailure>) -> (String, String) {
        let mut s = launching(LaunchKind::Phone);
        let g = s.generation;
        finish_launch(&mut s, g, r);
        assert_eq!(s.phase, ExternalLaunchPhase::Idle);
        (s.status, s.error)
    }

    #[test]
    fn six_results_are_distinct_and_none_claims_opened_or_called() {
        let all = [
            shown(Ok(CoreOutcome::HandedOff)),
            shown(Ok(CoreOutcome::NoHandler)),
            shown(Ok(CoreOutcome::Cancelled)),
            shown(Ok(CoreOutcome::TargetUnavailable)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err(ExternalLaunchFailure::Failed("x".into()))),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
            let text = format!("{}{}", a.0, a.1);
            assert!(!text.contains("開きました") && !text.contains("発信しました"));
        }
    }

    #[test]
    fn non_failures_do_not_go_to_error() {
        for o in [
            CoreOutcome::HandedOff,
            CoreOutcome::NoHandler,
            CoreOutcome::Cancelled,
            CoreOutcome::TargetUnavailable,
            CoreOutcome::Unsupported,
        ] {
            let (status, error) = shown(Ok(o));
            assert!(!status.is_empty() && error.is_empty());
        }
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = launching(LaunchKind::Map);
        let g = s.generation + 1;
        finish_launch(&mut s, g, Ok(CoreOutcome::HandedOff));
        assert_eq!(s.phase, ExternalLaunchPhase::Launching);
        assert!(s.status.is_empty());

        let mut idle = ExternalAppLaunchState::default();
        finish_launch(&mut idle, 0, Ok(CoreOutcome::HandedOff));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn double_press_is_noop_and_take_request_matches_kind() {
        let mut s = ExternalAppLaunchState::default();
        assert!(take_request(&s).is_none());
        open_map(&mut s);
        let r = take_request(&s).unwrap();
        assert_eq!(r.kind, LaunchKind::Map);
        open_phone(&mut s);
        assert_eq!(s.kind, Some(LaunchKind::Map));
        assert_eq!(s.generation, 1);
        mark_launching(&mut s);
        assert!(take_request(&s).is_none());
        open_phone(&mut s);
        assert_eq!(s.phase, ExternalLaunchPhase::Launching);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn buttons_hidden_while_busy() {
        let idle = current_screen(&ExternalAppLaunchState::default());
        let busy = current_screen(&launching(LaunchKind::Phone));
        assert!(format!("{idle:?}").contains(MAP_BUTTON_LABEL));
        assert!(!format!("{busy:?}").contains(MAP_BUTTON_LABEL));
    }
}
