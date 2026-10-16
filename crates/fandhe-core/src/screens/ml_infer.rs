//! Phase 11（端末機能の拡充ループ、親 #89）型サンプル「機械学習モデル推論」（Issue #289）。
//! 「推論する」で固定の線形モデルに固定入力を渡して 1 回推論し、出力・期待値・所要時間を
//! 表示する 1 画面。「壊れたモデルで試す」は OS がモデルを拒否することを確かめるための
//! 失敗系のボタン。`FANDHE_PROXY_APP=ml_infer` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod ml_infer;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/ml_inference.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_infer`] で状態を進める。全対象 OS（macOS / iOS / Windows /
//! Android）で同じ UI・文言を使うため、OS 別の子 issue はこのファイルを触らない。
//!
//! ```text
//! Idle/Done/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_infer（Running かつ世代一致のみ）--> Done | Failed
//! ```

use std::time::Duration;

use crate::{Node, Screen, WidgetId};

const INFER_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const CORRUPT_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 未操作時の案内。
pub const IDLE_HINT: &str =
    "固定の線形モデル（y = 2·x0 + 3·x1 + 1）に x = [4, 5] を渡して推論します";
/// 推論の最中の案内。
pub const RUNNING_STATUS: &str = "推論しています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS（または OS が古い）のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// OS がモデルを拒否したときの案内。
pub const INVALID_MODEL_MESSAGE: &str = "モデルを読み込めませんでした";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に終わりませんでした";
/// 出力が期待値と合わないときの案内。
pub const MISMATCH_MESSAGE: &str = "出力が期待値と一致しません";

/// 推論に使うモデルの選択（`fandhe-device` に依存しないための写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlModelChoice {
    /// 正しいモデル。
    Valid,
    /// 壊れたモデル。
    Corrupt,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// OS がモデルを拒否した。
    InvalidModel,
    /// 期限切れ。
    Timeout,
    /// 出力が期待値と合わない。
    Mismatch,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MlInferPhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested(MlModelChoice),
    /// 推論の最中。
    Running,
    /// 成功（出力・期待値・読み込み + 推論の所要時間）。
    Done {
        /// 推論の出力。
        y: f64,
        /// 期待値。
        expected: f64,
        /// 読み込み + 推論の所要時間。
        elapsed: Duration,
    },
    /// 失敗。
    Failed(InferFailure),
}

/// 機械学習モデル推論画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MlInferState {
    /// 操作の進行状態。
    pub phase: MlInferPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_infer(state: &mut MlInferState, choice: MlModelChoice) {
    if matches!(
        state.phase,
        MlInferPhase::Requested(_) | MlInferPhase::Running
    ) {
        return;
    }
    state.phase = MlInferPhase::Requested(choice);
    state.generation = state.generation.wrapping_add(1);
}

fn request_valid(state: &mut MlInferState) {
    request_infer(state, MlModelChoice::Valid);
}

