//! Phase 11（端末機能の拡充ループ、親 #178）型サンプル「アプリの前面・背面」（Issue #893）。
//! 「受け取りを始める」で OS のアプリの前面・背面・凍結の通知を購読し、受信順の番号つきで直近 8 件と
//! 種類ごとの回数を出す 1 画面。`FANDHE_PROXY_APP=app_lifecycle_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod app_lifecycle_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/app_lifecycle.rs` が [`take_init`] / [`take_start`] で要求を観測し、[`apply_init`] /
//! [`finish_start`] / [`apply_snapshot`] / [`mark_failed`] / [`mark_left`] で結果を注入する。
//! device の型はドライバが本画面の鏡像の型へ写す。
//!
//! **起動時には購読しない**（ボタン起点）。全対象 OS の状態・文言をここで確定済みで、後続 OS の
//! 実装（iOS・Android・Web）は本画面を触らない。表示は [`SupportView`]（OS が知らせる種類と読める状態）で
//! 決め、OS 名では分岐しない。OS が知らせない種類は「この OS では取れない」と出し、「0 回」とは出さない
//! （凍結・ページのキャッシュからの復帰は Web の一部だけが知らせる）。
//!
//! # 世代
//!
//! 開始のたびに `generation` を進め、停止後・差し替え後に届いた完了通知は世代で捨てる。停止は失敗ではない。
//!
//! ```text
//! Idle --「受け取りを始める」--> Starting(gen += 1) --finish_start(Ok)--> Receiving
//! Receiving --apply_snapshot(出来事・今の状態)--> Receiving
//! Starting/Receiving --「止める」--> Idle（記録は残し「止めました」）
//! Receiving --mark_failed--> Idle + 「受け取りが止まりました」
//! Receiving --mark_left--> Idle + 「画面を離れたため止めました」（正常）
//! ```

use crate::{Screen, WidgetId};

const TOGGLE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 画面タイトル。
pub const TITLE: &str = "アプリの前面・背面";
/// 開始ボタン。
pub const START_LABEL: &str = "受け取りを始める";
/// 停止ボタン。
pub const STOP_LABEL: &str = "止める";
/// 開始処理中。
pub const STARTING_STATUS: &str = "開始中…";
/// 受信中。
pub const RECEIVING_STATUS: &str = "受け取り中";
/// 利用者の停止（正常）。
pub const STOPPED_STATUS: &str = "止めました";
/// 画面を離れて止めた（正常）。
pub const LEFT_STATUS: &str = "画面を離れたため止めました";
/// 開始失敗。
pub const START_FAILED_TEXT: &str = "受け取りを始められませんでした";
/// 実行中の失敗。
pub const RUNTIME_FAILED_TEXT: &str = "受け取りが止まりました";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境ではアプリの前面・背面の通知を受け取れません";
/// OS が知らせない種類・読めない状態。
pub const NOT_AVAILABLE: &str = "この OS では取れない";
/// 今の状態の読み取り失敗。
pub const READ_FAILED: &str = "読み取れませんでした";
/// 状態ラベル: 操作の対象。
pub const ACTIVE_STATE: &str = "操作の対象";
/// 状態ラベル: 操作の対象ではない。
pub const INACTIVE_STATE: &str = "操作の対象ではない";
/// 状態ラベル: 見えている。
pub const VISIBLE_STATE: &str = "見えている";
/// 状態ラベル: 見えていない。
pub const HIDDEN_STATE: &str = "見えていない";
/// 状態ラベル: 不明。
pub const UNKNOWN_STATE: &str = "不明";
/// 今の「操作の対象か」の見出し。
pub const ACTIVE_PREFIX: &str = "操作の対象か: ";
/// 今の「見えているか」の見出し。
pub const VISIBLE_PREFIX: &str = "見えているか: ";
/// 直近一覧の見出し。
pub const RECENT_HEADING: &str = "直近の出来事";
/// 直近一覧が空のとき。
pub const RECENT_EMPTY: &str = "まだ届いていません";
/// 直近に残す件数の上限（device 側の `MAX_RECENT` と揃える）。
pub const MAX_RECENT: usize = 8;

