//! Phase 11（端末機能の拡充ループ、親 #172）型サンプル「音声の割り込み・セッション種別」（Issue #840）。
//! セッション種別（他の音を止める / 下げる / 混ぜる）を選んで「開始」を押すと OS に音声セッションを
//! 有効化し、割り込み・フォーカスの出来事を新しい順に一覧へ出す 1 画面。
//! `FANDHE_PROXY_APP=audio_session_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod audio_session_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/audio_session.rs` が [`take_command`] で要求を観測し、
//! [`apply_update`] で通知を注入する（`midi_show.rs` と同型の状態機械 + 継続配信）。
//! 種別・出来事・読み戻しの列挙は `fandhe_device::audio_session` と同順で複製している（依存方向のため）。
//!
//! 全対象 OS（iOS / Android。macOS・Windows・Web は非対応）の状態・文言をここで確定済みで、後続の
//! OS 実装（#841・#842）は本画面を触らない。何を選べて何が読めるかは OS が宣言する [`SupportView`] で
//! 決め、画面は OS 名で分岐しない。今の状態は OS から読み戻した値（[`CurrentView`]）だけを出し、
//! 選んだ値で上書きしない。
//!
//! ```text
//! Idle/Failed --「開始」--> Starting（generation += 1, command = Start(kind)）
//!   --Started--> Active --Event--> 一覧へ追加
//!   --「停止」--> Idle（generation += 1, command = Stop。一覧は残す）
//! Starting/Active --Failed--> Failed（ドライバが次フレームで解放する）
//! ```
//!
//! 世代の違う通知と、`Starting` / `Active` 以外で届いた出来事は捨てる（停止後の遅れた通知が一覧を汚さない）。

use crate::{Screen, WidgetId};

/// 種別ボタン（他の音を止める）。
const KIND_STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 種別ボタン（他の音を下げる）。
const KIND_DUCK_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 種別ボタン（他の音と混ぜる）。
const KIND_MIX_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
/// 「開始」ボタン。
const START_BUTTON_ID: WidgetId = WidgetId::from_raw(4);
/// 「停止」ボタン。
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(5);
/// 「状態を読み直す」ボタン。
const REFRESH_BUTTON_ID: WidgetId = WidgetId::from_raw(6);

/// 画面に残す直近の件数。
pub const MAX_SHOWN_EVENTS: usize = 8;

/// 画面タイトル。
pub const TITLE: &str = "音声の割り込み・セッション種別";
/// 開始ボタンのラベル。
pub const START_LABEL: &str = "開始";
/// 停止ボタンのラベル。
pub const STOP_LABEL: &str = "停止";
/// 読み直しボタンのラベル。
pub const REFRESH_LABEL: &str = "状態を読み直す";
/// 開始中の案内。
pub const STARTING_STATUS: &str = "開始しています…";
/// 有効中の案内の接頭辞（`{接頭辞}{種別}{接尾辞}`）。
pub const ACTIVE_PREFIX: &str = "有効（";
/// 有効中の案内の接尾辞。
pub const ACTIVE_SUFFIX: &str = "）";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では音声セッションを使えません";
/// この OS で選べない種別を開始しようとしたときの案内。
pub const KIND_UNSUPPORTED_STATUS: &str = "この環境ではその種別を選べません";
/// OS が受け付けなかったときの案内（Android の `AUDIOFOCUS_REQUEST_FAILED` 等）。
pub const REJECTED_STATUS: &str = "OS が音声セッションを受け付けませんでした（最前面でない等）";
/// 開始の途中で失敗し、元の設定へ戻せたときの案内。
pub const START_FAILED_STATUS: &str = "開始できませんでした";
/// 開始の途中で失敗し、元の設定へ戻せなかったときの案内。
pub const START_FAILED_NOT_RESTORED_STATUS: &str =
    "開始できませんでした。元の設定へ戻せていない可能性があります";
/// 実行中の失敗（iOS のメディアサービスのリセット等）の案内。
pub const RUNTIME_FAILED_STATUS: &str =
    "音声サービスがリセットされ、セッションを維持できませんでした";
