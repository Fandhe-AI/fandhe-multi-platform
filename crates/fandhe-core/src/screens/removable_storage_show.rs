//! Phase 11（端末機能の拡充ループ、親 #175）型サンプル「外部ストレージの着脱」（Issue #858）。
//! 「始める」で OS の着脱通知（マウントされた・取り外しの直前・取り外された・その他の変化）を購読し、
//! 受信順の番号つきで直近 8 件と種類ごとの回数を出す。今のボリューム一覧（名前・種類・状態）は
//! 画面を開いた時・通知が届くたび・「再取得」で OS から読み直して丸ごと置き換える 1 画面。
//! `FANDHE_PROXY_APP=removable_storage_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod removable_storage_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/removable_storage.rs` が [`take_init`] / [`take_start`] / [`take_refresh`] で要求を観測し、
//! [`apply_init`] / [`apply_list`] / [`finish_start`] / [`apply_snapshot`] / [`mark_failed`] で結果を
//! 注入する。device の型はドライバが本画面の鏡像の型へ写す。
//!
//! **起動時には購読しない**（ボタン起点）。全対象 OS の状態・文言をここで確定済みで、後続 OS の
//! 実装（Android: #859）は本画面を触らない。表示は [`SupportView`]（OS が知らせる種類）で決め、
//! OS 名では分岐しない。通知の種類が全部 false でも一覧が読める OS は非対応にしない。
//! Android API 30 未満は通知だけ取れず（[`Outcome::EventsUnavailable`]）、一覧は出す。
//!
//! # 世代
//!
//! 開始のたびに `generation` を進め、停止後・差し替え後に届いた完了通知は世代で捨てる。停止は失敗ではない。
//! 一覧の読み直しは `list_request` / `list_applied` で照合し、古い結果は反映しない。
//!
//! ```text
//! Idle --「始める」--> Starting(gen += 1) --finish_start(Ok)--> Receiving
//! Receiving --apply_snapshot(出来事・一覧)--> Receiving
//! Starting/Receiving --「止める」--> Idle（記録は残し「止めました」）
//! Receiving --mark_failed--> Idle + 「受け取りが止まりました」
//! ```

use crate::{Screen, WidgetId};

const TOGGLE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const REFRESH_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 画面タイトル。
pub const TITLE: &str = "外部ストレージの着脱";
/// 開始ボタン。
pub const START_LABEL: &str = "始める";
/// 停止ボタン。
pub const STOP_LABEL: &str = "止める";
/// 再取得ボタン。
pub const REFRESH_LABEL: &str = "再取得";
/// 開始処理中。
pub const STARTING_STATUS: &str = "開始中…";
/// 受信中。
pub const RECEIVING_STATUS: &str = "受け取り中";
/// 利用者の停止（正常）。
pub const STOPPED_STATUS: &str = "止めました";
/// 開始失敗。
pub const START_FAILED_TEXT: &str = "受け取りを始められませんでした";
/// 実行中の失敗。
pub const RUNTIME_FAILED_TEXT: &str = "受け取りが止まりました";
/// 一覧の読み取り失敗。
pub const LIST_FAILED_TEXT: &str = "一覧を読み取れませんでした";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境では外部ストレージの着脱を扱えません";
/// OS が知らせない種類。
pub const NOT_AVAILABLE: &str = "この OS では取れない";
/// 通知だけ取れない OS 版（失敗の色にしない）。
pub const EVENTS_UNAVAILABLE_TEXT: &str = "着脱の通知: この OS 版では取れない";
/// 一覧が 0 件。
pub const VOLUMES_EMPTY: &str = "ボリュームはありません";
/// 名前なし。
pub const NO_NAME: &str = "（名前なし）";
/// 一覧の見出し。
pub const VOLUMES_HEADING: &str = "今のボリューム";
/// 直近一覧の見出し。
pub const RECENT_HEADING: &str = "直近の出来事";
/// 直近一覧が空のとき。
pub const RECENT_EMPTY: &str = "まだ届いていません";
/// 直近に残す件数の上限（device 側の `MAX_RECENT` と揃える）。
pub const MAX_RECENT: usize = 8;

/// 出来事の種類（device の `VolumeEvent` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// マウントされた。
    Mounted,
    /// 取り外しの直前。
    WillUnmount,
    /// 取り外された。
    Unmounted,
    /// その他の変化。
    Other,
}

/// 表示順の全種類。
pub const ALL_KINDS: [EventKind; 4] = [
    EventKind::Mounted,
    EventKind::WillUnmount,
    EventKind::Unmounted,
    EventKind::Other,
];

