//! Phase 11（端末機能の拡充ループ、親 #91）型サンプル「動画・音声の変換」（Issue #311）。
//! 「変換する」で実行時に作った 5 秒の音声を AAC へ 1 回変換し、入力と出力の大きさ・長さを
//! 表示する 1 画面。`FANDHE_PROXY_APP=media_transcode` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod media_transcode;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/media_transcode.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_transcode`]・[`interrupt_transcode`] で状態を進める。
//! 全対象 OS（macOS / iOS / Windows / Android / Web）で同じ UI・文言・形式の定義を使うため、
//! OS 別の子 issue はこのファイルを触らない。Web はコンテナなしの AAC / Opus を出すため
//! [`TranscodeFormat`] に両方を持ち、ページを離れた場合の [`TranscodePhase::Interrupted`] も定義する。
//!
//! ```text
//! Idle/Done/Interrupted/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_transcode（Running かつ世代一致のみ）--> Done | Failed
//!   --interrupt_transcode（同上。Web の pagehide）--> Interrupted
//! ```

use crate::{Node, Screen, WidgetId};

const TRANSCODE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "5 秒の音声（WAV）を AAC に変換します";
/// 変換の最中の案内。
pub const RUNNING_STATUS: &str = "変換しています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS（または OS が古い）のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に終わりませんでした";
/// 出力を m4a として確かめられないときの案内。
pub const UNREADABLE_MESSAGE: &str = "出力を確かめられません";
/// 出力の長さが入力と合わないときの案内。
pub const MISMATCH_MESSAGE: &str = "出力の長さが入力と一致しません";
/// 出力が 0 バイトのときの案内。
pub const EMPTY_MESSAGE: &str = "出力が空でした";
/// 中断（ページを離れた）ときの案内。失敗としては出さない。
pub const INTERRUPTED_MESSAGE: &str = "中断しました（ページを離れたため）";

/// 出力の形式。Web はコンテナなしの AAC / Opus、それ以外は m4a。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscodeFormat {
    /// AAC-LC を m4a に包んだもの。
    AacM4a,
    /// コンテナなしの AAC（Web）。
    AacRaw,
    /// コンテナなしの Opus（Web）。
    OpusRaw,
}

impl TranscodeFormat {
    /// 画面に出す形式名。
    pub const fn label(self) -> &'static str {
        match self {
            Self::AacM4a => "AAC（m4a）",
            Self::AacRaw => "AAC（コンテナなし）",
            Self::OpusRaw => "Opus（コンテナなし）",
        }
    }
}

/// 変換結果（入力と出力の大きさ・長さ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscodeOutput {
    /// 出力の形式。
    pub format: TranscodeFormat,
    /// 入力（WAV）のバイト数。
    pub input_bytes: u64,
    /// 入力の長さ（ミリ秒）。
    pub input_ms: u64,
    /// 出力のバイト数。
    pub output_bytes: u64,
    /// 出力の長さ（ミリ秒）。
    pub output_ms: u64,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscodeFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// 期限切れ。
    Timeout,
    /// 出力を解析できない・符号形式が違う。
    Unreadable,
    /// 出力の長さが入力と合わない。
    Mismatch,
    /// 出力が 0 バイト。
    Empty,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TranscodePhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 変換の最中。
    Running,
    /// 成功。
    Done(TranscodeOutput),
    /// 中断（Web でページを離れた）。失敗とは別。
    Interrupted,
    /// 失敗。
    Failed(TranscodeFailure),
}

