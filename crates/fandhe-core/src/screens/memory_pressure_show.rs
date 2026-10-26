//! Phase 11（端末機能の拡充ループ、親 #179）型サンプル「メモリ逼迫・負荷」（Issue #911）。
//! 「受け取りを始める」で OS のメモリ逼迫（Web は CPU 負荷）の通知を購読し、受信順の番号つきで
//! 直近 8 件と種類ごとの回数を出す 1 画面。`FANDHE_PROXY_APP=memory_pressure_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod memory_pressure_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/memory_pressure.rs` が [`take_init`] / [`take_start`] で要求を観測し、[`apply_init`] /
//! [`finish_start`] / [`apply_snapshot`] / [`mark_failed`] / [`mark_left`] で結果を注入する。
//! device の型はドライバが本画面の鏡像の型へ写す。
//!
//! **起動時には購読しない**（ボタン起点）。全対象 OS の状態・文言をここで確定済みで、後続 OS の
//! 実装（iOS・Android・Web）は本画面を触らない。表示は [`SupportView`]（OS が知らせる種類と読める状態）で
//! 決め、OS 名では分岐しない。OS が知らせない種類は「この OS では取れない」と出し、「0 回」とは出さない。
//! 最初の通知までは「まだ届いていません」と出し、「平常」を仮定しない。Web の CPU の段階は
//! メモリではないことを明記して出す。
//!
//! # 世代
//!
//! 開始のたびに `generation` を進め、停止後・差し替え後に届いた完了通知は世代で捨てる。停止は失敗ではない。
//!
//! ```text
//! Idle --「受け取りを始める」--> Starting(gen += 1) --finish_start(Ok)--> Receiving
//! Receiving --apply_snapshot(合図・今の状態)--> Receiving
//! Starting/Receiving --「止める」--> Idle（記録は残し「止めました」）
//! Receiving --mark_failed--> Idle + 「受け取りが止まりました」
//! Receiving --mark_left--> Idle + 「画面を離れたため止めました」（正常）
//! ```

use crate::{Screen, WidgetId};

const TOGGLE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 画面タイトル。
pub const TITLE: &str = "メモリ逼迫・負荷";
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
/// 許可なし（Web の Permissions Policy 拒否）。非対応とは別の文言。
pub const NOT_ALLOWED_TEXT: &str = "この文書ではメモリ逼迫・負荷の通知が許可されていません";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境ではメモリ逼迫・負荷の通知を受け取れません";
/// OS が知らせない種類・読めない状態。
pub const NOT_AVAILABLE: &str = "この OS では取れない";
/// 今の状態の読み取り失敗。
pub const READ_FAILED: &str = "読み取れませんでした";
/// 今の状態の見出し。
pub const CURRENT_PREFIX: &str = "今のメモリの状態: ";
/// 状態ラベル: 不足。
pub const LOW_STATE: &str = "不足";
/// 状態ラベル: 不足ではない。
pub const NOT_LOW_STATE: &str = "不足ではない";
/// 状態ラベル: 余裕あり。
pub const HIGH_STATE: &str = "余裕あり";
/// 状態ラベル: 中間帯。
pub const MIDDLE_STATE: &str = "中間（不足でも余裕でもない）";
/// 直近一覧の見出し。
pub const RECENT_HEADING: &str = "直近の合図";
/// 直近一覧が空のとき。
pub const RECENT_EMPTY: &str = "まだ届いていません";
/// 直近に残す件数の上限（device 側の `MAX_RECENT` と揃える）。
pub const MAX_RECENT: usize = 8;
/// 種類の枠の数（device の `ALL_EVENTS` と揃える）。
pub const KIND_COUNT: usize = 11;

/// 合図の種類（device の `PressureEvent` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// メモリ警告（iOS）。
    MemoryWarning,
    /// メモリ逼迫の段階: 平常。
    Normal,
    /// メモリ逼迫の段階: 警告。
    Warning,
    /// メモリ逼迫の段階: 危機的。
    Critical,
    /// trim 段階（Android）。値をそのまま持つ。
    Trim(i32),
    /// メモリ不足（Windows の Low）。
    Low,
    /// メモリに余裕（Windows の High）。
    High,
    /// CPU の負荷: nominal（Web）。
    CpuNominal,
    /// CPU の負荷: fair。
    CpuFair,
    /// CPU の負荷: serious。
    CpuSerious,
    /// CPU の負荷: critical。
    CpuCritical,
}

