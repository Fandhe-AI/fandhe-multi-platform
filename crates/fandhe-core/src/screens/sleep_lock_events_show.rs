//! Phase 11（端末機能の拡充ループ、親 #167）型サンプル「スリープ・ロックの通知」（Issue #810）。
//! 「受け取りを始める」で OS のスリープ・復帰・画面の消灯と点灯・ロックと解除の通知を購読し、
//! 受信順の番号つきで直近 8 件と種類ごとの回数を出す 1 画面。`FANDHE_PROXY_APP=sleep_lock_events_show`
//! で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod sleep_lock_events_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/sleep_lock_events.rs` が [`take_init`] / [`take_start`] で要求を観測し、[`apply_init`] /
//! [`finish_start`] / [`apply_snapshot`] / [`mark_failed`] / [`mark_left`] で結果を注入する。
//! device の型はドライバが本画面の鏡像の型へ写す。
//!
//! **起動時には購読しない**（ボタン起点）。全対象 OS の状態・文言をここで確定済みで、後続 OS の
//! 実装（Android: #811）は本画面を触らない。表示は [`SupportView`]（OS が知らせる種類と読める状態）で
//! 決め、OS 名では分岐しない。OS ごとの違い:
//! macOS はロックを取れず今の状態も読めない、Windows は画面の消灯・点灯を取れずロック状態は読める、
//! Android はスリープとロックした瞬間を取れず画面とロックの状態は読める。
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
//! Starting --finish_start(Err(PermissionDenied))--> Idle + 「許可されていないため受け取れません」（Web のみ。再開始できる）
//! ```

use crate::{Screen, WidgetId};

const TOGGLE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 画面タイトル。
pub const TITLE: &str = "スリープ・ロックの通知";
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
/// 許可なし（Web の `idle-detection` の拒否）。
pub const PERMISSION_DENIED_TEXT: &str = "許可されていないため受け取れません";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境ではスリープ・ロックの通知を受け取れません";
/// OS が知らせない種類・読めない状態。
pub const NOT_AVAILABLE: &str = "この OS では取れない";
/// 今の状態の読み取り失敗。
pub const READ_FAILED: &str = "読み取れませんでした";
/// 状態ラベル: ロック中。
pub const LOCKED_STATE: &str = "ロックされている";
/// 状態ラベル: ロックなし。
pub const UNLOCKED_STATE: &str = "ロックされていない";
/// 状態ラベル: 画面点灯。
pub const SCREEN_ON_STATE: &str = "画面が点いている";
/// 状態ラベル: 画面消灯。
pub const SCREEN_OFF_STATE: &str = "画面が消えている";
/// 状態ラベル: 不明。
pub const UNKNOWN_STATE: &str = "不明";
/// 今のロック状態の見出し。
pub const LOCK_PREFIX: &str = "ロックの状態: ";
/// 今の画面状態の見出し。
pub const SCREEN_PREFIX: &str = "画面の状態: ";
/// 直近一覧の見出し。
pub const RECENT_HEADING: &str = "直近の出来事";
/// 直近一覧が空のとき。
pub const RECENT_EMPTY: &str = "まだ届いていません";
/// 直近に残す件数の上限（device 側の `MAX_RECENT` と揃える）。
pub const MAX_RECENT: usize = 8;

/// 出来事の種類（device の `SleepLockEvent` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// スリープに入る。
    SystemWillSleep,
    /// スリープから戻った。
    SystemDidWake,
    /// 画面が消えた。
    ScreenOff,
    /// 画面が点いた。
    ScreenOn,
    /// ロックされた。
    Locked,
    /// ロックが解除された。
    Unlocked,
}

/// 表示順の全種類。
pub const ALL_KINDS: [EventKind; 6] = [
    EventKind::SystemWillSleep,
    EventKind::SystemDidWake,
    EventKind::ScreenOff,
    EventKind::ScreenOn,
    EventKind::Locked,
    EventKind::Unlocked,
];

impl EventKind {
    /// [`ALL_KINDS`] 内の位置。
    pub fn index(self) -> usize {
        ALL_KINDS.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// 表示ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Self::SystemWillSleep => "スリープに入る",
            Self::SystemDidWake => "スリープから戻った",
            Self::ScreenOff => "画面が消えた",
            Self::ScreenOn => "画面が点いた",
            Self::Locked => "ロックされた",
            Self::Unlocked => "ロックが解除された",
        }
    }
}

/// OS が知らせる種類と読める状態の鏡像。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportView {
    /// 種類ごとに OS が通知を出すか（[`ALL_KINDS`] の順）。
    pub events: [bool; 6],
    /// 今のロック状態を読めるか。
    pub lock: bool,
    /// 今の画面状態を読めるか。
    pub screen: bool,
}