/// 停止したが元の設定へ戻せなかったときの案内。
pub const RESTORE_FAILED_STATUS: &str = "停止しましたが、元の設定へ戻せませんでした";
/// 出来事がまだ無いときの案内。
pub const NO_EVENTS_STATUS: &str = "出来事はまだ届いていません";
/// 読み戻しに失敗したときの案内。
pub const CURRENT_FAILED_STATUS: &str = "今の状態を読めませんでした";

/// 選べるセッション種別（`fandhe_device::audio_session::SessionKind` と同順）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindChoice {
    /// 他の音を止める。
    #[default]
    StopOthers,
    /// 他の音を下げる。
    DuckOthers,
    /// 他の音と混ぜる。
    MixWithOthers,
}

impl KindChoice {
    /// 種別の位置（[`SupportView::kinds`] の添字）。
    pub fn index(self) -> usize {
        self as usize
    }

    /// 種別の表示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::StopOthers => "他の音を止める",
            Self::DuckOthers => "他の音を下げる",
            Self::MixWithOthers => "他の音と混ぜる",
        }
    }
}

/// OS から読み戻した種別（`fandhe_device::audio_session::ReadKind` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKindView {
    /// 他の音を止める。
    StopOthers,
    /// 他の音を下げる。
    DuckOthers,
    /// 他の音と混ぜる。
    MixWithOthers,
    /// 3 種に当たらない OS の値。
    Other,
}

/// Android の音声モード（`fandhe_device::audio_session::AudioMode` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioModeView {
    /// 通常。
    Normal,
    /// 着信音。
    Ringtone,
    /// 通話中。
    InCall,
    /// 通信中（VoIP 等）。
    InCommunication,
    /// 通話スクリーニング。
    CallScreening,
    /// 通話のリダイレクト。
    Redirect,
    /// 不明。
    Unknown,
}

/// 出来事の種類（`fandhe_device::audio_session::EventKind` と同順）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKindView {
    /// iOS: 割り込みの開始。
    InterruptionBegan,
    /// iOS: 割り込みの終了。
    InterruptionEnded,
    /// iOS: 経路の変更。
    RouteChanged,
    /// Android: フォーカスの獲得。
    FocusGained,
    /// Android: フォーカスの喪失。
    FocusLost,
    /// Android: 一時的なフォーカスの喪失。
    FocusLostTransient,
    /// Android: 下げてよい一時的なフォーカスの喪失。
    FocusLostTransientCanDuck,
    /// Android: 出力経路が失われる。
    BecomingNoisy,
}

/// 宣言順の全種類（[`AudioSessionShowState::counts`] の並び）。
const ALL_EVENT_KINDS: [EventKindView; 8] = [
    EventKindView::InterruptionBegan,
    EventKindView::InterruptionEnded,
    EventKindView::RouteChanged,
    EventKindView::FocusGained,
    EventKindView::FocusLost,
    EventKindView::FocusLostTransient,
    EventKindView::FocusLostTransientCanDuck,
    EventKindView::BecomingNoisy,
];

impl EventKindView {
    /// 表示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::InterruptionBegan => "割り込み開始",
            Self::InterruptionEnded => "割り込み終了",
            Self::RouteChanged => "経路の変更",
            Self::FocusGained => "フォーカス獲得",
            Self::FocusLost => "フォーカス喪失",
            Self::FocusLostTransient => "フォーカス一時喪失",
            Self::FocusLostTransientCanDuck => "フォーカス一時喪失（下げてよい）",
            Self::BecomingNoisy => "出力経路の喪失",
        }
    }
}

/// 出来事 1 件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRow {
    /// 種類。
    pub kind: EventKindView,
    /// 再開してよいか。iOS の割り込み終了だけが `Some`。
    pub should_resume: Option<bool>,
}

/// OS が扱える範囲（`fandhe_device::audio_session::Support` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportView {
    /// 選べる種別（[`KindChoice::index`] の順）。
    pub kinds: [bool; 3],
    /// 今の種別を読み戻せるか。
    pub kind_readable: bool,
    /// 他アプリの再生中を読めるか。
    pub other_playing_readable: bool,
    /// 音声モードを読めるか。
    pub audio_mode_readable: bool,
    /// 音楽の再生中を読めるか。
    pub music_active_readable: bool,
}