/// 回数の一覧に出す枠ごとのラベル（添字は [`EventKind::index`]）。Trim は値違いでも 1 枠。
pub const SLOT_LABELS: [&str; KIND_COUNT] = [
    "メモリ警告",
    "メモリ逼迫の段階: 平常",
    "メモリ逼迫の段階: 警告",
    "メモリ逼迫の段階: 危機的",
    "trim 段階の通知",
    "メモリ不足（Low）",
    "メモリに余裕（High）",
    "CPU の負荷: nominal",
    "CPU の負荷: fair",
    "CPU の負荷: serious",
    "CPU の負荷: critical",
];

impl EventKind {
    /// 回数の配列の添字（device の `PressureEvent::index` と同じ並び）。
    pub fn index(self) -> usize {
        match self {
            Self::MemoryWarning => 0,
            Self::Normal => 1,
            Self::Warning => 2,
            Self::Critical => 3,
            Self::Trim(_) => 4,
            Self::Low => 5,
            Self::High => 6,
            Self::CpuNominal => 7,
            Self::CpuFair => 8,
            Self::CpuSerious => 9,
            Self::CpuCritical => 10,
        }
    }

    /// 直近一覧に出すラベル。Trim は値を含める。
    pub fn label(self) -> String {
        match self {
            Self::Trim(n) => format!("trim 段階 {n}"),
            other => SLOT_LABELS[other.index()].to_string(),
        }
    }
}

/// OS が知らせる種類と読める状態の鏡像。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportView {
    /// 種類ごとに OS が通知を出すか（[`EventKind::index`] の順）。
    pub events: [bool; KIND_COUNT],
    /// 今の状態を読めるか。
    pub current: bool,
}

/// 今のメモリの状態の表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CurrentView {
    /// 不足。
    Low,
    /// 不足ではない。
    NotLow,
    /// 余裕あり。
    High,
    /// 中間帯。
    Middle,
    /// 読み取りに失敗した。
    ReadFailed,
    /// この OS では読めない。
    #[default]
    NotAvailable,
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
    /// 許可なし。
    NotAllowed,
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
    /// 許可なし。
    NotAllowed,
    /// 開始に失敗した。
    Failed,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryPressureShowState {
    /// 進行状態。
    pub phase: Phase,
    /// 直近に立てた開始要求の世代。
    pub generation: u64,
    /// OS が知らせる種類と読める状態。`None` は未取得（初回の読み出し前）。
    pub support: Option<SupportView>,
    /// 今の状態。
    pub current: CurrentView,
    /// 直近の合図（新しい順、`(通し番号, 種類)`）。
    pub recent: Vec<(u64, EventKind)>,
    /// 種類ごとの回数（[`EventKind::index`] の順）。
    pub counts: [u64; KIND_COUNT],
    /// 結果の種類。
    pub outcome: Option<Outcome>,
}

/// 開始ボタン。`Idle` で非対応でないときだけ受け付け、連打しても世代は 1 回分だけ進む。
pub fn start(state: &mut MemoryPressureShowState) {
    if state.phase != Phase::Idle || state.outcome == Some(Outcome::Unsupported) {
        return;
    }
    state.phase = Phase::Starting;
    state.generation = state.generation.wrapping_add(1);
    state.outcome = None;
    state.recent.clear();
    state.counts = [0; KIND_COUNT];
}

/// 停止ボタン。利用者の停止は正常で、記録は残す。`Idle` では何もしない。
pub fn stop(state: &mut MemoryPressureShowState) {
    if state.phase == Phase::Idle {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::Stopped);
}

/// 開始 / 停止ボタンのハンドラ。
pub fn toggle(state: &mut MemoryPressureShowState) {
    if state.phase == Phase::Idle {
        start(state);
    } else {
        stop(state);
    }
}

/// 初回の読み出し要求を観測する（状態は変えない）。`apply_init` が届くまで true。
pub fn take_init(state: &MemoryPressureShowState) -> bool {
    state.support.is_none()
}

/// 開始要求の世代を観測する（状態は変えない）。
pub fn take_start(state: &MemoryPressureShowState) -> Option<u64> {
    (state.phase == Phase::Starting).then_some(state.generation)
}

