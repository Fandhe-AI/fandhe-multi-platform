//! Phase 11 の拡充ループ（親 #142）型サンプル「環境音の分類」（Issue #725）。
//! 「開始」「停止」でマイクの聞き取りを始め・止め、まわりの音の分類結果（上位 3 件のラベルと
//! 信頼度）を 1 画面に出す。`FANDHE_PROXY_APP=sound_classification_show` で選ばれ、
//! `fandhe-egui-proxy` の `screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod sound_classification_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `device/sound_classification.rs` が行い、[`apply_poll`] / [`apply_interrupted`] /
//! [`apply_failure`] で注入する。`fandhe_device::sound_classification` の型は依存方向の都合で
//! ここへ写す。
//!
//! 嘘の結果を出さない: 初期・待機・許可待ち・中断・失敗は結果を持たない（[`SoundView::Live`] だけが
//! 持つ）。古い世代の完了は [`SoundClassificationShowState::session`] で捨てる。文言は全対象 OS
//! （macOS・iOS）の分をここで定義する。

use crate::{Screen, WidgetId};

/// 「開始」ボタン。
pub const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタン。
pub const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「開始」の文言。
pub const START_LABEL: &str = "開始";
/// 「停止」の文言。
pub const STOP_LABEL: &str = "停止";
/// 待機中の案内。
pub const IDLE_TEXT: &str = "開始を押すとまわりの音を分類します";
/// 許可待ち。
pub const AWAITING_PERMISSION_TEXT: &str = "マイクの許可を待っています";
/// 聞き取り中で結果が無い。
pub const LISTENING_TEXT: &str = "聞き取り中…";
/// 背景遷移による中断（iOS）。
pub const INTERRUPTED_BACKGROUND_TEXT: &str = "アプリが背景に移ったため止めました";
/// 音声の割り込みによる中断（iOS）。
pub const INTERRUPTED_AUDIO_TEXT: &str = "ほかの音声に割り込まれたため止めました";
/// 入力機器の変更による中断。
pub const INTERRUPTED_INPUT_TEXT: &str = "入力機器が変わったため止めました";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境では音の分類を使えません（macOS 12 未満を含む）";
/// 権限拒否。
pub const PERMISSION_DENIED_TEXT: &str = "マイクの利用が許可されませんでした";
/// マイクなし。
pub const NO_MICROPHONE_TEXT: &str = "マイクが見つかりません";
/// 結果が届かない。
pub const NO_RESULT_TEXT: &str = "取得できません（分類の結果が届きません）";
/// それ以外の失敗。
pub const FAILED_TEXT: &str = "取得できません";
/// ラベルの表示についての注記。
pub const LABEL_NOTE_TEXT: &str = "ラベルは OS の識別子（英語）のまま表示します";

/// 分類の 1 候補（`fandhe_device::sound_classification::Classification` の写し。信頼度は百分率）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationView {
    /// OS の識別子のままのラベル。
    pub label: String,
    /// 信頼度の百分率（0〜100）。
    pub percent: u8,
}

/// 分類結果（`ClassificationResult` の写し）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultView {
    /// 上位の候補（信頼度の降順、最大 3 件）。
    pub top: Vec<ClassificationView>,
    /// 結果の番号。同じ番号は二重に反映しない。
    pub seq: u64,
}

/// 読み取り結果の写し（中断以外の `SoundPoll`）。中断は [`apply_interrupted`] で別に注入する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundPollView {
    /// 未開始・停止後に開始後の読み取りが返した待機。聞き取り中として扱う。
    Idle,
    /// 許可待ち。
    AwaitingPermission,
    /// 聞き取り中で結果なし。
    Listening,
    /// 最新の結果。
    Result(ResultView),
}

/// 中断の理由（`InterruptReason` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReasonView {
    /// 背景遷移（iOS）。
    Background,
    /// 音声の割り込み（iOS）。
    AudioInterruption,
    /// 入力機器の変更。
    InputChanged,
}

/// 失敗の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundFailureView {
    /// 非対応。
    Unsupported,
    /// 権限拒否。
    PermissionDenied,
    /// マイクなし。
    NoMicrophone,
    /// 期限内に結果が届かない。
    NoResult,
    /// それ以外。
    Failed,
}

/// 表示状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SoundView {
    /// 停止中。
    #[default]
    Idle,
    /// 許可待ち。
    AwaitingPermission,
    /// 聞き取り中でまだ結果が無い。
    Listening,
    /// 結果を表示中。
    Live(ResultView),
    /// 中断（資源は解放済み。再開は「開始」から）。
    Interrupted(InterruptReasonView),
    /// 失敗。
    Failed(SoundFailureView),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoundClassificationShowState {
    /// 表示状態。
    pub view: SoundView,
    /// 聞き取りを望んでいるか。driver が立ち上がり・立ち下がりで OS の聞き取りを開始・停止する。
    pub wanted: bool,
    /// 「開始」のたびに増える世代。遅れて届いた完了を捨てるために使う。
    pub session: u64,
    /// 最後に反映した結果の番号。
    pub last_seq: Option<u64>,
}

