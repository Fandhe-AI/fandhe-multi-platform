//! Phase 11 の拡充ループ（親 #124）型サンプル「行動認識（歩行・乗車等）」（Issue #613）。
//! 「開始」「停止」で購読を始め・止め、現在の行動種別と確度、種別が変わった履歴（上限
//! [`HISTORY_LIMIT`] 件）を 1 画面に出す。`FANDHE_PROXY_APP=activity_recognition_show` で選ばれ、
//! `fandhe-egui-proxy` の `screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod activity_recognition_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `device/activity_recognition.rs` が行い、[`apply_poll`] / [`apply_failure`] で注入する。
//! `fandhe_device::activity_recognition` の型は依存方向の都合でここへ写す。
//!
//! 嘘の行動を出さない: 初期・待機・許可待ち・失敗・非対応は種別を持たない（[`ActivityView::Live`]
//! だけが持つ）。判定なしは「静止」と見せない。古い世代の完了は [`ActivityRecognitionShowState::session`]
//! で捨てる。文言は全対象 OS（Windows・iOS）の分をここで定義する。

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
pub const IDLE_TEXT: &str = "開始を押すと現在の行動を表示します";
/// 判定中の案内。
pub const WAITING_TEXT: &str = "行動を判定中…";
/// 許可待ち（iOS の「モーションとフィットネス」）。
pub const AWAITING_PERMISSION_TEXT: &str = "「モーションとフィットネス」の許可を待っています";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境では行動認識を使えません";
/// 権限拒否。
pub const PERMISSION_DENIED_TEXT: &str = "行動認識の利用が許可されませんでした";
/// 値が届かない。
pub const NO_READING_TEXT: &str = "取得できません（行動の値が届きません）";
/// それ以外の失敗。
pub const FAILED_TEXT: &str = "取得できません";
/// 履歴が空のときの文言。
pub const HISTORY_EMPTY_TEXT: &str = "履歴: まだありません";
/// 履歴の見出し。
pub const HISTORY_HEADER: &str = "履歴（新しい順）";
/// 画面に残す履歴の上限（`fandhe_device::activity_recognition::HISTORY_LIMIT` の写し）。
pub const HISTORY_LIMIT: usize = 20;

/// 行動種別（`fandhe_device::activity_recognition::ActivityKind` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKindView {
    /// 判定なし。
    Unknown,
    /// 静止。
    Stationary,
    /// 歩行。
    Walking,
    /// 走行。
    Running,
    /// 自転車。
    Cycling,
    /// 乗り物。
    Automotive,
}

/// 確度（`Confidence` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceView {
    /// 低い。
    Low,
    /// 中。
    Medium,
    /// 高い。
    High,
}

/// 行動（`ActivityReading` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityReadingView {
    /// 種別。
    pub kind: ActivityKindView,
    /// 確度。
    pub confidence: ConfidenceView,
    /// 受信番号。同じ番号は二重に反映しない。
    pub seq: u64,
}

/// 読み取り結果の写し（`ActivityPoll`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityPollView {
    /// 値なし。
    Waiting,
    /// 許可待ち。
    AwaitingPermission,
    /// 最新の行動。
    Reading(ActivityReadingView),
}

/// 失敗の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityFailureView {
    /// 非対応。
    Unsupported,
    /// 権限拒否。
    PermissionDenied,
    /// 期限内に値が届かない。
    NoReading,
    /// それ以外。
    Failed,
}

/// 表示状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityView {
    /// 停止中。
    #[default]
    Idle,
    /// 購読中でまだ値が無い。
    Waiting,
    /// 許可待ち。
    AwaitingPermission,
    /// 行動を表示中。
    Live(ActivityReadingView),
    /// 失敗。
    Failed(ActivityFailureView),
}

/// 履歴の 1 行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    /// 種別。
    pub kind: ActivityKindView,
    /// 購読開始からの経過秒。
    pub elapsed_secs: u64,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivityRecognitionShowState {
    /// 表示状態。
    pub view: ActivityView,
    /// 購読を望んでいるか。driver が立ち上がり・立ち下がりで OS の購読を開始・停止する。
    pub wanted: bool,
    /// 「開始」のたびに増える世代。遅れて届いた完了を捨てるために使う。
    pub session: u64,
    /// 最後に反映した受信番号。
    pub last_seq: Option<u64>,
    /// 種別が変わったときの履歴（古い順。表示は新しい順）。
    pub history: Vec<HistoryEntry>,
}

/// 「開始」。冪等（購読中の二重押下では世代を進めない）。履歴は開始で空にする。
pub fn start(s: &mut ActivityRecognitionShowState) {
    if s.wanted {
        return;
    }
    s.wanted = true;
    s.view = ActivityView::Waiting;
    s.session = s.session.wrapping_add(1);
    s.last_seq = None;
    s.history.clear();
}

/// 「停止」。冪等。失敗ではなく待機へ戻す。履歴は次の開始まで残す（過去の表示で現在値ではない）。
pub fn stop(s: &mut ActivityRecognitionShowState) {
    s.wanted = false;
    s.view = ActivityView::Idle;
}