/// 出来事の種類（device の `LifecycleEvent` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// 操作の対象になった。
    Activated,
    /// 操作の対象から外れた。
    Deactivated,
    /// 見えなくなった。
    Hidden,
    /// 見えるようになった。
    Shown,
    /// 凍結された。
    Frozen,
    /// 凍結から戻った。
    Unfrozen,
    /// ページのキャッシュから戻った。
    RestoredFromCache,
}

/// 表示順の全種類。
pub const ALL_KINDS: [EventKind; 7] = [
    EventKind::Activated,
    EventKind::Deactivated,
    EventKind::Hidden,
    EventKind::Shown,
    EventKind::Frozen,
    EventKind::Unfrozen,
    EventKind::RestoredFromCache,
];

impl EventKind {
    /// [`ALL_KINDS`] 内の位置。
    pub fn index(self) -> usize {
        ALL_KINDS.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// 表示ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Self::Activated => "操作の対象になった",
            Self::Deactivated => "操作の対象から外れた",
            Self::Hidden => "見えなくなった",
            Self::Shown => "見えるようになった",
            Self::Frozen => "凍結された",
            Self::Unfrozen => "凍結から戻った",
            Self::RestoredFromCache => "ページのキャッシュから戻った",
        }
    }
}

/// OS が知らせる種類と読める状態の鏡像。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportView {
    /// 種類ごとに OS が通知を出すか（[`ALL_KINDS`] の順）。
    pub events: [bool; 7],
    /// 今の「操作の対象か」を読めるか。
    pub active: bool,
    /// 今の「見えているか」を読めるか。
    pub visible: bool,
}

/// 今の「操作の対象か」の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveView {
    /// 操作の対象。
    Active,
    /// 操作の対象ではない。
    Inactive,
    /// OS が判定できない。
    Unknown,
    /// 読み取りに失敗した。
    ReadFailed,
    /// この OS では読めない。
    #[default]
    NotAvailable,
}

/// 今の「見えているか」の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VisibleView {
    /// 見えている。
    Visible,
    /// 見えていない。
    Hidden,
    /// OS が判定できない。
    Unknown,
    /// 読み取りに失敗した。
    ReadFailed,
    /// この OS では読めない。
    #[default]
    NotAvailable,
}

/// 今の状態の鏡像。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurrentView {
    /// 操作の対象か。
    pub active: ActiveView,
    /// 見えているか。
    pub visible: VisibleView,
}

/// 進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 受け取っていない。
    #[default]
    Idle,
    /// 購読の開始待ち。
    Starting,
    /// 受信中。
    Receiving,
}

/// 結果の種類（受信中は `phase` で表す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 利用者の停止（正常）。
    Stopped,
    /// 画面を離れて停止（正常）。
    Left,
    /// 開始失敗。
    StartFailed,
    /// 実行中の失敗。
    RuntimeFailed,
    /// 非対応。
    Unsupported,
}

/// 開始の失敗の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFailure {
    /// 非対応。
    Unsupported,
    /// 開始に失敗した。
    Failed,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppLifecycleShowState {
    /// 進行状態。
    pub phase: Phase,
    /// 直近に立てた開始要求の世代。
    pub generation: u64,
    /// OS が知らせる種類と読める状態。`None` は未取得（初回の読み出し前）。
    pub support: Option<SupportView>,
    /// 今の状態。
    pub current: CurrentView,
    /// 直近の出来事（新しい順、`(通し番号, 種類)`）。
    pub recent: Vec<(u64, EventKind)>,
    /// 種類ごとの回数（[`ALL_KINDS`] の順）。
    pub counts: [u64; 7],
    /// 結果の種類。
    pub outcome: Option<Outcome>,
}

/// 開始ボタン。`Idle` で非対応でないときだけ受け付け、連打しても世代は 1 回分だけ進む。
pub fn start(state: &mut AppLifecycleShowState) {
    if state.phase != Phase::Idle || state.outcome == Some(Outcome::Unsupported) {
        return;
    }
    state.phase = Phase::Starting;
    state.generation = state.generation.wrapping_add(1);
    state.outcome = None;
    state.recent.clear();
    state.counts = [0; 7];
}

