//! Phase 11（端末機能の拡充ループ、親 #93）型サンプル「端末内 LLM」（Issue #345）。
//! 「質問する」で固定の短いプロンプトを端末内のモデルへ 1 回渡し、応答の全文を表示する 1 画面。
//! 使えない端末では理由つきの失敗を出す。`FANDHE_PROXY_APP=on_device_llm` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod on_device_llm;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/on_device_llm.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_respond`] で状態を進める。全対象 OS（macOS / iOS / Web）で
//! 同じ UI・文言を使うため、OS 別の子 issue はこのファイルを触らない。Web でだけ起きる失敗種別
//! （[`LlmFailure::PermissionDenied`]・[`LlmFailure::Interrupted`]）の文言もここで定義する。
//!
//! ```text
//! Idle/Done/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_respond（Running かつ世代一致のみ）--> Done | Failed
//! ```

use crate::{Node, Screen, WidgetId};

const ASK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// モデルへ渡す固定プロンプト（利用者の入力は受けない）。
pub const PROMPT: &str = "日本の四季を一文で紹介してください。";
/// 応答を画面に出す最大文字数（Web は応答長の上限指定が無いため画面側で切る）。
pub const MAX_DISPLAY_CHARS: usize = 400;
/// 応答を切り詰めたときに添える注記。
pub const TRUNCATED_NOTE: &str = "（長いため途中まで表示しています）";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "端末内のモデルに、次の質問を 1 回だけ渡して応答を表示します";
/// 応答を待っている間の案内。
pub const RUNNING_STATUS: &str = "応答を待っています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS・非対応機種のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 端末の設定で生成 AI が無効なときの案内。
pub const DISABLED_MESSAGE: &str = "端末の設定で生成 AI（Apple Intelligence など）が無効です";
/// モデルが端末に無いときの案内（この画面からダウンロードは始めない）。
pub const MODEL_NOT_READY_MESSAGE: &str =
    "モデルがまだ端末にありません（この画面からはダウンロードしません）";
/// モデルが応答を断ったときの案内。
pub const REFUSED_MESSAGE: &str = "モデルが応答を断りました";
/// 混雑・回数制限のときの案内。
pub const BUSY_MESSAGE: &str = "混み合っています。しばらくしてから試してください";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に応答がありませんでした";
/// 空の応答が返ったときの案内。
pub const EMPTY_MESSAGE: &str = "空の応答が返りました";
/// ブラウザが利用を許可しなかったときの案内（Web 用）。
pub const PERMISSION_DENIED_MESSAGE: &str = "ブラウザが利用を許可しませんでした";
/// 画面を離れて中断したときの案内（Web の `pagehide` 用）。
pub const INTERRUPTED_MESSAGE: &str = "画面を離れたため中断しました";

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmFailure {
    /// 非対応 OS・OS が古い・非対応機種。
    Unsupported,
    /// 端末の設定で無効。
    Disabled,
    /// モデルが端末に無い（未取得・取得中）。
    ModelNotReady,
    /// モデルが応答を断った。
    Refused,
    /// 混雑・回数制限。
    Busy,
    /// 期限切れ。
    Timeout,
    /// 空の応答。
    Empty,
    /// ブラウザが利用を許可しなかった（Web 用）。
    PermissionDenied,
    /// 画面を離れて中断した（Web の `pagehide` 用）。
    Interrupted,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LlmPhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 応答待ち。
    Running,
    /// 成功（応答の全文）。
    Done {
        /// 応答の全文（表示時に [`MAX_DISPLAY_CHARS`] で切る）。
        text: String,
    },
    /// 失敗。
    Failed(LlmFailure),
}

/// 端末内 LLM 画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OnDeviceLlmState {
    /// 操作の進行状態。
    pub phase: LlmPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
}