/// 読み取り結果を反映する。停止後・世代違いは捨てる。`Waiting` / `AwaitingPermission` は
/// 最新値が消えた場合（背景遷移・購読し直し）を含め表示をそれに戻す。`Reading` は受信番号が
/// 変わったときだけ反映し、種別が変わったとき（最初の 1 件を含む）だけ履歴へ足す。
pub fn apply_poll(
    s: &mut ActivityRecognitionShowState,
    session: u64,
    poll: ActivityPollView,
    elapsed_secs: u64,
) {
    if !s.wanted || s.session != session {
        return;
    }
    match poll {
        ActivityPollView::Waiting => s.view = ActivityView::Waiting,
        ActivityPollView::AwaitingPermission => s.view = ActivityView::AwaitingPermission,
        ActivityPollView::Reading(r) => {
            if s.last_seq == Some(r.seq) {
                return;
            }
            s.last_seq = Some(r.seq);
            // 履歴の最後の種別と比べる。`Waiting` を挟んでも同じ種別なら足さない。
            let changed = s.history.last().map(|h| h.kind) != Some(r.kind);
            s.view = ActivityView::Live(r);
            if changed {
                s.history.push(HistoryEntry {
                    kind: r.kind,
                    elapsed_secs,
                });
                if s.history.len() > HISTORY_LIMIT {
                    let over = s.history.len() - HISTORY_LIMIT;
                    s.history.drain(..over);
                }
            }
        }
    }
}

/// 失敗を反映する。購読中で世代が一致したときだけ確定し、購読の希望を下ろす（再度「開始」できる）。
pub fn apply_failure(s: &mut ActivityRecognitionShowState, session: u64, f: ActivityFailureView) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = ActivityView::Failed(f);
}

/// 種別の日本語ラベル。`Unknown` を「静止」と見せない。
pub fn kind_label(k: ActivityKindView) -> &'static str {
    match k {
        ActivityKindView::Stationary => "静止",
        ActivityKindView::Walking => "歩行",
        ActivityKindView::Running => "走行",
        ActivityKindView::Cycling => "自転車",
        ActivityKindView::Automotive => "乗り物",
        ActivityKindView::Unknown => "判定なし",
    }
}

/// 確度の日本語ラベル。
pub fn confidence_label(c: ConfidenceView) -> &'static str {
    match c {
        ConfidenceView::Low => "低",
        ConfidenceView::Medium => "中",
        ConfidenceView::High => "高",
    }
}

/// 主表示（`DynamicText` の `get`）。
pub fn activity_text(s: &ActivityRecognitionShowState) -> String {
    match s.view {
        ActivityView::Idle => IDLE_TEXT.to_string(),
        ActivityView::Waiting => WAITING_TEXT.to_string(),
        ActivityView::AwaitingPermission => AWAITING_PERMISSION_TEXT.to_string(),
        ActivityView::Live(r) => format!("現在の行動: {}", kind_label(r.kind)),
        ActivityView::Failed(ActivityFailureView::Unsupported) => UNSUPPORTED_TEXT.to_string(),
        ActivityView::Failed(ActivityFailureView::PermissionDenied) => {
            PERMISSION_DENIED_TEXT.to_string()
        }
        ActivityView::Failed(ActivityFailureView::NoReading) => NO_READING_TEXT.to_string(),
        ActivityView::Failed(ActivityFailureView::Failed) => FAILED_TEXT.to_string(),
    }
}

/// 確度の行。値が無いときは空文字。
pub fn confidence_text(s: &ActivityRecognitionShowState) -> String {
    match s.view {
        ActivityView::Live(r) => format!("確度: {}", confidence_label(r.confidence)),
        _ => String::new(),
    }
}

/// 履歴の 1 行の書式（例「12 秒 歩行」）。
pub fn history_line(e: &HistoryEntry) -> String {
    format!("{} 秒 {}", e.elapsed_secs, kind_label(e.kind))
}