impl EventKind {
    /// [`ALL_KINDS`] 内の位置。
    pub fn index(self) -> usize {
        ALL_KINDS.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// 表示ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Self::Mounted => "マウントされた",
            Self::WillUnmount => "取り外しの直前",
            Self::Unmounted => "取り外された",
            Self::Other => "その他の変化",
        }
    }
}

/// OS が知らせる種類の鏡像。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportView {
    /// 種類ごとに OS が通知を出すか（[`ALL_KINDS`] の順）。
    pub events: [bool; 4],
}

/// ボリュームの種類の鏡像。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindView {
    /// 内蔵。
    Internal,
    /// 取り外し可能。
    Removable,
    /// 固定。
    Fixed,
    /// ネットワーク。
    Network,
    /// 光学ドライブ。
    Optical,
    /// RAM ディスク。
    RamDisk,
    /// その他。
    Other,
    /// 不明。
    Unknown,
}

impl KindView {
    /// 表示ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Self::Internal => "内蔵",
            Self::Removable => "取り外し可能",
            Self::Fixed => "固定",
            Self::Network => "ネットワーク",
            Self::Optical => "光学ドライブ",
            Self::RamDisk => "RAM ディスク",
            Self::Other => "その他",
            Self::Unknown => "不明",
        }
    }
}

/// ボリュームの状態の鏡像。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateView {
    /// マウント中。
    Mounted,
    /// 読み取り専用でマウント中。
    MountedReadOnly,
    /// メディアなし。
    NoMedia,
    /// 確認中。
    Checking,
    /// 取り出し中。
    Ejecting,
    /// マウントされていない。
    Unmounted,
    /// 取り外された。
    Removed,
    /// 読めない形式。
    Unmountable,
    /// 不明。
    Unknown,
}

impl StateView {
    /// 表示ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Self::Mounted => "マウント中",
            Self::MountedReadOnly => "読み取り専用でマウント中",
            Self::NoMedia => "メディアなし",
            Self::Checking => "確認中",
            Self::Ejecting => "取り出し中",
            Self::Unmounted => "マウントされていない",
            Self::Removed => "取り外された",
            Self::Unmountable => "読めない形式",
            Self::Unknown => "不明",
        }
    }
}

/// ボリューム 1 行の鏡像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeRow {
    /// 表示名。取れなければ `None`。
    pub name: Option<String>,
    /// 種類。
    pub kind: KindView,
    /// 状態。
    pub state: StateView,
}

/// 今のボリューム一覧の表示用。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ListView {
    /// まだ読んでいない。
    #[default]
    NotRead,
    /// 読めた（0 件を含む）。
    Listed(Vec<VolumeRow>),
    /// 読み取りに失敗した（0 件とは別）。
    ReadFailed,
    /// この環境では読めない。
    Unsupported,
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
    /// 開始失敗。
    StartFailed,
    /// 実行中の失敗。
    RuntimeFailed,
    /// 通知だけ取れない OS 版（一覧は読める。失敗ではない）。
    EventsUnavailable,
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
pub struct RemovableStorageShowState {
    /// 進行状態。
    pub phase: Phase,
    /// 直近に立てた開始要求の世代。
    pub generation: u64,
    /// OS が知らせる種類。`None` は未取得（初回の読み出し前）。
    pub support: Option<SupportView>,
    /// 今のボリューム一覧。
    pub list: ListView,
    /// 一覧の読み直し要求の通し番号。
    pub list_request: u64,
    /// 反映済みの読み直し要求の通し番号。
    pub list_applied: u64,
    /// 直近の出来事（新しい順、`(通し番号, 種類)`）。
    pub recent: Vec<(u64, EventKind)>,
    /// 種類ごとの回数（[`ALL_KINDS`] の順）。
    pub counts: [u64; 4],
    /// 結果の種類。
    pub outcome: Option<Outcome>,
}

/// 開始ボタン。`Idle` で非対応でないときだけ受け付け、連打しても世代は 1 回分だけ進む。
pub fn start(state: &mut RemovableStorageShowState) {
    if state.phase != Phase::Idle || state.outcome == Some(Outcome::Unsupported) {
        return;
    }
    state.phase = Phase::Starting;
    state.generation = state.generation.wrapping_add(1);
    state.outcome = None;
    state.recent.clear();
    state.counts = [0; 4];
}

/// 停止ボタン。利用者の停止は正常で、記録は残す。`Idle` では何もしない。
pub fn stop(state: &mut RemovableStorageShowState) {
    if state.phase == Phase::Idle {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::Stopped);
}

