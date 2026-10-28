//! Phase 11（端末機能の拡充ループ、親 #340）型サンプル「動画の変換」（Issue #1014）。
//! 「変換する」で実行時に作った 2 秒の元動画（1280×720）を小さい解像度の MP4 へ 1 回書き出し、
//! 元動画と出力の寸法・長さ・大きさを表示する 1 画面。`FANDHE_PROXY_APP=video_transcode` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod video_transcode;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/video_transcode.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_transcode`]・[`interrupt_transcode`] で状態を進める。
//! 全対象 OS（macOS / iOS / Windows / Android / Web）で同じ UI・文言を使うため、OS 別の子 issue は
//! このファイルを触らない。Web はコンテナなしの出力のため [`VideoContainer::Raw`] を持ち、
//! ページを離れた場合の [`VideoTranscodePhase::Interrupted`] も定義する。
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
pub const IDLE_HINT: &str = "1280×720・2 秒の動画を小さい解像度へ変換します";
/// 変換の最中の案内。
pub const RUNNING_STATUS: &str = "変換しています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS（または OS が古い）のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に終わりませんでした";
/// 出力を確かめられないときの案内。
pub const UNREADABLE_MESSAGE: &str = "出力を確かめられません";
/// 出力が元動画より小さくなっていないときの案内。
pub const NOT_SMALLER_MESSAGE: &str = "出力が元動画より小さくなっていません";
/// 出力の長さが元動画と合わないときの案内。
pub const MISMATCH_MESSAGE: &str = "出力の長さが元動画と一致しません";
/// 出力が 0 バイトのときの案内。
pub const EMPTY_MESSAGE: &str = "出力が空でした";
/// 中断（ページを離れた）ときの案内。失敗としては出さない。
pub const INTERRUPTED_MESSAGE: &str = "中断しました（ページを離れたため）";
/// 上記以外の失敗の固定文言（OS の文言は出さない）。
pub const GENERIC_FAILED_MESSAGE: &str = "変換できませんでした";
/// 前回の変換の後片付けを確認できないときの案内。
pub const RELEASE_PENDING_MESSAGE: &str = "前回の変換の後片付けが終わっていません";

/// 出力のコンテナ。Web はコンテナなし、それ以外は MP4。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoContainer {
    /// MP4。
    Mp4,
    /// コンテナなし（Web の符号化チャンク）。
    Raw,
}

impl VideoContainer {
    /// 画面に出すコンテナ名。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Raw => "コンテナなし",
        }
    }
}

/// 変換結果（元動画と出力の寸法・長さ・大きさ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTranscodeOutput {
    /// 出力のコンテナ。
    pub container: VideoContainer,
    /// 出力の符号形式（OS が書いた fourcc / codec 文字列。英数字のみ）。
    pub codec: String,
    /// 元動画の幅。
    pub source_width: u32,
    /// 元動画の高さ。
    pub source_height: u32,
    /// 元動画の長さ（ミリ秒）。
    pub source_ms: u64,
    /// 出力の幅（OS が書いた値）。
    pub output_width: u32,
    /// 出力の高さ（OS が書いた値）。
    pub output_height: u32,
    /// 出力の長さ（ミリ秒）。
    pub output_ms: u64,
    /// 出力のバイト数。
    pub output_bytes: u64,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoTranscodeFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// 期限切れ。
    Timeout,
    /// 出力が 0 バイト。
    Empty,
    /// 出力を解析できない・映像が無い・寸法が 0。
    Unreadable,
    /// 出力の幅か高さが元動画以上。
    NotSmaller,
    /// 出力の長さが元動画と合わない。
    Mismatch,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum VideoTranscodePhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 変換の最中。
    Running,
    /// 成功。
    Done(VideoTranscodeOutput),
    /// 中断（Web でページを離れた）。失敗とは別。
    Interrupted,
    /// 失敗。
    Failed(VideoTranscodeFailure),
}