/// 初回の読み出し結果を反映する。何も知らせない OS は非対応にする。
pub fn apply_init(state: &mut MemoryPressureShowState, support: SupportView, current: CurrentView) {
    state.support = Some(support);
    state.current = current;
    if !support.events.iter().any(|e| *e) && state.phase == Phase::Idle {
        state.outcome = Some(Outcome::Unsupported);
    }
}

/// 開始の結果を反映する。`Starting` かつ世代が一致するときだけ。
pub fn finish_start(
    state: &mut MemoryPressureShowState,
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
                StartFailure::NotAllowed => Outcome::NotAllowed,
                StartFailure::Failed => Outcome::StartFailed,
            });
        }
    }
}

/// 合図の記録と今の状態を反映する。`Receiving` かつ世代が一致するときだけ（停止後・古い世代は捨てる）。
pub fn apply_snapshot(
    state: &mut MemoryPressureShowState,
    generation: u64,
    recent: Vec<(u64, EventKind)>,
    counts: [u64; KIND_COUNT],
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
pub fn mark_failed(state: &mut MemoryPressureShowState, generation: u64) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::RuntimeFailed);
}

/// 画面を離れて止めたことを反映する（正常。Activity 再生成などで使う）。
pub fn mark_left(state: &mut MemoryPressureShowState, generation: u64) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::Left);
}

fn current_text(v: CurrentView) -> &'static str {
    match v {
        CurrentView::Low => LOW_STATE,
        CurrentView::NotLow => NOT_LOW_STATE,
        CurrentView::High => HIGH_STATE,
        CurrentView::Middle => MIDDLE_STATE,
        CurrentView::ReadFailed => READ_FAILED,
        CurrentView::NotAvailable => NOT_AVAILABLE,
    }
}