fn request_corrupt(state: &mut MlInferState) {
    request_infer(state, MlModelChoice::Corrupt);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。世代は [`finish_infer`] へそのまま渡す。
pub fn take_request(state: &MlInferState) -> Option<(u64, MlModelChoice)> {
    match state.phase {
        MlInferPhase::Requested(choice) => Some((state.generation, choice)),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut MlInferState) {
    if matches!(state.phase, MlInferPhase::Requested(_)) {
        state.phase = MlInferPhase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_infer(
    state: &mut MlInferState,
    generation: u64,
    result: Result<(f64, f64, Duration), InferFailure>,
) {
    if state.phase != MlInferPhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok((y, expected, elapsed)) => MlInferPhase::Done {
            y,
            expected,
            elapsed,
        },
        Err(f) => MlInferPhase::Failed(f),
    };
}

/// 処理中の案内文言。
pub fn status_text(state: &MlInferState) -> String {
    match state.phase {
        MlInferPhase::Requested(_) | MlInferPhase::Running => RUNNING_STATUS.to_string(),
        _ => String::new(),
    }
}

/// 成功時の結果文言（出力・期待値・所要時間を改行で並べる）。
pub fn result_text(state: &MlInferState) -> String {
    match state.phase {
        MlInferPhase::Done {
            y,
            expected,
            elapsed,
        } => format!(
            "出力 y = {y:.3}\n期待値 = {expected:.3}\n所要時間（読み込み + 推論）{} ms",
            elapsed.as_millis()
        ),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &MlInferState) -> String {
    let MlInferPhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        InferFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        InferFailure::InvalidModel => INVALID_MODEL_MESSAGE,
        InferFailure::Timeout => TIMEOUT_MESSAGE,
        InferFailure::Mismatch => MISMATCH_MESSAGE,
        InferFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 結果を `Node::Text` 一覧へ展開する必要は無いが、成功文言の行を分けるために使う。
fn result_nodes(state: &MlInferState) -> Vec<Node<MlInferState>> {
    result_text(state)
        .lines()
        .map(|l| Node::Text(l.to_string()))
        .collect()
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &MlInferState) -> Screen<MlInferState> {
    let screen = Screen::new("機械学習モデル推論")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if matches!(
        state.phase,
        MlInferPhase::Requested(_) | MlInferPhase::Running
    ) {
        return screen;
    }
    screen
        .button(INFER_BUTTON_ID, "推論する", request_valid)
        .button(CORRUPT_BUTTON_ID, "壊れたモデルで試す", request_corrupt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(3);

    fn running(choice: MlModelChoice) -> MlInferState {
        let mut s = MlInferState::default();
        request_infer(&mut s, choice);
        mark_running(&mut s);
        s
    }

    #[test]
    fn request_accepted_from_idle_done_failed_only() {
        let mut s = MlInferState::default();
        request_infer(&mut s, MlModelChoice::Valid);
        assert_eq!(take_request(&s), Some((1, MlModelChoice::Valid)));
        // Requested / Running では受け付けない
        request_infer(&mut s, MlModelChoice::Corrupt);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        request_infer(&mut s, MlModelChoice::Corrupt);
        assert_eq!(s.generation, 1);
        finish_infer(&mut s, 1, Ok((24.0, 24.0, MS)));
        request_infer(&mut s, MlModelChoice::Corrupt);
        assert_eq!(take_request(&s), Some((2, MlModelChoice::Corrupt)));
        assert_eq!(result_text(&s), "");
    }

    #[test]
    fn failed_clears_previous_result_on_new_request() {
        let mut s = running(MlModelChoice::Valid);
        finish_infer(&mut s, 1, Err(InferFailure::Timeout));
        request_infer(&mut s, MlModelChoice::Valid);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn stale_and_non_running_finish_are_ignored() {
        let mut s = running(MlModelChoice::Valid);
        finish_infer(&mut s, 0, Ok((24.0, 24.0, MS)));
        assert_eq!(s.phase, MlInferPhase::Running);
        finish_infer(&mut s, 1, Err(InferFailure::Timeout));
        // 期限で確定済みの後に届いた成功は適用しない
        finish_infer(&mut s, 1, Ok((24.0, 24.0, MS)));
        assert_eq!(s.phase, MlInferPhase::Failed(InferFailure::Timeout));
        let mut idle = MlInferState::default();
        finish_infer(&mut idle, 0, Ok((24.0, 24.0, MS)));
        assert_eq!(idle.phase, MlInferPhase::Idle);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = MlInferState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, MlInferPhase::Idle);
    }

    #[test]
    fn success_text_shows_output_expected_elapsed() {
        let mut s = running(MlModelChoice::Valid);
        finish_infer(&mut s, 1, Ok((24.0, 24.0, MS)));
        assert_eq!(
            result_text(&s),
            "出力 y = 24.000\n期待値 = 24.000\n所要時間（読み込み + 推論）3 ms"
        );
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn failure_messages() {
        let cases = [
            (InferFailure::Unsupported, UNSUPPORTED_MESSAGE.to_string()),
            (
                InferFailure::InvalidModel,
                format!("{FAILED_PREFIX}{INVALID_MODEL_MESSAGE}"),
            ),
            (
                InferFailure::Timeout,
                format!("{FAILED_PREFIX}{TIMEOUT_MESSAGE}"),
            ),
            (
                InferFailure::Mismatch,
                format!("{FAILED_PREFIX}{MISMATCH_MESSAGE}"),
            ),
            (
                InferFailure::Failed("x".into()),
                format!("{FAILED_PREFIX}x"),
            ),
        ];
        for (f, expect) in cases {
            let mut s = running(MlModelChoice::Valid);
            finish_infer(&mut s, 1, Err(f));
            assert_eq!(error_text(&s), expect);
            assert_eq!(result_text(&s), "");
        }
    }

    #[test]
    fn buttons_hidden_while_busy() {
        // 固定 4 ノード（案内・状態・失敗・結果）+ ボタン
        let count = |s: &MlInferState| current_screen(s).len() - 4;
        let mut s = MlInferState::default();
        assert_eq!(count(&s), 2);
        request_infer(&mut s, MlModelChoice::Valid);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_infer(&mut s, 1, Err(InferFailure::Mismatch));
        assert_eq!(count(&s), 2);
    }
}