/// 停止ボタン。利用者の停止は正常で、記録は残す。`Idle` では何もしない。
pub fn stop(state: &mut AppLifecycleShowState) {
    if state.phase == Phase::Idle {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::Stopped);
}

/// 開始 / 停止ボタンのハンドラ。
pub fn toggle(state: &mut AppLifecycleShowState) {
    if state.phase == Phase::Idle {
        start(state);
    } else {
        stop(state);
    }
}

/// 初回の読み出し要求を観測する（状態は変えない）。`apply_init` が届くまで true。
pub fn take_init(state: &AppLifecycleShowState) -> bool {
    state.support.is_none()
}

/// 開始要求の世代を観測する（状態は変えない）。
pub fn take_start(state: &AppLifecycleShowState) -> Option<u64> {
    (state.phase == Phase::Starting).then_some(state.generation)
}

/// 初回の読み出し結果を反映する。何も知らせない OS は非対応にする。
pub fn apply_init(state: &mut AppLifecycleShowState, support: SupportView, current: CurrentView) {
    state.support = Some(support);
    state.current = current;
    if !support.events.iter().any(|e| *e) && state.phase == Phase::Idle {
        state.outcome = Some(Outcome::Unsupported);
    }
}

/// 開始の結果を反映する。`Starting` かつ世代が一致するときだけ。
pub fn finish_start(
    state: &mut AppLifecycleShowState,
    generation: u64,
    result: Result<(), StartFailure>,
) {
    if state.phase != Phase::Starting || state.generation != generation {
        return;
    }
    match result {
        Ok(()) => state.phase = Phase::Receiving,
        Err(f) => {
            state.phase = Phase::Idle;
            state.outcome = Some(match f {
                StartFailure::Unsupported => Outcome::Unsupported,
                StartFailure::Failed => Outcome::StartFailed,
            });
        }
    }
}

/// 出来事の記録と今の状態を反映する。`Receiving` かつ世代が一致するときだけ（停止後・古い世代は捨てる）。
pub fn apply_snapshot(
    state: &mut AppLifecycleShowState,
    generation: u64,
    recent: Vec<(u64, EventKind)>,
    counts: [u64; 7],
    current: Option<CurrentView>,
) {
    if state.phase != Phase::Receiving || state.generation != generation {
        return;
    }
    state.recent = recent;
    state.recent.truncate(MAX_RECENT);
    state.counts = counts;
    if let Some(c) = current {
        state.current = c;
    }
}

/// 実行中の失敗を反映する。`Starting` / `Receiving` かつ世代が一致するときだけ。
pub fn mark_failed(state: &mut AppLifecycleShowState, generation: u64) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::RuntimeFailed);
}

/// 画面を離れて止めたことを反映する（正常。Activity 再生成などで使う）。
pub fn mark_left(state: &mut AppLifecycleShowState, generation: u64) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::Left);
}

fn active_text(v: ActiveView) -> &'static str {
    match v {
        ActiveView::Active => ACTIVE_STATE,
        ActiveView::Inactive => INACTIVE_STATE,
        ActiveView::Unknown => UNKNOWN_STATE,
        ActiveView::ReadFailed => READ_FAILED,
        ActiveView::NotAvailable => NOT_AVAILABLE,
    }
}

fn visible_text(v: VisibleView) -> &'static str {
    match v {
        VisibleView::Visible => VISIBLE_STATE,
        VisibleView::Hidden => HIDDEN_STATE,
        VisibleView::Unknown => UNKNOWN_STATE,
        VisibleView::ReadFailed => READ_FAILED,
        VisibleView::NotAvailable => NOT_AVAILABLE,
    }
}