/// 「開始」。冪等（聞き取り中の二重押下では世代を進めない）。
pub fn start(s: &mut SoundClassificationShowState) {
    if s.wanted {
        return;
    }
    s.wanted = true;
    s.view = SoundView::Listening;
    s.session = s.session.wrapping_add(1);
    s.last_seq = None;
}

/// 「停止」。冪等。失敗ではなく待機へ戻し、最後の結果を残さない。
pub fn stop(s: &mut SoundClassificationShowState) {
    s.wanted = false;
    s.view = SoundView::Idle;
    s.last_seq = None;
}

/// 読み取り結果を反映する。停止後・世代違いは捨てる。結果以外は表示をそれに戻し、直前の結果を
/// 残さない。結果は番号が変わったときだけ反映する。
pub fn apply_poll(s: &mut SoundClassificationShowState, session: u64, poll: SoundPollView) {
    if !s.wanted || s.session != session {
        return;
    }
    match poll {
        SoundPollView::Idle | SoundPollView::Listening => s.view = SoundView::Listening,
        SoundPollView::AwaitingPermission => s.view = SoundView::AwaitingPermission,
        SoundPollView::Result(r) => {
            if s.last_seq == Some(r.seq) {
                return;
            }
            s.last_seq = Some(r.seq);
            s.view = SoundView::Live(r);
        }
    }
}

/// 中断を反映する。聞き取り中で世代が一致したときだけ確定し、希望を下ろす（再度「開始」できる）。
pub fn apply_interrupted(
    s: &mut SoundClassificationShowState,
    session: u64,
    reason: InterruptReasonView,
) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = SoundView::Interrupted(reason);
}

/// 失敗を反映する。聞き取り中で世代が一致したときだけ確定し、希望を下ろす（再度「開始」できる）。
pub fn apply_failure(s: &mut SoundClassificationShowState, session: u64, f: SoundFailureView) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = SoundView::Failed(f);
}

/// 主表示（`DynamicText` の `get`）。
pub fn status_text(s: &SoundClassificationShowState) -> String {
    match &s.view {
        SoundView::Idle => IDLE_TEXT.to_string(),
        SoundView::AwaitingPermission => AWAITING_PERMISSION_TEXT.to_string(),
        SoundView::Listening => LISTENING_TEXT.to_string(),
        SoundView::Live(_) => LISTENING_TEXT.to_string(),
        SoundView::Interrupted(InterruptReasonView::Background) => {
            INTERRUPTED_BACKGROUND_TEXT.to_string()
        }
        SoundView::Interrupted(InterruptReasonView::AudioInterruption) => {
            INTERRUPTED_AUDIO_TEXT.to_string()
        }
        SoundView::Interrupted(InterruptReasonView::InputChanged) => {
            INTERRUPTED_INPUT_TEXT.to_string()
        }
        SoundView::Failed(SoundFailureView::Unsupported) => UNSUPPORTED_TEXT.to_string(),
        SoundView::Failed(SoundFailureView::PermissionDenied) => PERMISSION_DENIED_TEXT.to_string(),
        SoundView::Failed(SoundFailureView::NoMicrophone) => NO_MICROPHONE_TEXT.to_string(),
        SoundView::Failed(SoundFailureView::NoResult) => NO_RESULT_TEXT.to_string(),
        SoundView::Failed(SoundFailureView::Failed) => FAILED_TEXT.to_string(),
    }
}

/// 候補 1 行の書式（例「dog_bark — 87%」）。
pub fn line_text(c: &ClassificationView) -> String {
    format!("{} — {}%", c.label, c.percent)
}