/// 今のロック状態の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockView {
    /// ロック中。
    Locked,
    /// ロックなし。
    Unlocked,
    /// OS が判定できない。
    Unknown,
    /// 読み取りに失敗した。
    ReadFailed,
    /// この OS では読めない。
    #[default]
    NotAvailable,
}

/// 今の画面状態の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenView {
    /// 点いている。
    On,
    /// 消えている。
    Off,
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
    /// ロック。
    pub lock: LockView,
    /// 画面。
    pub screen: ScreenView,
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
    /// 許可なし（Web のみ。native は返さない）。
    PermissionDenied,
}

/// 開始の失敗の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFailure {
    /// 非対応。
    Unsupported,
    /// 許可されなかった（Web のみ）。
    PermissionDenied,
    /// 開始に失敗した。
    Failed,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SleepLockEventsShowState {
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
    pub counts: [u64; 6],
    /// 結果の種類。
    pub outcome: Option<Outcome>,
}

/// 開始ボタン。`Idle` で非対応でないときだけ受け付け、連打しても世代は 1 回分だけ進む。
pub fn start(state: &mut SleepLockEventsShowState) {
    if state.phase != Phase::Idle || state.outcome == Some(Outcome::Unsupported) {
        return;
    }
    state.phase = Phase::Starting;
    state.generation = state.generation.wrapping_add(1);
    state.outcome = None;
    state.recent.clear();
    state.counts = [0; 6];
}

/// 停止ボタン。利用者の停止は正常で、記録は残す。`Idle` では何もしない。
pub fn stop(state: &mut SleepLockEventsShowState) {
    if state.phase == Phase::Idle {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::Stopped);
}

/// 開始 / 停止ボタンのハンドラ。
pub fn toggle(state: &mut SleepLockEventsShowState) {
    if state.phase == Phase::Idle {
        start(state);
    } else {
        stop(state);
    }
}

/// 初回の読み出し要求を観測する（状態は変えない）。`apply_init` が届くまで true。
pub fn take_init(state: &SleepLockEventsShowState) -> bool {
    state.support.is_none()
}

/// 開始要求の世代を観測する（状態は変えない）。
pub fn take_start(state: &SleepLockEventsShowState) -> Option<u64> {
    (state.phase == Phase::Starting).then_some(state.generation)
}

/// 初回の読み出し結果を反映する。何も知らせない OS は非対応にする。
pub fn apply_init(
    state: &mut SleepLockEventsShowState,
    support: SupportView,
    current: CurrentView,
) {
    state.support = Some(support);
    state.current = current;
    if !support.events.iter().any(|e| *e) && state.phase == Phase::Idle {
        state.outcome = Some(Outcome::Unsupported);
    }
}

/// 開始の結果を反映する。`Starting` かつ世代が一致するときだけ。
pub fn finish_start(
    state: &mut SleepLockEventsShowState,
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
                StartFailure::PermissionDenied => Outcome::PermissionDenied,
                StartFailure::Failed => Outcome::StartFailed,
            });
        }
    }
}

/// 出来事の記録と今の状態を反映する。`Receiving` かつ世代が一致するときだけ（停止後・古い世代は捨てる）。
pub fn apply_snapshot(
    state: &mut SleepLockEventsShowState,
    generation: u64,
    recent: Vec<(u64, EventKind)>,
    counts: [u64; 6],
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
pub fn mark_failed(state: &mut SleepLockEventsShowState, generation: u64) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::RuntimeFailed);
}

/// 画面を離れて止めたことを反映する（正常。Activity 再生成などで使う）。
pub fn mark_left(state: &mut SleepLockEventsShowState, generation: u64) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::Left);
}

fn lock_text(v: LockView) -> &'static str {
    match v {
        LockView::Locked => LOCKED_STATE,
        LockView::Unlocked => UNLOCKED_STATE,
        LockView::Unknown => UNKNOWN_STATE,
        LockView::ReadFailed => READ_FAILED,
        LockView::NotAvailable => NOT_AVAILABLE,
    }
}

fn screen_text(v: ScreenView) -> &'static str {
    match v {
        ScreenView::On => SCREEN_ON_STATE,
        ScreenView::Off => SCREEN_OFF_STATE,
        ScreenView::Unknown => UNKNOWN_STATE,
        ScreenView::ReadFailed => READ_FAILED,
        ScreenView::NotAvailable => NOT_AVAILABLE,
    }
}