fn is_busy(state: &OnDeviceLlmState) -> bool {
    matches!(state.phase, LlmPhase::Requested | LlmPhase::Running)
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_respond(state: &mut OnDeviceLlmState) {
    if is_busy(state) {
        return;
    }
    state.phase = LlmPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_respond`] へそのまま渡す。
pub fn take_request(state: &OnDeviceLlmState) -> Option<u64> {
    matches!(state.phase, LlmPhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut OnDeviceLlmState) {
    if state.phase == LlmPhase::Requested {
        state.phase = LlmPhase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_respond(
    state: &mut OnDeviceLlmState,
    generation: u64,
    result: Result<String, LlmFailure>,
) {
    if state.phase != LlmPhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok(text) => LlmPhase::Done { text },
        Err(f) => LlmPhase::Failed(f),
    };
}

/// 処理中の案内文言。
pub fn status_text(state: &OnDeviceLlmState) -> String {
    if is_busy(state) {
        RUNNING_STATUS.to_string()
    } else {
        String::new()
    }
}

/// 成功時の応答文言。[`MAX_DISPLAY_CHARS`] 文字で切り、切ったときは注記を添える。
pub fn result_text(state: &OnDeviceLlmState) -> String {
    let LlmPhase::Done { text } = &state.phase else {
        return String::new();
    };
    match text.char_indices().nth(MAX_DISPLAY_CHARS) {
        Some((i, _)) => format!("{}\n{TRUNCATED_NOTE}", &text[..i]),
        None => text.clone(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &OnDeviceLlmState) -> String {
    let LlmPhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        LlmFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        LlmFailure::Disabled => DISABLED_MESSAGE,
        LlmFailure::ModelNotReady => MODEL_NOT_READY_MESSAGE,
        LlmFailure::Refused => REFUSED_MESSAGE,
        LlmFailure::Busy => BUSY_MESSAGE,
        LlmFailure::Timeout => TIMEOUT_MESSAGE,
        LlmFailure::Empty => EMPTY_MESSAGE,
        LlmFailure::PermissionDenied => PERMISSION_DENIED_MESSAGE,
        LlmFailure::Interrupted => INTERRUPTED_MESSAGE,
        LlmFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

fn result_nodes(state: &OnDeviceLlmState) -> Vec<Node<OnDeviceLlmState>> {
    let t = result_text(state);
    if t.is_empty() {
        Vec::new()
    } else {
        vec![Node::Text(t)]
    }
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &OnDeviceLlmState) -> Screen<OnDeviceLlmState> {
    let screen = Screen::new("端末内 LLM")
        .text(IDLE_HINT)
        .text(format!("質問: {PROMPT}"))
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if is_busy(state) {
        return screen;
    }
    screen.button(ASK_BUTTON_ID, "質問する", request_respond)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> OnDeviceLlmState {
        let mut s = OnDeviceLlmState::default();
        request_respond(&mut s);
        mark_running(&mut s);
        s
    }

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = OnDeviceLlmState::default();
        request_respond(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_respond(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        request_respond(&mut s);
        assert_eq!(s.generation, 1);
        finish_respond(&mut s, 1, Ok("春".into()));
        assert_eq!(result_text(&s), "春");
        request_respond(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(result_text(&s), "");
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn stale_and_non_running_finish_are_ignored() {
        let mut s = running();
        finish_respond(&mut s, 0, Ok("x".into()));
        assert_eq!(s.phase, LlmPhase::Running);
        finish_respond(&mut s, 1, Err(LlmFailure::Timeout));
        // 期限で確定済みの後に届いた成功は適用しない
        finish_respond(&mut s, 1, Ok("x".into()));
        assert_eq!(s.phase, LlmPhase::Failed(LlmFailure::Timeout));
        let mut idle = OnDeviceLlmState::default();
        finish_respond(&mut idle, 0, Ok("x".into()));
        assert_eq!(idle.phase, LlmPhase::Idle);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = OnDeviceLlmState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, LlmPhase::Idle);
    }

    #[test]
    fn result_truncates_at_boundary() {
        let at = "あ".repeat(MAX_DISPLAY_CHARS);
        let mut s = running();
        finish_respond(&mut s, 1, Ok(at.clone()));
        assert_eq!(result_text(&s), at);
        let over = format!("{at}あ");
        let mut s = running();
        finish_respond(&mut s, 1, Ok(over));
        assert_eq!(result_text(&s), format!("{at}\n{TRUNCATED_NOTE}"));
    }

    #[test]
    fn every_failure_has_nonempty_text() {
        let all = [
            LlmFailure::Unsupported,
            LlmFailure::Disabled,
            LlmFailure::ModelNotReady,
            LlmFailure::Refused,
            LlmFailure::Busy,
            LlmFailure::Timeout,
            LlmFailure::Empty,
            LlmFailure::PermissionDenied,
            LlmFailure::Interrupted,
            LlmFailure::Failed("x".into()),
        ];
        for f in all {
            let mut s = running();
            finish_respond(&mut s, 1, Err(f.clone()));
            assert!(!error_text(&s).is_empty(), "{f:?}");
            assert_eq!(result_text(&s), "");
        }
        let mut s = running();
        finish_respond(&mut s, 1, Err(LlmFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
    }

    #[test]
    fn button_hidden_while_busy() {
        // 固定 5 ノード（案内・質問・状態・失敗・結果）+ ボタン
        let count = |s: &OnDeviceLlmState| current_screen(s).len() - 5;
        let mut s = OnDeviceLlmState::default();
        assert_eq!(count(&s), 1);
        request_respond(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_respond(&mut s, 1, Err(LlmFailure::Busy));
        assert_eq!(count(&s), 1);
    }
}