/// 結果の表示。結果が無いときは空文字（嘘の結果を出さない）。
pub fn result_text(s: &SoundClassificationShowState) -> String {
    match &s.view {
        SoundView::Live(r) => r.top.iter().map(line_text).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// ラベルについての注記。結果を出しているときだけ出す。
pub fn note_text(s: &SoundClassificationShowState) -> String {
    match s.view {
        SoundView::Live(_) => LABEL_NOTE_TEXT.to_string(),
        _ => String::new(),
    }
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。`wanted` に応じてボタンを 1 つだけ出す。
pub fn current_screen(s: &SoundClassificationShowState) -> Screen<SoundClassificationShowState> {
    let screen = Screen::new("環境音の分類")
        .dynamic_text(status_text)
        .dynamic_text(result_text)
        .dynamic_text(note_text);
    if s.wanted {
        screen.button(STOP_BUTTON_ID, STOP_LABEL, stop)
    } else {
        screen.button(START_BUTTON_ID, START_LABEL, start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(seq: u64, labels: &[(&str, u8)]) -> SoundPollView {
        SoundPollView::Result(ResultView {
            top: labels
                .iter()
                .map(|(l, p)| ClassificationView {
                    label: (*l).to_string(),
                    percent: *p,
                })
                .collect(),
            seq,
        })
    }

    fn started() -> SoundClassificationShowState {
        let mut s = SoundClassificationShowState::default();
        start(&mut s);
        s
    }

    #[test]
    fn start_listens_and_bumps_session_once() {
        let mut s = started();
        assert_eq!(
            (s.view.clone(), s.wanted, s.session),
            (SoundView::Listening, true, 1)
        );
        start(&mut s);
        assert_eq!(s.session, 1);
        stop(&mut s);
        assert_eq!((s.view.clone(), s.wanted), (SoundView::Idle, false));
    }

    #[test]
    fn stale_session_and_stopped_results_are_dropped() {
        let mut s = started();
        stop(&mut s);
        start(&mut s);
        apply_poll(&mut s, 1, result(1, &[("dog", 90)]));
        apply_failure(&mut s, 1, SoundFailureView::Failed);
        apply_interrupted(&mut s, 1, InterruptReasonView::InputChanged);
        assert_eq!((s.view.clone(), s.wanted), (SoundView::Listening, true));
        stop(&mut s);
        apply_poll(&mut s, 2, result(1, &[("dog", 90)]));
        apply_failure(&mut s, 2, SoundFailureView::Failed);
        apply_interrupted(&mut s, 2, InterruptReasonView::InputChanged);
        assert_eq!(s.view, SoundView::Idle);
    }

    #[test]
    fn live_shows_top_lines_and_note() {
        let mut s = started();
        apply_poll(&mut s, 1, result(1, &[("dog_bark", 87), ("speech", 5)]));
        assert_eq!(result_text(&s), "dog_bark — 87%\nspeech — 5%");
        assert_eq!(note_text(&s), LABEL_NOTE_TEXT);
    }

    #[test]
    fn same_seq_is_not_applied_twice() {
        let mut s = started();
        apply_poll(&mut s, 1, result(5, &[("a", 90)]));
        apply_poll(&mut s, 1, result(5, &[("b", 80)]));
        assert_eq!(result_text(&s), "a — 90%");
    }

    #[test]
    fn stop_drops_last_result() {
        let mut s = started();
        apply_poll(&mut s, 1, result(1, &[("a", 90)]));
        stop(&mut s);
        assert_eq!(result_text(&s), "");
        assert_eq!(note_text(&s), "");
        assert_eq!(status_text(&s), IDLE_TEXT);
        start(&mut s);
        // 開始し直したら番号の重複判定も戻り、同じ番号でも反映できる。
        apply_poll(&mut s, 2, result(1, &[("b", 70)]));
        assert_eq!(result_text(&s), "b — 70%");
    }

    #[test]
    fn listening_after_live_drops_value() {
        let mut s = started();
        apply_poll(&mut s, 1, result(1, &[("a", 90)]));
        apply_poll(&mut s, 1, SoundPollView::Listening);
        assert_eq!(s.view, SoundView::Listening);
        assert_eq!(result_text(&s), "");
        assert_eq!(status_text(&s), LISTENING_TEXT);
    }

    #[test]
    fn awaiting_permission_text() {
        let mut s = started();
        apply_poll(&mut s, 1, SoundPollView::AwaitingPermission);
        assert_eq!(status_text(&s), AWAITING_PERMISSION_TEXT);
    }

    #[test]
    fn interruption_and_failure_clear_result_and_wanted() {
        for (reason, text) in [
            (InterruptReasonView::Background, INTERRUPTED_BACKGROUND_TEXT),
            (
                InterruptReasonView::AudioInterruption,
                INTERRUPTED_AUDIO_TEXT,
            ),
            (InterruptReasonView::InputChanged, INTERRUPTED_INPUT_TEXT),
        ] {
            let mut s = started();
            apply_poll(&mut s, 1, result(1, &[("a", 90)]));
            apply_interrupted(&mut s, 1, reason);
            assert!(!s.wanted);
            assert_eq!(status_text(&s), text);
            assert_eq!(result_text(&s), "");
            assert_eq!(note_text(&s), "");
        }
        for (f, text) in [
            (SoundFailureView::Unsupported, UNSUPPORTED_TEXT),
            (SoundFailureView::PermissionDenied, PERMISSION_DENIED_TEXT),
            (SoundFailureView::NoMicrophone, NO_MICROPHONE_TEXT),
            (SoundFailureView::NoResult, NO_RESULT_TEXT),
            (SoundFailureView::Failed, FAILED_TEXT),
        ] {
            let mut s = started();
            apply_poll(&mut s, 1, result(1, &[("a", 90)]));
            apply_failure(&mut s, 1, f);
            assert!(!s.wanted);
            assert_eq!(status_text(&s), text);
            assert_eq!(result_text(&s), "");
        }
    }

    #[test]
    fn restart_after_failure_works() {
        let mut s = started();
        apply_failure(&mut s, 1, SoundFailureView::Failed);
        start(&mut s);
        assert_eq!((s.view.clone(), s.session), (SoundView::Listening, 2));
    }
}