/// 状態の案内（結果・種類ごとの回数・今の状態・直近の合図）。失敗の文言は含めない。
pub fn status_text(state: &MemoryPressureShowState) -> String {
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
        for (i, label) in SLOT_LABELS.iter().enumerate() {
            if support.events[i] {
                lines.push(format!("{label}: {} 回", state.counts[i]));
            } else {
                lines.push(format!("{label}: {NOT_AVAILABLE}"));
            }
        }
        lines.push(format!("{CURRENT_PREFIX}{}", current_text(state.current)));
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
pub fn error_text(state: &MemoryPressureShowState) -> String {
    match (state.phase, state.outcome) {
        (Phase::Idle, Some(Outcome::StartFailed)) => START_FAILED_TEXT.to_string(),
        (Phase::Idle, Some(Outcome::NotAllowed)) => NOT_ALLOWED_TEXT.to_string(),
        (Phase::Idle, Some(Outcome::RuntimeFailed)) => RUNTIME_FAILED_TEXT.to_string(),
        _ => String::new(),
    }
}

/// 画面を組み立てる。非対応のときは開始ボタンを出さない。
pub fn current_screen(state: &MemoryPressureShowState) -> Screen<MemoryPressureShowState> {
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
            events: [true; KIND_COUNT],
            current: true,
        }
    }

    fn receiving() -> MemoryPressureShowState {
        let mut s = MemoryPressureShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Ok(()));
        s
    }

    #[test]
    fn startup_idle_and_does_not_subscribe() {
        let s = MemoryPressureShowState::default();
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(take_start(&s), None);
        assert!(take_init(&s));
        let mut s = s;
        apply_init(&mut s, support_all(), CurrentView::default());
        assert!(!take_init(&s));
    }

    #[test]
    fn start_spam_advances_generation_once() {
        let mut s = MemoryPressureShowState::default();
        start(&mut s);
        start(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_start(&s), Some(1));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = MemoryPressureShowState::default();
        start(&mut s);
        finish_start(&mut s, 99, Ok(()));
        assert_eq!(s.phase, Phase::Starting);
        finish_start(&mut s, 1, Ok(()));
        let mut counts = [0; KIND_COUNT];
        counts[0] = 1;
        apply_snapshot(
            &mut s,
            99,
            vec![(1, EventKind::MemoryWarning)],
            counts,
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
        apply_snapshot(&mut s, 1, vec![(1, EventKind::Low)], [0; KIND_COUNT], None);
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
    fn unsupported_hides_button_and_start_is_ignored() {
        let mut s = MemoryPressureShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Err(StartFailure::Unsupported));
        assert_eq!(s.outcome, Some(Outcome::Unsupported));
        let screen = current_screen(&s);
        let mut s2 = s.clone();
        assert!(!click_button(&screen, START_LABEL, &mut s2));
        assert!(status_text(&s).contains(UNSUPPORTED_TEXT));
        start(&mut s);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn not_allowed_and_unsupported_use_different_text() {
        let mut s = MemoryPressureShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Err(StartFailure::NotAllowed));
        assert_eq!(s.outcome, Some(Outcome::NotAllowed));
        assert_eq!(error_text(&s), NOT_ALLOWED_TEXT);
        assert!(!status_text(&s).contains(UNSUPPORTED_TEXT));
        assert_ne!(NOT_ALLOWED_TEXT, UNSUPPORTED_TEXT);
        // 許可なしは再試行できる（開始ボタンが残る）。
        assert!(click_button(&current_screen(&s), START_LABEL, &mut s));
    }

    #[test]
    fn start_failure_shows_error() {
        let mut s = MemoryPressureShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Err(StartFailure::Failed));
        assert_eq!(error_text(&s), START_FAILED_TEXT);
    }

    #[test]
    fn none_support_becomes_unsupported() {
        let mut s = MemoryPressureShowState::default();
        apply_init(
            &mut s,
            SupportView {
                events: [false; KIND_COUNT],
                current: false,
            },
            CurrentView::default(),
        );
        assert_eq!(s.outcome, Some(Outcome::Unsupported));
    }

    #[test]
    fn unsupported_kinds_show_not_available_not_zero() {
        let mut s = MemoryPressureShowState::default();
        let mut support = SupportView {
            events: [false; KIND_COUNT],
            current: false,
        };
        support.events[EventKind::Normal.index()] = true;
        apply_init(&mut s, support, CurrentView::default());
        let text = status_text(&s);
        assert!(text.contains("メモリ逼迫の段階: 平常: 0 回"));
        assert!(text.contains(&format!("メモリ不足（Low）: {NOT_AVAILABLE}")));
        assert!(text.contains(&format!("{CURRENT_PREFIX}{NOT_AVAILABLE}")));
        assert!(text.contains(RECENT_EMPTY));
        // 「平常」を仮定しない。
        assert!(!text.contains(&format!("{CURRENT_PREFIX}{NOT_LOW_STATE}")));
    }

    #[test]
    fn read_failed_is_not_shown_as_normal() {
        let mut s = MemoryPressureShowState::default();
        apply_init(&mut s, support_all(), CurrentView::ReadFailed);
        let text = status_text(&s);
        assert!(text.contains(&format!("{CURRENT_PREFIX}{READ_FAILED}")));
        assert!(!text.contains(&format!("{CURRENT_PREFIX}{NOT_LOW_STATE}")));
    }

    #[test]
    fn trim_label_includes_value_and_counts_one_slot() {
        assert_eq!(EventKind::Trim(80).label(), "trim 段階 80");
        assert_eq!(EventKind::Trim(-1).index(), EventKind::Trim(80).index());
        let mut s = receiving();
        apply_init(&mut s, support_all(), CurrentView::default());
        let mut counts = [0; KIND_COUNT];
        counts[4] = 2;
        apply_snapshot(
            &mut s,
            1,
            vec![(2, EventKind::Trim(999)), (1, EventKind::Trim(5))],
            counts,
            Some(CurrentView::NotLow),
        );
        let text = status_text(&s);
        assert!(text.contains("#2 trim 段階 999"));
        assert!(text.contains("trim 段階の通知: 2 回"));
        assert!(text.contains(&format!("{CURRENT_PREFIX}{NOT_LOW_STATE}")));
    }

    #[test]
    fn cpu_labels_say_cpu_not_memory() {
        for k in [
            EventKind::CpuNominal,
            EventKind::CpuFair,
            EventKind::CpuSerious,
            EventKind::CpuCritical,
        ] {
            assert!(k.label().starts_with("CPU の負荷"));
        }
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
        let mut s = MemoryPressureShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, START_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Starting);
        let screen = current_screen(&s);
        assert!(click_button(&screen, STOP_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Idle);
    }
}