/// 開始 / 停止ボタンのハンドラ。
pub fn toggle(state: &mut RemovableStorageShowState) {
    if state.phase == Phase::Idle {
        start(state);
    } else {
        stop(state);
    }
}

/// 再取得ボタンのハンドラ。読み直しを要求するだけ（ドライバが [`take_refresh`] で観測する）。
pub fn refresh(state: &mut RemovableStorageShowState) {
    state.list_request = state.list_request.wrapping_add(1);
}

/// 初回の読み出し要求を観測する（状態は変えない）。`apply_init` が届くまで true。
pub fn take_init(state: &RemovableStorageShowState) -> bool {
    state.support.is_none()
}

/// 開始要求の世代を観測する（状態は変えない）。
pub fn take_start(state: &RemovableStorageShowState) -> Option<u64> {
    (state.phase == Phase::Starting).then_some(state.generation)
}

/// 一覧の読み直し要求を観測する（状態は変えない）。反映されるまで同じ番号を返す。
pub fn take_refresh(state: &RemovableStorageShowState) -> Option<u64> {
    (state.list_request != state.list_applied).then_some(state.list_request)
}

/// 初回の読み出し結果を反映する。何も知らせず一覧も読めない OS は非対応にする。
pub fn apply_init(state: &mut RemovableStorageShowState, support: SupportView, list: ListView) {
    state.support = Some(support);
    let no_events = !support.events.iter().any(|e| *e);
    if no_events && list == ListView::Unsupported && state.phase == Phase::Idle {
        state.outcome = Some(Outcome::Unsupported);
    }
    // 初回より後に要求された読み直しの結果を、古い初回の結果で上書きしない。
    if state.list_applied == 0 {
        state.list = list;
    }
}

/// 一覧の読み直し結果を反映する。`request` が反映済みより新しいときだけ。
pub fn apply_list(state: &mut RemovableStorageShowState, request: u64, list: ListView) {
    if request <= state.list_applied {
        return;
    }
    state.list = list;
    state.list_applied = request;
}

/// 開始の結果を反映する。`Starting` かつ世代が一致するときだけ。
pub fn finish_start(
    state: &mut RemovableStorageShowState,
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
                // 通知だけ取れない OS 版。一覧は読めるので画面全体は非対応にしない。
                StartFailure::Unsupported => Outcome::EventsUnavailable,
                StartFailure::Failed => Outcome::StartFailed,
            });
        }
    }
}

/// 出来事の記録と読み直した一覧を反映する。`Receiving` かつ世代が一致するときだけ
/// （停止後・古い世代は捨てる）。
pub fn apply_snapshot(
    state: &mut RemovableStorageShowState,
    generation: u64,
    recent: Vec<(u64, EventKind)>,
    counts: [u64; 4],
    list: Option<ListView>,
) {
    if state.phase != Phase::Receiving || state.generation != generation {
        return;
    }
    state.recent = recent;
    state.recent.truncate(MAX_RECENT);
    state.counts = counts;
    if let Some(l) = list {
        state.list = l;
    }
}

/// 実行中の失敗を反映する。`Starting` / `Receiving` かつ世代が一致するときだけ。
pub fn mark_failed(state: &mut RemovableStorageShowState, generation: u64) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.outcome = Some(Outcome::RuntimeFailed);
}

fn is_unsupported(state: &RemovableStorageShowState) -> bool {
    state.outcome == Some(Outcome::Unsupported) && state.phase == Phase::Idle
}

/// 状態の案内（結果・種類ごとの回数）。失敗の文言は含めない。
pub fn status_text(state: &RemovableStorageShowState) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        match (state.phase, state.outcome) {
            (Phase::Starting, _) => STARTING_STATUS,
            (Phase::Receiving, _) => RECEIVING_STATUS,
            (Phase::Idle, Some(Outcome::Stopped)) => STOPPED_STATUS,
            (Phase::Idle, Some(Outcome::EventsUnavailable)) => EVENTS_UNAVAILABLE_TEXT,
            (Phase::Idle, Some(Outcome::Unsupported)) => UNSUPPORTED_TEXT,
            _ => "",
        }
        .to_string(),
    );
    if let Some(support) = state.support.filter(|_| !is_unsupported(state)) {
        for kind in ALL_KINDS {
            let i = kind.index();
            if support.events[i] {
                lines.push(format!("{}: {} 回", kind.label(), state.counts[i]));
            } else {
                lines.push(format!("{}: {}", kind.label(), NOT_AVAILABLE));
            }
        }
    }
    lines.retain(|l| !l.is_empty());
    lines.join("\n")
}