/// 動画の変換画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VideoTranscodeState {
    /// 操作の進行状態。
    pub phase: VideoTranscodePhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_transcode(state: &mut VideoTranscodeState) {
    if matches!(
        state.phase,
        VideoTranscodePhase::Requested | VideoTranscodePhase::Running
    ) {
        return;
    }
    state.phase = VideoTranscodePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。世代は [`finish_transcode`] へそのまま渡す。
pub fn take_request(state: &VideoTranscodeState) -> Option<u64> {
    matches!(state.phase, VideoTranscodePhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut VideoTranscodeState) {
    if state.phase == VideoTranscodePhase::Requested {
        state.phase = VideoTranscodePhase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_transcode(
    state: &mut VideoTranscodeState,
    generation: u64,
    result: Result<VideoTranscodeOutput, VideoTranscodeFailure>,
) {
    if state.phase != VideoTranscodePhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok(o) => VideoTranscodePhase::Done(o),
        Err(f) => VideoTranscodePhase::Failed(f),
    };
}

/// 中断を反映する（Web の `pagehide`）。適用条件は [`finish_transcode`] と同じ。
pub fn interrupt_transcode(state: &mut VideoTranscodeState, generation: u64) {
    if state.phase == VideoTranscodePhase::Running && state.generation == generation {
        state.phase = VideoTranscodePhase::Interrupted;
    }
}

/// 処理中の案内文言。
pub fn status_text(state: &VideoTranscodeState) -> String {
    match state.phase {
        VideoTranscodePhase::Requested | VideoTranscodePhase::Running => RUNNING_STATUS.to_string(),
        VideoTranscodePhase::Interrupted => INTERRUPTED_MESSAGE.to_string(),
        _ => String::new(),
    }
}

fn seconds(ms: u64) -> String {
    format!("{:.2}", ms as f64 / 1000.0)
}

/// 成功時の結果文言（元動画・出力・長さの差を改行で並べる）。
pub fn result_text(state: &VideoTranscodeState) -> String {
    let VideoTranscodePhase::Done(o) = &state.phase else {
        return String::new();
    };
    let diff = o.output_ms as i64 - o.source_ms as i64;
    format!(
        "元動画: {}×{}・{} 秒\n出力: {}（{}）・{}×{}・{} 秒・{} バイト\n長さの差: {diff:+} ms",
        o.source_width,
        o.source_height,
        seconds(o.source_ms),
        o.container.label(),
        o.codec,
        o.output_width,
        o.output_height,
        seconds(o.output_ms),
        o.output_bytes,
    )
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。中断は失敗ではないので空。
pub fn error_text(state: &VideoTranscodeState) -> String {
    let VideoTranscodePhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        VideoTranscodeFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        VideoTranscodeFailure::Timeout => TIMEOUT_MESSAGE,
        VideoTranscodeFailure::Empty => EMPTY_MESSAGE,
        VideoTranscodeFailure::Unreadable => UNREADABLE_MESSAGE,
        VideoTranscodeFailure::NotSmaller => NOT_SMALLER_MESSAGE,
        VideoTranscodeFailure::Mismatch => MISMATCH_MESSAGE,
        VideoTranscodeFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 成功文言の行を `Node::Text` に分ける。
fn result_nodes(state: &VideoTranscodeState) -> Vec<Node<VideoTranscodeState>> {
    result_text(state)
        .lines()
        .map(|l| Node::Text(l.to_string()))
        .collect()
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &VideoTranscodeState) -> Screen<VideoTranscodeState> {
    let screen = Screen::new("動画の変換")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if matches!(
        state.phase,
        VideoTranscodePhase::Requested | VideoTranscodePhase::Running
    ) {
        return screen;
    }
    screen.button(TRANSCODE_BUTTON_ID, "変換する", request_transcode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out() -> VideoTranscodeOutput {
        VideoTranscodeOutput {
            container: VideoContainer::Mp4,
            codec: "avc1".to_string(),
            source_width: 1280,
            source_height: 720,
            source_ms: 2000,
            output_width: 640,
            output_height: 360,
            output_ms: 1990,
            output_bytes: 12_345,
        }
    }

    fn running() -> VideoTranscodeState {
        let mut s = VideoTranscodeState::default();
        request_transcode(&mut s);
        mark_running(&mut s);
        s
    }

    #[test]
    fn busy_states_do_not_accept_requests() {
        let mut s = VideoTranscodeState::default();
        request_transcode(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_transcode(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        request_transcode(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), None);
        finish_transcode(&mut s, 1, Ok(out()));
        request_transcode(&mut s);
        assert_eq!(take_request(&s), Some(2));
        assert_eq!(result_text(&s), "");
    }

    #[test]
    fn stale_and_non_running_are_ignored() {
        let mut s = running();
        finish_transcode(&mut s, 0, Ok(out()));
        interrupt_transcode(&mut s, 0);
        assert_eq!(s.phase, VideoTranscodePhase::Running);
        finish_transcode(&mut s, 1, Err(VideoTranscodeFailure::Timeout));
        // 期限で確定した後に届いた成功・中断は適用しない
        finish_transcode(&mut s, 1, Ok(out()));
        interrupt_transcode(&mut s, 1);
        assert_eq!(
            s.phase,
            VideoTranscodePhase::Failed(VideoTranscodeFailure::Timeout)
        );
        let mut idle = VideoTranscodeState::default();
        finish_transcode(&mut idle, 0, Ok(out()));
        interrupt_transcode(&mut idle, 0);
        assert_eq!(idle.phase, VideoTranscodePhase::Idle);
    }

    #[test]
    fn interrupt_is_not_a_failure() {
        let mut s = running();
        interrupt_transcode(&mut s, 1);
        assert_eq!(s.phase, VideoTranscodePhase::Interrupted);
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), INTERRUPTED_MESSAGE);
        request_transcode(&mut s);
        assert_eq!(take_request(&s), Some(2));
    }

    #[test]
    fn result_text_with_signed_diff() {
        let mut s = running();
        finish_transcode(&mut s, 1, Ok(out()));
        assert_eq!(
            result_text(&s),
            "元動画: 1280×720・2.00 秒\n出力: MP4（avc1）・640×360・1.99 秒・12345 バイト\n長さの差: -10 ms"
        );
        let mut s = running();
        let raw = VideoTranscodeOutput {
            container: VideoContainer::Raw,
            output_ms: 2010,
            ..out()
        };
        finish_transcode(&mut s, 1, Ok(raw));
        assert!(result_text(&s).ends_with("長さの差: +10 ms"));
        assert!(result_text(&s).contains("コンテナなし"));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn failure_messages() {
        let cases = [
            (
                VideoTranscodeFailure::Unsupported,
                UNSUPPORTED_MESSAGE.to_string(),
            ),
            (
                VideoTranscodeFailure::Timeout,
                format!("{FAILED_PREFIX}{TIMEOUT_MESSAGE}"),
            ),
            (
                VideoTranscodeFailure::Empty,
                format!("{FAILED_PREFIX}{EMPTY_MESSAGE}"),
            ),
            (
                VideoTranscodeFailure::Unreadable,
                format!("{FAILED_PREFIX}{UNREADABLE_MESSAGE}"),
            ),
            (
                VideoTranscodeFailure::NotSmaller,
                format!("{FAILED_PREFIX}出力が元動画より小さくなっていません"),
            ),
            (
                VideoTranscodeFailure::Mismatch,
                format!("{FAILED_PREFIX}{MISMATCH_MESSAGE}"),
            ),
            (
                VideoTranscodeFailure::Failed("x".into()),
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
        let count = |s: &VideoTranscodeState| current_screen(s).len() - 4;
        let mut s = VideoTranscodeState::default();
        assert_eq!(count(&s), 1);
        request_transcode(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_transcode(&mut s, 1, Err(VideoTranscodeFailure::Mismatch));
        assert_eq!(count(&s), 1);
    }
}