/// 状態の案内（結果・種類ごとの回数・今の状態・直近の出来事）。失敗の文言は含めない。
pub fn status_text(state: &AppLifecycleShowState) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        match (state.phase, state.outcome) {
            (Phase::Starting, _) => STARTING_STATUS,
            (Phase::Receiving, _) => RECEIVING_STATUS,
            (Phase::Idle, Some(Outcome::Stopped)) => STOPPED_STATUS,
            (Phase::Idle, Some(Outcome::Left)) => LEFT_STATUS,
            (Phase::Idle, Some(Outcome::Unsupported)) => UNSUPPORTED_TEXT,
            _ => "",
        }
        .to_string(),
    );
    if let Some(support) = state.support {
        for kind in ALL_KINDS {
            let i = kind.index();
            if support.events[i] {
                lines.push(format!("{}: {} 回", kind.label(), state.counts[i]));
            } else {
                lines.push(format!("{}: {}", kind.label(), NOT_AVAILABLE));
            }
        }
        lines.push(format!(
            "{ACTIVE_PREFIX}{}",
            active_text(state.current.active)
        ));
        lines.push(format!(
            "{VISIBLE_PREFIX}{}",
            visible_text(state.current.visible)
        ));
    }
    if state.outcome != Some(Outcome::Unsupported) {
        lines.push(RECENT_HEADING.to_string());
        if state.recent.is_empty() {
            lines.push(RECENT_EMPTY.to_string());
        }
        for (seq, kind) in &state.recent {
            lines.push(format!("#{seq} {}", kind.label()));
        }
    }
    lines.retain(|l| !l.is_empty());
    lines.join("\n")
}

/// 失敗の案内（`TextRole::Error`）。停止・画面を離れたは失敗ではないので出さない。
pub fn error_text(state: &AppLifecycleShowState) -> String {
    match (state.phase, state.outcome) {
        (Phase::Idle, Some(Outcome::StartFailed)) => START_FAILED_TEXT.to_string(),
        (Phase::Idle, Some(Outcome::RuntimeFailed)) => RUNTIME_FAILED_TEXT.to_string(),
        _ => String::new(),
    }
}