/// 履歴の表示（新しい順）。空なら [`HISTORY_EMPTY_TEXT`]。
pub fn history_text(s: &ActivityRecognitionShowState) -> String {
    if s.history.is_empty() {
        return HISTORY_EMPTY_TEXT.to_string();
    }
    let mut out = String::from(HISTORY_HEADER);
    for e in s.history.iter().rev() {
        out.push('\n');
        out.push_str(&history_line(e));
    }
    out
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。`wanted` に応じてボタンを 1 つだけ出す。
pub fn current_screen(s: &ActivityRecognitionShowState) -> Screen<ActivityRecognitionShowState> {
    let screen = Screen::new("行動認識")
        .dynamic_text(activity_text)
        .dynamic_text(confidence_text)
        .dynamic_text(history_text);
    if s.wanted {
        screen.button(STOP_BUTTON_ID, STOP_LABEL, stop)
    } else {
        screen.button(START_BUTTON_ID, START_LABEL, start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(kind: ActivityKindView, seq: u64) -> ActivityPollView {
        ActivityPollView::Reading(ActivityReadingView {
            kind,
            confidence: ConfidenceView::High,
            seq,
        })
    }

    fn started() -> ActivityRecognitionShowState {
        let mut s = ActivityRecognitionShowState::default();
        start(&mut s);
        s
    }

    #[test]
    fn start_waits_and_bumps_session_once() {
        let mut s = started();
        assert_eq!(
            (s.view, s.wanted, s.session),
            (ActivityView::Waiting, true, 1)
        );
        start(&mut s);
        assert_eq!(s.session, 1);
        stop(&mut s);
        assert_eq!((s.view, s.wanted), (ActivityView::Idle, false));
    }

    #[test]
    fn stale_session_and_stopped_results_are_dropped() {
        let mut s = started();
        stop(&mut s);
        start(&mut s);
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 1), 0);
        apply_failure(&mut s, 1, ActivityFailureView::Failed);
        assert_eq!((s.view, s.wanted), (ActivityView::Waiting, true));
        stop(&mut s);
        apply_poll(&mut s, 2, r(ActivityKindView::Walking, 1), 0);
        apply_failure(&mut s, 2, ActivityFailureView::Failed);
        assert_eq!(s.view, ActivityView::Idle);
    }

    #[test]
    fn same_seq_is_not_applied_twice() {
        let mut s = started();
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 5), 1);
        apply_poll(&mut s, 1, r(ActivityKindView::Running, 5), 2);
        assert_eq!(s.history.len(), 1);
        assert!(matches!(s.view, ActivityView::Live(x) if x.kind == ActivityKindView::Walking));
    }

    #[test]
    fn waiting_between_same_kind_does_not_duplicate_history() {
        let mut s = started();
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 1), 1);
        apply_poll(&mut s, 1, ActivityPollView::Waiting, 2);
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 2), 3);
        assert_eq!(s.history.len(), 1);
        apply_poll(&mut s, 1, ActivityPollView::Waiting, 4);
        apply_poll(&mut s, 1, r(ActivityKindView::Running, 3), 5);
        assert_eq!(s.history.len(), 2);
    }

    #[test]
    fn history_only_on_kind_change_and_capped() {
        let mut s = started();
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 1), 3);
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 2), 4);
        assert_eq!(s.history.len(), 1);
        for i in 0..30u64 {
            let k = if i % 2 == 0 {
                ActivityKindView::Running
            } else {
                ActivityKindView::Walking
            };
            apply_poll(&mut s, 1, r(k, 10 + i), 10 + i);
        }
        assert_eq!(s.history.len(), HISTORY_LIMIT);
        assert_eq!(s.history.last().unwrap().elapsed_secs, 39);
    }

    #[test]
    fn start_clears_history_stop_keeps_it() {
        let mut s = started();
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 1), 12);
        stop(&mut s);
        assert_eq!(history_text(&s), format!("{HISTORY_HEADER}\n12 秒 歩行"));
        start(&mut s);
        assert_eq!(s.history.len(), 0);
        assert_eq!(s.last_seq, None);
        assert_eq!(history_text(&s), HISTORY_EMPTY_TEXT);
    }

    #[test]
    fn unknown_is_not_shown_as_stationary() {
        let mut s = started();
        apply_poll(&mut s, 1, r(ActivityKindView::Unknown, 1), 0);
        assert_eq!(activity_text(&s), "現在の行動: 判定なし");
        assert!(!activity_text(&s).contains("静止"));
    }

    #[test]
    fn waiting_after_live_drops_value() {
        let mut s = started();
        apply_poll(&mut s, 1, r(ActivityKindView::Walking, 1), 0);
        apply_poll(&mut s, 1, ActivityPollView::Waiting, 1);
        assert_eq!(s.view, ActivityView::Waiting);
        assert_eq!(confidence_text(&s), "");
        assert_eq!(activity_text(&s), WAITING_TEXT);
    }

    #[test]
    fn awaiting_permission_text() {
        let mut s = started();
        apply_poll(&mut s, 1, ActivityPollView::AwaitingPermission, 0);
        assert_eq!(activity_text(&s), AWAITING_PERMISSION_TEXT);
    }

    #[test]
    fn failure_resets_wanted_and_shows_no_value() {
        let mut s = started();
        for (f, t) in [
            (ActivityFailureView::Unsupported, UNSUPPORTED_TEXT),
            (
                ActivityFailureView::PermissionDenied,
                PERMISSION_DENIED_TEXT,
            ),
            (ActivityFailureView::NoReading, NO_READING_TEXT),
            (ActivityFailureView::Failed, FAILED_TEXT),
        ] {
            let mut s2 = s.clone();
            apply_failure(&mut s2, 1, f);
            assert!(!s2.wanted);
            assert_eq!(activity_text(&s2), t);
            assert_eq!(confidence_text(&s2), "");
        }
        apply_failure(&mut s, 1, ActivityFailureView::Failed);
        start(&mut s);
        assert_eq!((s.view, s.session), (ActivityView::Waiting, 2));
    }
}