impl SupportView {
    /// 何か 1 つでも選べる種別があるか。
    pub fn any_kind(&self) -> bool {
        self.kinds.iter().any(|k| *k)
    }

    /// 何か 1 つでも読み戻せる項目があるか。
    pub fn any_readable(&self) -> bool {
        self.kind_readable
            || self.other_playing_readable
            || self.audio_mode_readable
            || self.music_active_readable
    }
}

/// OS から読み戻した今の状態。`None` は「この OS では読めない」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurrentView {
    /// 種別。
    pub kind: Option<ReadKindView>,
    /// 他アプリが再生中か。
    pub other_audio_playing: Option<bool>,
    /// 音声モード。
    pub audio_mode: Option<AudioModeView>,
    /// 音楽が再生中か。
    pub music_active: Option<bool>,
}

/// 失敗の種別（OS の生コード・メッセージは入れない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSessionFailure {
    /// 非対応。
    Unsupported,
    /// この OS では選べない種別。
    KindUnsupported,
    /// OS が受け付けなかった。
    Rejected,
    /// 開始の途中で失敗した。`restored` は元の設定へ戻せたか。
    StartFailed {
        /// 元の設定へ戻せたか。
        restored: bool,
    },
    /// 実行中の失敗。
    Runtime,
}

/// 画面側の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioSessionPhase {
    /// 未開始・停止済み。
    #[default]
    Idle,
    /// 開始要求中（確定待ち）。
    Starting,
    /// 有効。
    Active,
    /// 失敗。
    Failed(AudioSessionFailure),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSessionCommand {
    /// 種別を指定してセッションを開始する。
    Start(KindChoice),
    /// セッションを止める。
    Stop,
}

/// effect ドライバから注入する通知。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSessionUpdate {
    /// OS が扱える範囲（世代に依らず反映する）。
    Support(SupportView),
    /// 開始が確定した。
    Started,
    /// 出来事が届いた。
    Event(EventRow),
    /// 開始または実行中の失敗。
    Failed(AudioSessionFailure),
    /// 停止したが元の設定へ戻せなかった。
    RestoreFailed,
    /// 今の状態を読み戻した結果（`Err` は読めなかった。世代に依らず反映する）。
    Current(Result<CurrentView, ()>),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioSessionShowState {
    /// 進行状態。
    pub phase: AudioSessionPhase,
    /// 直近に立てた要求の世代。通知が「今の要求」かの照合に使う。
    pub generation: u64,
    /// 未発送の要求。
    pub command: Option<AudioSessionCommand>,
    /// 選択中の種別（開始時に要求へ載せる）。
    pub selected: KindChoice,
    /// OS が扱える範囲。ドライバが最初のフレームで注入する。
    pub support: Option<SupportView>,
    /// 最後に読み戻した今の状態。
    pub current: Option<CurrentView>,
    /// 直近の読み戻しが失敗したか。
    pub current_failed: bool,
    /// 「状態を読み直す」を押した回数。ドライバが変化を見て読み戻す。
    pub refresh_epoch: u64,
    /// 新しい順の直近の出来事（番号つき、最大 [`MAX_SHOWN_EVENTS`] 件）。
    pub events: Vec<(u64, EventRow)>,
    /// 受信した出来事の通し番号。ドライバが読み戻しの契機に使う。
    pub event_seq: u64,
    /// 種類ごとの受信回数（`EventKindView` の宣言順）。
    pub counts: [u64; 8],
    /// 直近の停止で元の設定へ戻せなかったか。
    pub restore_failed: bool,
}

fn select(state: &mut AudioSessionShowState, kind: KindChoice) {
    if matches!(
        state.phase,
        AudioSessionPhase::Idle | AudioSessionPhase::Failed(_)
    ) {
        state.selected = kind;
    }
}

fn select_stop(state: &mut AudioSessionShowState) {
    select(state, KindChoice::StopOthers);
}