/// 状態の案内（結果・種類ごとの回数・今の状態・直近の出来事）。失敗の文言は含めない。
pub fn status_text(state: &SleepLockEventsShowState) -> String {
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
        lines.push(format!("{LOCK_PREFIX}{}", lock_text(state.current.lock)));
        lines.push(format!(
            "{SCREEN_PREFIX}{}",
            screen_text(state.current.screen)
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
pub fn error_text(state: &SleepLockEventsShowState) -> String {
    match (state.phase, state.outcome) {
        (Phase::Idle, Some(Outcome::StartFailed)) => START_FAILED_TEXT.to_string(),
        (Phase::Idle, Some(Outcome::RuntimeFailed)) => RUNTIME_FAILED_TEXT.to_string(),
        (Phase::Idle, Some(Outcome::PermissionDenied)) => PERMISSION_DENIED_TEXT.to_string(),
        _ => String::new(),
    }
}

/// 画面を組み立てる。非対応のときは開始ボタンを出さない。
pub fn current_screen(state: &SleepLockEventsShowState) -> Screen<SleepLockEventsShowState> {
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
            events: [true; 6],
            lock: true,
            screen: true,
        }
    }

    fn receiving() -> SleepLockEventsShowState {
        let mut s = SleepLockEventsShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Ok(()));
        s
    }

    #[test]
    fn startup_idle_and_does_not_subscribe() {
        let s = SleepLockEventsShowState::default();
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(take_start(&s), None);
        assert!(take_init(&s));
        let mut s = s;
        apply_init(&mut s, support_all(), CurrentView::default());
        assert!(!take_init(&s));
    }

    #[test]
    fn start_spam_advances_generation_once() {
        let mut s = SleepLockEventsShowState::default();
        start(&mut s);
        start(&mut s);
        start(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_start(&s), Some(1));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = SleepLockEventsShowState::default();
        start(&mut s);
        finish_start(&mut s, 99, Ok(()));
        assert_eq!(s.phase, Phase::Starting);
        finish_start(&mut s, 1, Ok(()));
        apply_snapshot(
            &mut s,
            99,
            vec![(1, EventKind::Locked)],
            [0, 0, 0, 0, 1, 0],
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
            vec![(1, EventKind::ScreenOn)],
            [0, 0, 0, 1, 0, 0],
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
        let mut s = SleepLockEventsShowState::default();
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
        let mut s = SleepLockEventsShowState::default();
        apply_init(
            &mut s,
            SupportView {
                events: [false; 6],
                lock: false,
                screen: false,
            },
            CurrentView::default(),
        );
        assert_eq!(s.outcome, Some(Outcome::Unsupported));
    }

    #[test]
    fn unsupported_kinds_show_not_available() {
        let mut s = SleepLockEventsShowState::default();
        let mut support = support_all();
        support.events[EventKind::Locked.index()] = false;
        support.lock = false;
        apply_init(&mut s, support, CurrentView::default());
        let text = status_text(&s);
        assert!(text.contains(&format!("ロックされた: {NOT_AVAILABLE}")));
        assert!(text.contains(&format!("{LOCK_PREFIX}{NOT_AVAILABLE}")));
        assert!(text.contains("画面が消えた: 0 回"));
    }

    #[test]
    fn read_failed_is_not_shown_as_unlocked() {
        let mut s = SleepLockEventsShowState::default();
        apply_init(
            &mut s,
            support_all(),
            CurrentView {
                lock: LockView::ReadFailed,
                screen: ScreenView::NotAvailable,
            },
        );
        let text = status_text(&s);
        assert!(text.contains(READ_FAILED));
        assert!(!text.contains(UNLOCKED_STATE));
    }

    #[test]
    fn snapshot_renders_recent_and_counts() {
        let mut s = receiving();
        apply_init(&mut s, support_all(), CurrentView::default());
        apply_snapshot(
            &mut s,
            1,
            vec![(2, EventKind::ScreenOn), (1, EventKind::ScreenOff)],
            [0, 0, 1, 1, 0, 0],
            Some(CurrentView {
                lock: LockView::Unlocked,
                screen: ScreenView::On,
            }),
        );
        let text = status_text(&s);
        assert!(text.contains("#2 画面が点いた"));
        assert!(text.contains("画面が消えた: 1 回"));
        assert!(text.contains(UNLOCKED_STATE));
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
    fn permission_denied_is_distinct_and_restartable() {
        let mut s = SleepLockEventsShowState::default();
        start(&mut s);
        finish_start(&mut s, 99, Err(StartFailure::PermissionDenied));
        assert_eq!(s.phase, Phase::Starting);
        finish_start(&mut s, 1, Err(StartFailure::PermissionDenied));
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(error_text(&s), PERMISSION_DENIED_TEXT);
        assert!(!error_text(&s).contains(START_FAILED_TEXT));
        let screen = current_screen(&s);
        assert!(click_button(&screen, START_LABEL, &mut s));
        assert_eq!(
            (s.phase, s.generation, s.outcome),
            (Phase::Starting, 2, None)
        );
    }

    #[test]
    fn button_toggles() {
        let mut s = SleepLockEventsShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, START_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Starting);
        let screen = current_screen(&s);
        assert!(click_button(&screen, STOP_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Idle);
    }
}