/// 一覧と直近の出来事の案内。一覧は `名前（種類・状態）` を 1 行ずつ並べる。
/// 読み取り失敗は「ボリュームはありません」にしない。
pub fn list_text(state: &RemovableStorageShowState) -> String {
    if is_unsupported(state) {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push(VOLUMES_HEADING.to_string());
    match &state.list {
        ListView::NotRead | ListView::Unsupported => {}
        ListView::ReadFailed => lines.push(LIST_FAILED_TEXT.to_string()),
        ListView::Listed(rows) if rows.is_empty() => lines.push(VOLUMES_EMPTY.to_string()),
        ListView::Listed(rows) => {
            for r in rows {
                lines.push(format!(
                    "{}（{}・{}）",
                    r.name.as_deref().unwrap_or(NO_NAME),
                    r.kind.label(),
                    r.state.label()
                ));
            }
        }
    }
    lines.push(RECENT_HEADING.to_string());
    if state.recent.is_empty() {
        lines.push(RECENT_EMPTY.to_string());
    }
    for (seq, kind) in &state.recent {
        lines.push(format!("#{seq} {}", kind.label()));
    }
    lines.join("\n")
}

/// 失敗の案内（`TextRole::Error`）。停止・通知だけ取れないは失敗ではないので出さない。
pub fn error_text(state: &RemovableStorageShowState) -> String {
    match (state.phase, state.outcome) {
        (Phase::Idle, Some(Outcome::StartFailed)) => START_FAILED_TEXT.to_string(),
        (Phase::Idle, Some(Outcome::RuntimeFailed)) => RUNTIME_FAILED_TEXT.to_string(),
        _ => String::new(),
    }
}

/// 画面を組み立てる。操作を前、一覧を後に置く。全体が非対応のときは開始ボタンを出さず、
/// 一覧が非対応のときは再取得ボタンを出さない。
pub fn current_screen(state: &RemovableStorageShowState) -> Screen<RemovableStorageShowState> {
    let mut screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text);
    if is_unsupported(state) {
        return screen;
    }
    let label = if state.phase == Phase::Idle {
        START_LABEL
    } else {
        STOP_LABEL
    };
    screen = screen.button(TOGGLE_BUTTON_ID, label, toggle);
    if state.list != ListView::Unsupported {
        screen = screen.button(REFRESH_BUTTON_ID, REFRESH_LABEL, refresh);
    }
    screen.dynamic_text(list_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn support_all() -> SupportView {
        SupportView { events: [true; 4] }
    }

    fn receiving() -> RemovableStorageShowState {
        let mut s = RemovableStorageShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Ok(()));
        s
    }

    fn row(name: &str) -> VolumeRow {
        VolumeRow {
            name: Some(name.into()),
            kind: KindView::Removable,
            state: StateView::Mounted,
        }
    }

    #[test]
    fn startup_idle_and_does_not_subscribe() {
        let mut s = RemovableStorageShowState::default();
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(take_start(&s), None);
        assert!(take_init(&s));
        apply_init(&mut s, support_all(), ListView::Listed(vec![]));
        assert!(!take_init(&s));
    }

    #[test]
    fn start_spam_advances_generation_once() {
        let mut s = RemovableStorageShowState::default();
        start(&mut s);
        start(&mut s);
        start(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_start(&s), Some(1));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = RemovableStorageShowState::default();
        start(&mut s);
        finish_start(&mut s, 99, Ok(()));
        assert_eq!(s.phase, Phase::Starting);
        finish_start(&mut s, 1, Ok(()));
        apply_snapshot(
            &mut s,
            99,
            vec![(1, EventKind::Mounted)],
            [1, 0, 0, 0],
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
            vec![(1, EventKind::Mounted)],
            [1, 0, 0, 0],
            Some(ListView::ReadFailed),
        );
        assert!(s.recent.is_empty());
        assert_eq!(s.list, ListView::NotRead);
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
    fn start_failure_shows_error() {
        let mut s = RemovableStorageShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Err(StartFailure::Failed));
        assert_eq!(error_text(&s), START_FAILED_TEXT);
    }

    #[test]
    fn events_unavailable_is_not_an_error_and_keeps_list() {
        let mut s = RemovableStorageShowState::default();
        apply_init(
            &mut s,
            SupportView { events: [false; 4] },
            ListView::Listed(vec![row("A")]),
        );
        // 一覧が読めるので非対応にならない。
        assert_eq!(s.outcome, None);
        start(&mut s);
        finish_start(&mut s, 1, Err(StartFailure::Unsupported));
        assert_eq!(s.outcome, Some(Outcome::EventsUnavailable));
        assert_eq!(error_text(&s), "");
        assert!(status_text(&s).contains(EVENTS_UNAVAILABLE_TEXT));
        assert!(list_text(&s).contains("A（取り外し可能・マウント中）"));
        // 再度始められる。
        start(&mut s);
        assert_eq!(s.phase, Phase::Starting);
    }

    #[test]
    fn nothing_supported_hides_buttons() {
        let mut s = RemovableStorageShowState::default();
        apply_init(
            &mut s,
            SupportView { events: [false; 4] },
            ListView::Unsupported,
        );
        assert_eq!(s.outcome, Some(Outcome::Unsupported));
        let screen = current_screen(&s);
        let mut s2 = s.clone();
        assert!(!click_button(&screen, START_LABEL, &mut s2));
        assert!(!click_button(&screen, REFRESH_LABEL, &mut s2));
        assert!(status_text(&s).contains(UNSUPPORTED_TEXT));
        start(&mut s);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn unsupported_kinds_show_not_available() {
        let mut s = RemovableStorageShowState::default();
        apply_init(
            &mut s,
            SupportView {
                events: [true, false, true, false],
            },
            ListView::Listed(vec![]),
        );
        let text = status_text(&s);
        assert!(text.contains(&format!("取り外しの直前: {NOT_AVAILABLE}")));
        assert!(text.contains("マウントされた: 0 回"));
    }

    #[test]
    fn read_failed_is_not_shown_as_empty() {
        let mut s = RemovableStorageShowState::default();
        apply_init(&mut s, support_all(), ListView::ReadFailed);
        let text = list_text(&s);
        assert!(text.contains(LIST_FAILED_TEXT));
        assert!(!text.contains(VOLUMES_EMPTY));
    }

    #[test]
    fn empty_list_and_unnamed_rows_render() {
        let mut s = RemovableStorageShowState::default();
        apply_init(&mut s, support_all(), ListView::Listed(vec![]));
        assert!(list_text(&s).contains(VOLUMES_EMPTY));
        refresh(&mut s);
        let request = s.list_request;
        apply_list(
            &mut s,
            request,
            ListView::Listed(vec![VolumeRow {
                name: None,
                kind: KindView::Unknown,
                state: StateView::Unknown,
            }]),
        );
        assert!(list_text(&s).contains("（名前なし）（不明・不明）"));
    }

    #[test]
    fn refresh_requests_and_stale_list_is_ignored() {
        let mut s = RemovableStorageShowState::default();
        apply_init(&mut s, support_all(), ListView::Listed(vec![]));
        assert_eq!(take_refresh(&s), None);
        refresh(&mut s);
        assert_eq!(take_refresh(&s), Some(1));
        refresh(&mut s);
        assert_eq!(take_refresh(&s), Some(2));
        apply_list(&mut s, 2, ListView::Listed(vec![row("New")]));
        assert_eq!(take_refresh(&s), None);
        // 古い要求 1 の結果は無視する。
        apply_list(&mut s, 1, ListView::Listed(vec![row("Old")]));
        assert!(list_text(&s).contains("New"));
        assert!(!list_text(&s).contains("Old"));
    }

    #[test]
    fn snapshot_renders_recent_counts_and_list() {
        let mut s = receiving();
        apply_init(&mut s, support_all(), ListView::NotRead);
        apply_snapshot(
            &mut s,
            1,
            vec![(2, EventKind::Unmounted), (1, EventKind::Mounted)],
            [1, 0, 1, 0],
            Some(ListView::Listed(vec![row("USB")])),
        );
        let text = status_text(&s);
        assert!(text.contains("マウントされた: 1 回"));
        let list = list_text(&s);
        assert!(list.contains("#2 取り外された"));
        assert!(list.contains("USB（取り外し可能・マウント中）"));
    }

    #[test]
    fn button_toggles_and_refresh_button_works() {
        let mut s = RemovableStorageShowState::default();
        apply_init(&mut s, support_all(), ListView::Listed(vec![]));
        let screen = current_screen(&s);
        assert!(click_button(&screen, START_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Starting);
        let screen = current_screen(&s);
        assert!(click_button(&screen, REFRESH_LABEL, &mut s));
        assert_eq!(take_refresh(&s), Some(1));
        assert!(click_button(&screen, STOP_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Idle);
    }
}