fn select_duck(state: &mut AudioSessionShowState) {
    select(state, KindChoice::DuckOthers);
}

fn select_mix(state: &mut AudioSessionShowState) {
    select(state, KindChoice::MixWithOthers);
}

/// 「開始」。`Idle` / `Failed` のときだけ受け付ける。
pub fn start(state: &mut AudioSessionShowState) {
    if !matches!(
        state.phase,
        AudioSessionPhase::Idle | AudioSessionPhase::Failed(_)
    ) {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.phase = AudioSessionPhase::Starting;
    state.command = Some(AudioSessionCommand::Start(state.selected));
    state.restore_failed = false;
}

/// 「停止」。`Starting` / `Active` のときだけ受け付ける。利用者の停止は失敗ではなく `Idle`。
pub fn stop(state: &mut AudioSessionShowState) {
    if !matches!(
        state.phase,
        AudioSessionPhase::Starting | AudioSessionPhase::Active
    ) {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.phase = AudioSessionPhase::Idle;
    state.command = Some(AudioSessionCommand::Stop);
}

/// 「状態を読み直す」。
pub fn refresh(state: &mut AudioSessionShowState) {
    state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
}

/// 未発送の要求と世代を観測する getter（状態は変えない）。
pub fn take_command(state: &AudioSessionShowState) -> Option<(AudioSessionCommand, u64)> {
    state.command.map(|c| (c, state.generation))
}

/// 要求を発送済みにする（次フレームで二重に取らない）。世代が違えば何もしない。
pub fn mark_command_taken(state: &mut AudioSessionShowState, generation: u64) {
    if state.generation == generation {
        state.command = None;
    }
}

/// 通知を反映する。`Support` と `Current` は世代に依らず反映し、他は世代違いを捨てる。
pub fn apply_update(
    state: &mut AudioSessionShowState,
    generation: u64,
    update: AudioSessionUpdate,
) {
    match update {
        AudioSessionUpdate::Support(s) => state.support = Some(s),
        AudioSessionUpdate::Current(Ok(c)) => {
            state.current = Some(c);
            state.current_failed = false;
        }
        AudioSessionUpdate::Current(Err(())) => state.current_failed = true,
        _ if state.generation != generation => {}
        AudioSessionUpdate::Started => {
            if state.phase == AudioSessionPhase::Starting {
                state.phase = AudioSessionPhase::Active;
            }
        }
        AudioSessionUpdate::Event(row) => {
            // 登録から `Started` の適用までの間に届いた出来事も落とさない。
            if matches!(
                state.phase,
                AudioSessionPhase::Starting | AudioSessionPhase::Active
            ) {
                state.event_seq += 1;
                state.counts[row.kind as usize] += 1;
                state.events.insert(0, (state.event_seq, row));
                state.events.truncate(MAX_SHOWN_EVENTS);
            }
        }
        AudioSessionUpdate::Failed(f) => {
            if matches!(
                state.phase,
                AudioSessionPhase::Starting | AudioSessionPhase::Active
            ) {
                state.phase = AudioSessionPhase::Failed(f);
            }
        }
        // 停止（`Idle`）の後に確定する失敗なので、フェーズに依らず世代一致で反映する。
        AudioSessionUpdate::RestoreFailed => state.restore_failed = true,
    }
}

fn read_kind_label(k: ReadKindView) -> &'static str {
    match k {
        ReadKindView::StopOthers => "他の音を止める",
        ReadKindView::DuckOthers => "他の音を下げる",
        ReadKindView::MixWithOthers => "他の音と混ぜる",
        ReadKindView::Other => "上の 3 種以外",
    }
}

fn audio_mode_label(m: AudioModeView) -> &'static str {
    match m {
        AudioModeView::Normal => "通常",
        AudioModeView::Ringtone => "着信音",
        AudioModeView::InCall => "通話中",
        AudioModeView::InCommunication => "通信中",
        AudioModeView::CallScreening => "通話スクリーニング",
        AudioModeView::Redirect => "通話のリダイレクト",
        AudioModeView::Unknown => "不明",
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "はい" } else { "いいえ" }
}