/// 画面を組み立てる。非対応のときは開始ボタンを出さない。
pub fn current_screen(state: &AppLifecycleShowState) -> Screen<AppLifecycleShowState> {
    let screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.outcome == Some(Outcome::Unsupported) && state.phase == Phase::Idle {
        return screen;
    }
    let label = if state.phase == Phase::Idle {
        START_LABEL
    } else {
        STOP_LABEL
    };
    screen.button(TOGGLE_BUTTON_ID, label, toggle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn support_all() -> SupportView {
        SupportView {
            events: [true; 7],
            active: true,
            visible: true,
        }
    }

    fn receiving() -> AppLifecycleShowState {
        let mut s = AppLifecycleShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Ok(()));
        s
    }

    #[test]
    fn startup_idle_and_does_not_subscribe() {
        let s = AppLifecycleShowState::default();
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(take_start(&s), None);
        assert!(take_init(&s));
        let mut s = s;
        apply_init(&mut s, support_all(), CurrentView::default());
        assert!(!take_init(&s));
    }

    #[test]
    fn start_spam_advances_generation_once() {
        let mut s = AppLifecycleShowState::default();
        start(&mut s);
        start(&mut s);
        start(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_start(&s), Some(1));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = AppLifecycleShowState::default();
        start(&mut s);
        finish_start(&mut s, 99, Ok(()));
        assert_eq!(s.phase, Phase::Starting);
        finish_start(&mut s, 1, Ok(()));
        apply_snapshot(
            &mut s,
            99,
            vec![(1, EventKind::Activated)],
            [1, 0, 0, 0, 0, 0, 0],
            None,
        );
        assert!(s.recent.is_empty());
        mark_failed(&mut s, 99);
        assert_eq!(s.phase, Phase::Receiving);
    }

    #[test]
    fn snapshot_after_stop_is_dropped() {
        let mut s = receiving();
        stop(&mut s);
        apply_snapshot(
            &mut s,
            1,
            vec![(1, EventKind::Shown)],
            [0, 0, 0, 1, 0, 0, 0],
            None,
        );
        assert!(s.recent.is_empty());
    }

    #[test]
    fn stop_is_normal_not_failure() {
        let mut s = receiving();
        stop(&mut s);
        assert_eq!(s.outcome, Some(Outcome::Stopped));
        assert_eq!(error_text(&s), "");
        assert!(status_text(&s).contains(STOPPED_STATUS));
    }

    #[test]
    fn runtime_failure_shows_error() {
        let mut s = receiving();
        mark_failed(&mut s, 1);
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(error_text(&s), RUNTIME_FAILED_TEXT);
    }

    #[test]
    fn start_failure_shows_error_and_unsupported_hides_button() {
        let mut s = AppLifecycleShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Err(StartFailure::Failed));
        assert_eq!(error_text(&s), START_FAILED_TEXT);
        start(&mut s);
        finish_start(&mut s, 2, Err(StartFailure::Unsupported));
        assert_eq!(s.outcome, Some(Outcome::Unsupported));
        let screen = current_screen(&s);
        let mut s2 = s.clone();
        assert!(!click_button(&screen, START_LABEL, &mut s2));
        assert!(status_text(&s).contains(UNSUPPORTED_TEXT));
        start(&mut s);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn none_support_becomes_unsupported() {
        let mut s = AppLifecycleShowState::default();
        apply_init(
            &mut s,
            SupportView {
                events: [false; 7],
                active: false,
                visible: false,
            },
            CurrentView::default(),
        );
        assert_eq!(s.outcome, Some(Outcome::Unsupported));
    }

    #[test]
    fn frozen_kinds_show_not_available_not_zero() {
        let mut s = AppLifecycleShowState::default();
        let mut support = support_all();
        for k in [
            EventKind::Frozen,
            EventKind::Unfrozen,
            EventKind::RestoredFromCache,
        ] {
            support.events[k.index()] = false;
        }
        apply_init(&mut s, support, CurrentView::default());
        let text = status_text(&s);
        assert!(text.contains(&format!("凍結された: {NOT_AVAILABLE}")));
        assert!(text.contains(&format!("凍結から戻った: {NOT_AVAILABLE}")));
        assert!(text.contains(&format!("ページのキャッシュから戻った: {NOT_AVAILABLE}")));
        assert!(text.contains("見えなくなった: 0 回"));
    }

    #[test]
    fn unreadable_state_shows_not_available() {
        let mut s = AppLifecycleShowState::default();
        let mut support = support_all();
        support.active = false;
        apply_init(&mut s, support, CurrentView::default());
        let text = status_text(&s);
        assert!(text.contains(&format!("{ACTIVE_PREFIX}{NOT_AVAILABLE}")));
    }

    #[test]
    fn read_failed_is_not_shown_as_active_or_visible() {
        let mut s = AppLifecycleShowState::default();
        apply_init(
            &mut s,
            support_all(),
            CurrentView {
                active: ActiveView::ReadFailed,
                visible: VisibleView::ReadFailed,
            },
        );
        let text = status_text(&s);
        assert!(text.contains(&format!("{ACTIVE_PREFIX}{READ_FAILED}")));
        assert!(text.contains(&format!("{VISIBLE_PREFIX}{READ_FAILED}")));
        assert!(!text.contains(&format!("{ACTIVE_PREFIX}{ACTIVE_STATE}")));
        assert!(!text.contains(&format!("{VISIBLE_PREFIX}{VISIBLE_STATE}")));
    }

    #[test]
    fn snapshot_renders_recent_and_counts() {
        let mut s = receiving();
        apply_init(&mut s, support_all(), CurrentView::default());
        apply_snapshot(
            &mut s,
            1,
            vec![(2, EventKind::Activated), (1, EventKind::Deactivated)],
            [1, 1, 0, 0, 0, 0, 0],
            Some(CurrentView {
                active: ActiveView::Active,
                visible: VisibleView::Visible,
            }),
        );
        let text = status_text(&s);
        assert!(text.contains("#2 操作の対象になった"));
        assert!(text.contains("操作の対象から外れた: 1 回"));
        assert!(text.contains(&format!("{ACTIVE_PREFIX}{ACTIVE_STATE}")));
        assert!(text.contains(&format!("{VISIBLE_PREFIX}{VISIBLE_STATE}")));
    }

    #[test]
    fn left_is_normal() {
        let mut s = receiving();
        mark_left(&mut s, 1);
        assert_eq!(s.outcome, Some(Outcome::Left));
        assert_eq!(error_text(&s), "");
        assert!(status_text(&s).contains(LEFT_STATUS));
    }

    #[test]
    fn button_toggles() {
        let mut s = AppLifecycleShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, START_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Starting);
        let screen = current_screen(&s);
        assert!(click_button(&screen, STOP_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Idle);
    }
}