/// 動画・音声の変換画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TranscodeState {
    /// 操作の進行状態。
    pub phase: TranscodePhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_transcode(state: &mut TranscodeState) {
    if matches!(
        state.phase,
        TranscodePhase::Requested | TranscodePhase::Running
    ) {
        return;
    }
    state.phase = TranscodePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。世代は [`finish_transcode`] へそのまま渡す。
pub fn take_request(state: &TranscodeState) -> Option<u64> {
    matches!(state.phase, TranscodePhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut TranscodeState) {
    if state.phase == TranscodePhase::Requested {
        state.phase = TranscodePhase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_transcode(
    state: &mut TranscodeState,
    generation: u64,
    result: Result<TranscodeOutput, TranscodeFailure>,
) {
    if state.phase != TranscodePhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok(o) => TranscodePhase::Done(o),
        Err(f) => TranscodePhase::Failed(f),
    };
}

/// 中断を反映する（Web の `pagehide`）。適用条件は [`finish_transcode`] と同じ。
pub fn interrupt_transcode(state: &mut TranscodeState, generation: u64) {
    if state.phase == TranscodePhase::Running && state.generation == generation {
        state.phase = TranscodePhase::Interrupted;
    }
}

/// 処理中の案内文言。
pub fn status_text(state: &TranscodeState) -> String {
    match state.phase {
        TranscodePhase::Requested | TranscodePhase::Running => RUNNING_STATUS.to_string(),
        TranscodePhase::Interrupted => INTERRUPTED_MESSAGE.to_string(),
        _ => String::new(),
    }
}

fn seconds(ms: u64) -> String {
    format!("{:.2}", ms as f64 / 1000.0)
}

/// 成功時の結果文言（入力・出力・長さの差を改行で並べる）。
pub fn result_text(state: &TranscodeState) -> String {
    let TranscodePhase::Done(o) = &state.phase else {
        return String::new();
    };
    let diff = o.output_ms as i64 - o.input_ms as i64;
    format!(
        "入力: WAV・{} バイト・{} 秒\n出力: {}・{} バイト・{} 秒\n長さの差: {diff:+} ms",
        o.input_bytes,
        seconds(o.input_ms),
        o.format.label(),
        o.output_bytes,
        seconds(o.output_ms),
    )
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。中断は失敗ではないので空。
pub fn error_text(state: &TranscodeState) -> String {
    let TranscodePhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        TranscodeFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        TranscodeFailure::Timeout => TIMEOUT_MESSAGE,
        TranscodeFailure::Unreadable => UNREADABLE_MESSAGE,
        TranscodeFailure::Mismatch => MISMATCH_MESSAGE,
        TranscodeFailure::Empty => EMPTY_MESSAGE,
        TranscodeFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 成功文言の行を `Node::Text` に分ける。
fn result_nodes(state: &TranscodeState) -> Vec<Node<TranscodeState>> {
    result_text(state)
        .lines()
        .map(|l| Node::Text(l.to_string()))
        .collect()
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &TranscodeState) -> Screen<TranscodeState> {
    let screen = Screen::new("動画・音声の変換")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if matches!(
        state.phase,
        TranscodePhase::Requested | TranscodePhase::Running
    ) {
        return screen;
    }
    screen.button(TRANSCODE_BUTTON_ID, "変換する", request_transcode)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: TranscodeOutput = TranscodeOutput {
        format: TranscodeFormat::AacM4a,
        input_bytes: 480_044,
        input_ms: 5000,
        output_bytes: 21_000,
        output_ms: 4980,
    };

    fn running() -> TranscodeState {
        let mut s = TranscodeState::default();
        request_transcode(&mut s);
        mark_running(&mut s);
        s
    }

    #[test]
    fn busy_states_do_not_accept_requests() {
        let mut s = TranscodeState::default();
        request_transcode(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_transcode(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        request_transcode(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), None);
        finish_transcode(&mut s, 1, Ok(OUT));
        request_transcode(&mut s);
        assert_eq!(take_request(&s), Some(2));
        assert_eq!(result_text(&s), "");
    }

    #[test]
    fn stale_and_non_running_are_ignored() {
        let mut s = running();
        finish_transcode(&mut s, 0, Ok(OUT));
        interrupt_transcode(&mut s, 0);
        assert_eq!(s.phase, TranscodePhase::Running);
        finish_transcode(&mut s, 1, Err(TranscodeFailure::Timeout));
        // 期限で確定した後に届いた成功・中断は適用しない
        finish_transcode(&mut s, 1, Ok(OUT));
        interrupt_transcode(&mut s, 1);
        assert_eq!(s.phase, TranscodePhase::Failed(TranscodeFailure::Timeout));
        let mut idle = TranscodeState::default();
        finish_transcode(&mut idle, 0, Ok(OUT));
        interrupt_transcode(&mut idle, 0);
        assert_eq!(idle.phase, TranscodePhase::Idle);
    }

    #[test]
    fn interrupt_is_not_a_failure() {
        let mut s = running();
        interrupt_transcode(&mut s, 1);
        assert_eq!(s.phase, TranscodePhase::Interrupted);
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), INTERRUPTED_MESSAGE);
        request_transcode(&mut s);
        assert_eq!(take_request(&s), Some(2));
    }

    #[test]
    fn result_text_with_signed_diff() {
        let mut s = running();
        finish_transcode(&mut s, 1, Ok(OUT));
        assert_eq!(
            result_text(&s),
            "入力: WAV・480044 バイト・5.00 秒\n出力: AAC（m4a）・21000 バイト・4.98 秒\n長さの差: -20 ms"
        );
        let mut s = running();
        let longer = TranscodeOutput {
            output_ms: 5010,
            format: TranscodeFormat::OpusRaw,
            ..OUT
        };
        finish_transcode(&mut s, 1, Ok(longer));
        assert!(result_text(&s).ends_with("長さの差: +10 ms"));
        assert!(result_text(&s).contains("Opus（コンテナなし）"));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn failure_messages() {
        let cases = [
            (
                TranscodeFailure::Unsupported,
                UNSUPPORTED_MESSAGE.to_string(),
            ),
            (
                TranscodeFailure::Timeout,
                format!("{FAILED_PREFIX}{TIMEOUT_MESSAGE}"),
            ),
            (
                TranscodeFailure::Unreadable,
                format!("{FAILED_PREFIX}{UNREADABLE_MESSAGE}"),
            ),
            (
                TranscodeFailure::Mismatch,
                format!("{FAILED_PREFIX}{MISMATCH_MESSAGE}"),
            ),
            (
                TranscodeFailure::Empty,
                format!("{FAILED_PREFIX}{EMPTY_MESSAGE}"),
            ),
            (
                TranscodeFailure::Failed("x".into()),
                format!("{FAILED_PREFIX}x"),
            ),
        ];
        for (f, expect) in cases {
            let mut s = running();
            finish_transcode(&mut s, 1, Err(f));
            assert_eq!(error_text(&s), expect);
            assert_eq!(result_text(&s), "");
        }
    }

    #[test]
    fn button_hidden_while_busy() {
        // 固定 4 ノード（案内・状態・失敗・結果）+ ボタン
        let count = |s: &TranscodeState| current_screen(s).len() - 4;
        let mut s = TranscodeState::default();
        assert_eq!(count(&s), 1);
        request_transcode(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_transcode(&mut s, 1, Err(TranscodeFailure::Mismatch));
        assert_eq!(count(&s), 1);
    }
}