/// 案内文言（通常色）。失敗は [`error_text`] 側。
pub fn status_text(state: &AudioSessionShowState) -> String {
    match state.phase {
        AudioSessionPhase::Idle | AudioSessionPhase::Failed(_) => match state.support {
            Some(s) if s.any_kind() => format!("選択中: {}", state.selected.label()),
            _ => String::new(),
        },
        AudioSessionPhase::Starting => STARTING_STATUS.to_string(),
        AudioSessionPhase::Active => {
            format!("{ACTIVE_PREFIX}{}{ACTIVE_SUFFIX}", state.selected.label())
        }
    }
}

/// 失敗の案内（`TextRole::Error`）。失敗がなければ空。
pub fn error_text(state: &AudioSessionShowState) -> String {
    let mut lines = Vec::new();
    match state.phase {
        AudioSessionPhase::Failed(f) => lines.push(match f {
            AudioSessionFailure::Unsupported => UNSUPPORTED_STATUS,
            AudioSessionFailure::KindUnsupported => KIND_UNSUPPORTED_STATUS,
            AudioSessionFailure::Rejected => REJECTED_STATUS,
            AudioSessionFailure::StartFailed { restored: true } => START_FAILED_STATUS,
            AudioSessionFailure::StartFailed { restored: false } => {
                START_FAILED_NOT_RESTORED_STATUS
            }
            AudioSessionFailure::Runtime => RUNTIME_FAILED_STATUS,
        }),
        // 非対応 OS は開始を押す前から分かる（OS が宣言を返す）。
        _ if state.support.is_some_and(|s| !s.any_kind()) => lines.push(UNSUPPORTED_STATUS),
        _ => {}
    }
    if state.restore_failed {
        lines.push(RESTORE_FAILED_STATUS);
    }
    if state.current_failed {
        lines.push(CURRENT_FAILED_STATUS);
    }
    lines.join("\n")
}

/// OS から読み戻した今の状態。読めない項目（`None`）は出さない。
pub fn current_text(state: &AudioSessionShowState) -> String {
    let Some(c) = state.current else {
        return String::new();
    };
    let mut lines = Vec::new();
    if let Some(k) = c.kind {
        lines.push(format!("OS の種別: {}", read_kind_label(k)));
    }
    if let Some(b) = c.other_audio_playing {
        lines.push(format!("他アプリが再生中: {}", yes_no(b)));
    }
    if let Some(m) = c.audio_mode {
        lines.push(format!("音声モード: {}", audio_mode_label(m)));
    }
    if let Some(b) = c.music_active {
        lines.push(format!("音楽が再生中: {}", yes_no(b)));
    }
    lines.join("\n")
}

/// 1 行の表示。`should_resume` は iOS の割り込み終了だけ持つ。
pub fn row_text(seq: u64, row: &EventRow) -> String {
    let mut s = format!("#{seq} {}", row.kind.label());
    match row.should_resume {
        Some(true) => s.push_str("（再開してよい）"),
        Some(false) => s.push_str("（再開は求められていない）"),
        None => {}
    }
    s
}

/// 受信回数の要約（1 回以上届いた種類だけ）。
pub fn counts_text(state: &AudioSessionShowState) -> String {
    ALL_EVENT_KINDS
        .iter()
        .zip(state.counts)
        .filter(|(_, n)| *n > 0)
        .map(|(k, n)| format!("{} {n} 回", k.label()))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 一覧（新しい順、1 行 1 件）。有効で空の間だけ案内を出す。
pub fn events_text(state: &AudioSessionShowState) -> String {
    if state.events.is_empty() {
        return if state.phase == AudioSessionPhase::Active {
            NO_EVENTS_STATUS.to_string()
        } else {
            String::new()
        };
    }
    state
        .events
        .iter()
        .map(|(n, r)| row_text(*n, r))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 種別ボタン 1 つぶん（ボタン id・種別・押下時の処理）。
type KindButton = (WidgetId, KindChoice, fn(&mut AudioSessionShowState));

/// 画面を組み立てる。`Starting` 中は開始・停止のどちらも出さない。選べない種別のボタンは出さない。
pub fn current_screen(state: &AudioSessionShowState) -> Screen<AudioSessionShowState> {
    let mut screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .dynamic_text(current_text);
    match state.phase {
        AudioSessionPhase::Idle | AudioSessionPhase::Failed(_) => {
            let kinds = state.support.map(|s| s.kinds).unwrap_or([false; 3]);
            let buttons: [KindButton; 3] = [
                (KIND_STOP_BUTTON_ID, KindChoice::StopOthers, select_stop),
                (KIND_DUCK_BUTTON_ID, KindChoice::DuckOthers, select_duck),
                (KIND_MIX_BUTTON_ID, KindChoice::MixWithOthers, select_mix),
            ];
            for (id, kind, on_click) in buttons {
                if kinds[kind.index()] {
                    screen = screen.button(id, kind.label(), on_click);
                }
            }
            if kinds.iter().any(|k| *k) {
                screen = screen.button(START_BUTTON_ID, START_LABEL, start);
            }
        }
        AudioSessionPhase::Active => {
            screen = screen.button(STOP_BUTTON_ID, STOP_LABEL, stop);
        }
        AudioSessionPhase::Starting => {}
    }
    if state.support.is_some_and(|s| s.any_readable()) {
        screen = screen.button(REFRESH_BUTTON_ID, REFRESH_LABEL, refresh);
    }
    // 操作を前、増える一覧を最後に置く（窓外へ出てもボタンへ届く。Issue #77）。
    screen.dynamic_text(counts_text).dynamic_text(events_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IOS: SupportView = SupportView {
        kinds: [true, true, true],
        kind_readable: true,
        other_playing_readable: true,
        audio_mode_readable: false,
        music_active_readable: false,
    };

    fn upd(s: &mut AudioSessionShowState, u: AudioSessionUpdate) {
        let g = s.generation;
        apply_update(s, g, u);
    }

    fn ev(kind: EventKindView) -> AudioSessionUpdate {
        AudioSessionUpdate::Event(EventRow {
            kind,
            should_resume: None,
        })
    }

    fn active() -> AudioSessionShowState {
        let mut s = AudioSessionShowState::default();
        upd(&mut s, AudioSessionUpdate::Support(IOS));
        start(&mut s);
        upd(&mut s, AudioSessionUpdate::Started);
        s
    }

    #[test]
    fn default_does_not_request_anything() {
        let s = AudioSessionShowState::default();
        assert_eq!(take_command(&s), None);
        assert_eq!(s.phase, AudioSessionPhase::Idle);
    }

    #[test]
    fn start_carries_selected_kind_and_mark_taken_clears() {
        let mut s = AudioSessionShowState::default();
        select_duck(&mut s);
        start(&mut s);
        assert_eq!(
            take_command(&s),
            Some((AudioSessionCommand::Start(KindChoice::DuckOthers), 1))
        );
        mark_command_taken(&mut s, 0);
        assert!(take_command(&s).is_some(), "世代違いでは消えない");
        mark_command_taken(&mut s, 1);
        assert_eq!(take_command(&s), None);
        start(&mut s); // Starting 中は二重要求しない
        assert_eq!(s.generation, 1);
        select_mix(&mut s); // 開始後は種別を変えない
        assert_eq!(s.selected, KindChoice::DuckOthers);
    }

    #[test]
    fn stale_generation_and_idle_events_are_dropped() {
        let mut s = active();
        let g = s.generation;
        apply_update(&mut s, g - 1, ev(EventKindView::FocusLost));
        assert!(s.events.is_empty());
        stop(&mut s);
        upd(&mut s, ev(EventKindView::FocusLost));
        apply_update(&mut s, g, ev(EventKindView::FocusLost));
        assert!(s.events.is_empty());
        assert_eq!(s.phase, AudioSessionPhase::Idle);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn events_are_newest_first_capped_and_counted() {
        let mut s = active();
        for _ in 0..10 {
            upd(&mut s, ev(EventKindView::InterruptionBegan));
        }
        upd(&mut s, ev(EventKindView::RouteChanged));
        assert_eq!(s.events.len(), MAX_SHOWN_EVENTS);
        assert_eq!(s.events[0].0, 11);
        assert_eq!(s.counts[0], 10);
        assert_eq!(counts_text(&s), "割り込み開始 10 回 / 経路の変更 1 回");
    }

    #[test]
    fn resume_hint_is_shown_only_when_present() {
        let r = EventRow {
            kind: EventKindView::InterruptionEnded,
            should_resume: Some(true),
        };
        assert!(row_text(1, &r).contains("再開してよい"));
        let r = EventRow {
            should_resume: Some(false),
            ..r
        };
        assert!(row_text(1, &r).contains("再開は求められていない"));
        let r = EventRow {
            should_resume: None,
            kind: EventKindView::FocusLost,
        };
        assert!(!row_text(1, &r).contains("再開"));
    }

    #[test]
    fn failures_are_never_success_and_messages_differ() {
        let all = [
            AudioSessionFailure::Unsupported,
            AudioSessionFailure::KindUnsupported,
            AudioSessionFailure::Rejected,
            AudioSessionFailure::StartFailed { restored: true },
            AudioSessionFailure::StartFailed { restored: false },
            AudioSessionFailure::Runtime,
        ];
        let mut texts = Vec::new();
        for f in all {
            let mut s = AudioSessionShowState::default();
            start(&mut s);
            upd(&mut s, AudioSessionUpdate::Failed(f));
            assert_eq!(s.phase, AudioSessionPhase::Failed(f));
            let t = error_text(&s);
            assert!(!t.is_empty());
            texts.push(t);
        }
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), all.len());
    }

    #[test]
    fn failure_during_active_keeps_list_and_allows_restart() {
        let mut s = active();
        upd(&mut s, ev(EventKindView::RouteChanged));
        upd(
            &mut s,
            AudioSessionUpdate::Failed(AudioSessionFailure::Runtime),
        );
        assert_eq!(s.events.len(), 1);
        start(&mut s);
        assert_eq!(s.phase, AudioSessionPhase::Starting);
    }

    #[test]
    fn restore_failure_after_stop_is_shown_and_cleared_on_next_start() {
        let mut s = active();
        stop(&mut s);
        upd(&mut s, AudioSessionUpdate::RestoreFailed);
        assert_eq!(s.phase, AudioSessionPhase::Idle);
        assert_eq!(error_text(&s), RESTORE_FAILED_STATUS);
        // 古い世代の通知は捨てる
        let mut t = AudioSessionShowState::default();
        apply_update(&mut t, 5, AudioSessionUpdate::RestoreFailed);
        assert!(!t.restore_failed);
        start(&mut s);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn unsupported_support_shows_failure_and_no_start_button() {
        let mut s = AudioSessionShowState::default();
        upd(
            &mut s,
            AudioSessionUpdate::Support(SupportView {
                kinds: [false; 3],
                kind_readable: false,
                other_playing_readable: false,
                audio_mode_readable: false,
                music_active_readable: false,
            }),
        );
        assert_eq!(error_text(&s), UNSUPPORTED_STATUS);
        assert_eq!(status_text(&s), "");
    }

    #[test]
    fn current_text_hides_unreadable_items() {
        let mut s = AudioSessionShowState::default();
        upd(
            &mut s,
            AudioSessionUpdate::Current(Ok(CurrentView {
                kind: Some(ReadKindView::Other),
                other_audio_playing: Some(false),
                audio_mode: None,
                music_active: None,
            })),
        );
        let t = current_text(&s);
        assert!(t.contains("上の 3 種以外") && t.contains("他アプリが再生中: いいえ"));
        assert!(!t.contains("音声モード") && !t.contains("音楽"));
        upd(&mut s, AudioSessionUpdate::Current(Err(())));
        assert_eq!(error_text(&s), CURRENT_FAILED_STATUS);
        upd(
            &mut s,
            AudioSessionUpdate::Current(Ok(CurrentView::default())),
        );
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn active_without_events_says_so() {
        let s = active();
        assert_eq!(events_text(&s), NO_EVENTS_STATUS);
        assert!(status_text(&s).contains("他の音を止める"));
    }
}
