//! Phase 11（端末機能の拡充ループ、親 #1137）型サンプル「MIDI 入力」（Issue #1398）。
//! 「受信開始」を押すと接続中の MIDI 入力から届いたノートオン / オフを新しい順に一覧へ出す 1 画面。
//! `FANDHE_PROXY_APP=midi_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod midi_show;` と対）。範囲はノートオン / オフの
//! 表示のみで、出力・SysEx・CC・デバイス名は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/midi.rs` が [`take_command`] で要求を観測し、[`apply_update`] で
//! 通知を注入する（`audio_devices_show.rs` と同型の状態機械 + 継続配信）。
//!
//! 全対象 OS（macOS / iOS / Android / Windows / Web）の文言・状態をここで確定済みで、後続の OS 実装
//! （#1399〜#1402）は本画面を触らない。0 台は成功表示（[`NO_SOURCES_STATUS`]）で、エラーにしない。
//!
//! ```text
//! Idle/Failed --「受信開始」--> Starting（generation += 1, command = Start）
//!   --Started(n, failed)--> Listening{n, failed} --Notes/Sources--> 反映
//!   --「停止」--> Idle（generation += 1, command = Stop。一覧は残す）
//! Starting --期限切れ expire_start--> Failed(START_TIMEOUT)
//! ```
//!
//! 世代の違う通知と、`Listening` 以外で届いた `Sources` / `Notes` は捨てる（停止後の遅れた通知が
//! 一覧を汚さない）。

use std::collections::VecDeque;

use crate::{Screen, WidgetId};

/// 「受信開始」ボタン。
const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタン。
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「消去」ボタン。
const CLEAR_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 画面に残す直近の件数（`fandhe_device::midi::MAX_SHOWN_EVENTS` と同値。依存方向のため複製）。
pub const MAX_SHOWN_EVENTS: usize = 100;

/// 画面タイトル。
pub const TITLE: &str = "MIDI 入力";
/// 開始ボタンのラベル。
pub const START_LABEL: &str = "受信開始";
/// 停止ボタンのラベル。
pub const STOP_LABEL: &str = "停止";
/// 消去ボタンのラベル。
pub const CLEAR_LABEL: &str = "消去";
/// 開始中の案内。
pub const STARTING_STATUS: &str = "開始しています…";
/// 受信中の案内の接頭辞（`{接頭辞}{N}{接尾辞}`）。
pub const LISTENING_PREFIX: &str = "受信中（接続中の入力 ";
/// 受信中の案内の接尾辞。
pub const LISTENING_SUFFIX: &str = " 件）";
/// 0 台のときの案内（成功表示）。
pub const NO_SOURCES_STATUS: &str = "受信中（接続中の MIDI 入力がありません）";
/// 接続できなかった入力があるときの案内（`{N}{接尾辞}`）。接続 0 台でも「入力がありません」とは出さない。
pub const FAILED_SOURCES_SUFFIX: &str = " 台の入力に接続できませんでした";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境では MIDI 入力を使えません";
/// 許可されなかったときの案内（Web）。
pub const PERMISSION_DENIED_STATUS: &str = "MIDI の使用が許可されませんでした";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 期限切れの失敗理由。
pub const START_TIMEOUT: &str = "開始できませんでした（期限切れ）";

/// 画面側の進行状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MidiPhase {
    /// 未開始・停止済み。
    #[default]
    Idle,
    /// 開始要求中（確定待ち）。
    Starting,
    /// 受信中。
    Listening {
        /// 接続中の入力数。
        sources: usize,
        /// 接続できなかった入力数（受信は接続できた分で続ける）。
        failed: usize,
    },
    /// 失敗。
    Failed(MidiFailure),
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiFailure {
    /// 非対応。
    Unsupported,
    /// 許可されなかった（Web）。
    PermissionDenied,
    /// それ以外（固定文言のみ。OS の生コードを入れない）。
    Failed(String),
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiCommand {
    /// 購読を始める。
    Start,
    /// 購読を止める。
    Stop,
}

/// 一覧の 1 行（`fandhe_device::midi::NoteEvent` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNoteRow {
    /// ノートオンか。
    pub on: bool,
    /// チャンネル（1..=16）。
    pub channel: u8,
    /// ノート番号。
    pub note: u8,
    /// ベロシティ。
    pub velocity: u8,
}

/// effect ドライバから注入する通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiUpdate {
    /// 開始が確定した（接続数, 接続失敗数）。
    Started(usize, usize),
    /// 入力数が変わった（接続数, 接続失敗数）。
    Sources(usize, usize),
    /// ノート（古い順）と、これまでに取りこぼした累計件数。
    Notes(Vec<MidiNoteRow>, u64),
    /// 「消去」の時点でドライバが数えていた取りこぼし累計。ドライバ側の集計と順序付けた基準で、
    /// 消去後に画面へ届く通知が消去前の超過分を再表示しないようにする。
    DroppedBase(u64),
    /// 開始の失敗。
    Failed(MidiFailure),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MidiShowState {
    /// 進行状態。
    pub phase: MidiPhase,
    /// 直近に立てた要求の世代。通知が「今の要求」かの照合に使う。
    pub generation: u64,
    /// 未発送の要求。
    pub command: Option<MidiCommand>,
    /// 新しい順の一覧（最大 [`MAX_SHOWN_EVENTS`] 件）。
    pub events: VecDeque<MidiNoteRow>,
    /// 受信した累計件数。
    pub total: u64,
    /// 取りこぼした累計件数（ドライバの保留上限超過）。`total` と同じく「消去」までの累計で、
    /// 停止→再開では保持する。
    pub dropped: u64,
    /// 前の接続までの取りこぼし累計（ドライバの数え直しをまたいで足す）。
    dropped_carry: u64,
    /// ドライバが通知した取りこぼし累計の最新値（接続ごとに 0 から数え直される）。
    dropped_raw: u64,
    /// 「消去」時点の `dropped_raw`。表示は差分にして集計期間を `total` と揃える。
    dropped_base: u64,
    /// 「消去」を受け付けた回数。ドライバが変化を見て `MidiUpdate::DroppedBase` を返す。
    pub clear_epoch: u64,
}

/// 「受信開始」。`Idle` / `Failed` のときだけ受け付ける。
pub fn start(state: &mut MidiShowState) {
    if !matches!(state.phase, MidiPhase::Idle | MidiPhase::Failed(_)) {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.phase = MidiPhase::Starting;
    state.command = Some(MidiCommand::Start);
    // 接続ごとに数え直されるドライバの値は、ここまでの分を繰り越して表示を保つ。
    state.dropped_carry = state.dropped;
    state.dropped_raw = 0;
    state.dropped_base = 0;
}

/// 「停止」。`Starting` / `Listening` のときだけ受け付ける。利用者の停止は失敗ではなく `Idle`。
pub fn stop(state: &mut MidiShowState) {
    if !matches!(
        state.phase,
        MidiPhase::Starting | MidiPhase::Listening { .. }
    ) {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.phase = MidiPhase::Idle;
    state.command = Some(MidiCommand::Stop);
}

/// 「消去」。購読は止めない。
pub fn clear(state: &mut MidiShowState) {
    state.events.clear();
    state.total = 0;
    state.dropped = 0;
    state.dropped_carry = 0;
    // 基準は暫定で最後に適用した値。ドライバが `clear_epoch` を見て、その時点の最新カウンタで
    // `DroppedBase` を返し確定させる（受信スレッドで先に増えた分を消去後に出さない）。
    state.dropped_base = state.dropped_raw;
    state.clear_epoch = state.clear_epoch.wrapping_add(1);
}

/// 未発送の要求と世代を観測する getter（状態は変えない）。
pub fn take_command(state: &MidiShowState) -> Option<(MidiCommand, u64)> {
    state.command.map(|c| (c, state.generation))
}

/// 要求を発送済みにする（次フレームで二重に取らない）。世代が違えば何もしない。
pub fn mark_command_taken(state: &mut MidiShowState, generation: u64) {
    if state.generation == generation {
        state.command = None;
    }
}

/// 通知を反映する。世代違いは捨てる。
pub fn apply_update(state: &mut MidiShowState, generation: u64, update: MidiUpdate) {
    if state.generation != generation {
        return;
    }
    match update {
        MidiUpdate::Started(n, bad) => {
            if state.phase == MidiPhase::Starting {
                state.phase = MidiPhase::Listening {
                    sources: n,
                    failed: bad,
                };
            }
        }
        MidiUpdate::Sources(n, bad) => {
            if let MidiPhase::Listening { sources, failed } = &mut state.phase {
                *sources = n;
                *failed = bad;
            }
        }
        MidiUpdate::Notes(rows, dropped) => {
            if matches!(state.phase, MidiPhase::Listening { .. }) {
                state.total += rows.len() as u64;
                state.dropped_raw = dropped;
                state.dropped = state.dropped_carry + dropped.saturating_sub(state.dropped_base);
                for r in rows {
                    state.events.push_front(r);
                }
                state.events.truncate(MAX_SHOWN_EVENTS);
            }
        }
        MidiUpdate::DroppedBase(raw) => {
            // カウンタは単調増加。順序が前後しても基準は戻さない。
            state.dropped_raw = state.dropped_raw.max(raw);
            state.dropped_base = state.dropped_base.max(raw);
            state.dropped = state.dropped_carry + state.dropped_raw - state.dropped_base;
        }
        MidiUpdate::Failed(f) => {
            if matches!(
                state.phase,
                MidiPhase::Starting | MidiPhase::Listening { .. }
            ) {
                state.phase = MidiPhase::Failed(f);
            }
        }
    }
}

/// 開始の期限切れ。`Starting` かつ世代一致のときだけ失敗にし、世代を進める（遅れた通知を捨てる）。
pub fn expire_start(state: &mut MidiShowState, generation: u64) {
    if state.generation == generation && state.phase == MidiPhase::Starting {
        state.generation = state.generation.wrapping_add(1);
        state.phase = MidiPhase::Failed(MidiFailure::Failed(START_TIMEOUT.to_string()));
    }
}

/// ノート番号の音名（C4 = 60）。
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i32::from(note / 12) - 1;
    format!("{}{octave}", NAMES[usize::from(note % 12)])
}

/// 1 行の表示。オフではベロシティを出さない。
pub fn row_text(r: &MidiNoteRow) -> String {
    let name = note_name(r.note);
    if r.on {
        format!(
            "ノートオン ch{} {} ({name}) vel {}",
            r.channel, r.note, r.velocity
        )
    } else {
        format!("ノートオフ ch{} {} ({name})", r.channel, r.note)
    }
}

/// 案内文言（通常色）。失敗は [`error_text`] 側。
pub fn status_text(state: &MidiShowState) -> String {
    match &state.phase {
        MidiPhase::Idle | MidiPhase::Failed(_) => String::new(),
        MidiPhase::Starting => STARTING_STATUS.to_string(),
        MidiPhase::Listening {
            sources: 0,
            failed: 0,
        } => NO_SOURCES_STATUS.to_string(),
        MidiPhase::Listening { sources, failed } => {
            let mut s = if *sources == 0 {
                "受信中".to_string()
            } else {
                format!("{LISTENING_PREFIX}{sources}{LISTENING_SUFFIX}")
            };
            if *failed > 0 {
                s.push_str(&format!(" / {failed}{FAILED_SOURCES_SUFFIX}"));
            }
            s
        }
    }
}

/// 失敗の案内（`TextRole::Error`）。失敗以外は空。
pub fn error_text(state: &MidiShowState) -> String {
    match &state.phase {
        MidiPhase::Failed(MidiFailure::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        MidiPhase::Failed(MidiFailure::PermissionDenied) => PERMISSION_DENIED_STATUS.to_string(),
        MidiPhase::Failed(MidiFailure::Failed(m)) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 件数の要約。取りこぼしがあれば隠さず足す。
pub fn summary_text(state: &MidiShowState) -> String {
    if state.total == 0 && state.events.is_empty() {
        return String::new();
    }
    let mut s = format!("直近 {} 件 / 累計 {} 件", state.events.len(), state.total);
    if state.dropped > 0 {
        s.push_str(&format!(" / 取りこぼし {} 件", state.dropped));
    }
    s
}

/// 一覧（新しい順、1 行 1 件）。100 件なので `virtual_list` は使わない。
pub fn events_text(state: &MidiShowState) -> String {
    state
        .events
        .iter()
        .map(row_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 画面を組み立てる。`Starting` 中は開始・停止のどちらも出さない。
pub fn current_screen(state: &MidiShowState) -> Screen<MidiShowState> {
    let mut screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .dynamic_text(summary_text);
    match state.phase {
        MidiPhase::Idle | MidiPhase::Failed(_) => {
            screen = screen.button(START_BUTTON_ID, START_LABEL, start);
        }
        MidiPhase::Listening { .. } => {
            screen = screen.button(STOP_BUTTON_ID, STOP_LABEL, stop);
        }
        MidiPhase::Starting => {}
    }
    if state.total > 0 || !state.events.is_empty() {
        screen = screen.button(CLEAR_BUTTON_ID, CLEAR_LABEL, clear);
    }
    // 操作を前、長い一覧を最後に置く（窓外へ出てもボタンへ届く。Issue #77）。
    screen.dynamic_text(events_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(s: &mut MidiShowState, u: MidiUpdate) {
        let g = s.generation;
        apply_update(s, g, u);
    }

    fn row(on: bool, note: u8) -> MidiNoteRow {
        MidiNoteRow {
            on,
            channel: 1,
            note,
            velocity: 100,
        }
    }

    fn listening() -> MidiShowState {
        let mut s = MidiShowState::default();
        start(&mut s);
        upd(&mut s, MidiUpdate::Started(1, 0));
        s
    }

    #[test]
    fn default_does_not_request_anything() {
        let s = MidiShowState::default();
        assert_eq!(take_command(&s), None);
        assert_eq!(s.phase, MidiPhase::Idle);
    }

    #[test]
    fn start_requests_and_mark_taken_clears() {
        let mut s = MidiShowState::default();
        start(&mut s);
        assert_eq!(take_command(&s), Some((MidiCommand::Start, 1)));
        mark_command_taken(&mut s, 0);
        assert!(take_command(&s).is_some(), "世代違いでは消えない");
        mark_command_taken(&mut s, 1);
        assert_eq!(take_command(&s), None);
        start(&mut s); // Starting 中は二重要求しない
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn stale_generation_and_idle_updates_are_dropped() {
        let mut s = listening();
        let g = s.generation;
        apply_update(&mut s, g - 1, MidiUpdate::Notes(vec![row(true, 60)], 0));
        assert!(s.events.is_empty());
        stop(&mut s);
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 60)], 0));
        apply_update(&mut s, g, MidiUpdate::Notes(vec![row(true, 60)], 0));
        assert!(s.events.is_empty());
        assert_eq!(s.phase, MidiPhase::Idle);
    }

    #[test]
    fn zero_sources_is_success_not_error() {
        let mut s = MidiShowState::default();
        start(&mut s);
        upd(&mut s, MidiUpdate::Started(0, 0));
        assert_eq!(status_text(&s), NO_SOURCES_STATUS);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn partial_connect_failure_is_shown() {
        let mut s = MidiShowState::default();
        start(&mut s);
        upd(&mut s, MidiUpdate::Started(1, 1));
        let t = status_text(&s);
        assert!(t.contains("接続中の入力 1 件") && t.contains("1 台の入力に接続できませんでした"));
        // 抜き差しの経路も同じ扱い
        upd(&mut s, MidiUpdate::Sources(2, 2));
        assert!(status_text(&s).contains("2 台の入力に接続できませんでした"));
        upd(&mut s, MidiUpdate::Sources(2, 0));
        assert!(!status_text(&s).contains("接続できませんでした"));
    }

    #[test]
    fn zero_connected_with_failures_is_not_no_sources() {
        let mut s = MidiShowState::default();
        start(&mut s);
        upd(&mut s, MidiUpdate::Started(0, 0));
        upd(&mut s, MidiUpdate::Sources(0, 1));
        let t = status_text(&s);
        assert_ne!(t, NO_SOURCES_STATUS);
        assert!(!t.contains("入力がありません"));
        assert!(t.contains("1 台の入力に接続できませんでした"));
        assert!(
            matches!(s.phase, MidiPhase::Listening { .. }),
            "受信は止めない"
        );
    }

    #[test]
    fn dropped_survives_restart_and_clear_resets() {
        let mut s = listening();
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 60)], 5));
        stop(&mut s);
        start(&mut s);
        assert_eq!(s.dropped, 5);
        upd(&mut s, MidiUpdate::Started(1, 0));
        // ドライバは接続ごとに 0 から数え直す
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 61)], 2));
        assert_eq!(s.dropped, 7);
        clear(&mut s);
        assert_eq!(s.dropped, 0);
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 62)], 3));
        assert_eq!(s.dropped, 1);
    }

    #[test]
    fn dropped_base_from_driver_hides_pre_clear_overflow() {
        let mut s = listening();
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 60)], 1));
        // 画面が知らない間にドライバで 300 件増え、その後に消去。
        clear(&mut s);
        upd(&mut s, MidiUpdate::DroppedBase(301));
        assert_eq!(s.dropped, 0);
        // 消去後の取りこぼしは数える。
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 61)], 303));
        assert_eq!(s.dropped, 2);
        // 基準は戻らない。
        upd(&mut s, MidiUpdate::DroppedBase(250));
        assert_eq!(s.dropped, 2);
    }

    #[test]
    fn failure_messages_differ() {
        let mut texts = Vec::new();
        for f in [
            MidiFailure::Unsupported,
            MidiFailure::PermissionDenied,
            MidiFailure::Failed("x".into()),
        ] {
            let mut s = MidiShowState::default();
            start(&mut s);
            upd(&mut s, MidiUpdate::Failed(f));
            let t = error_text(&s);
            assert!(!t.is_empty());
            texts.push(t);
        }
        texts.dedup();
        assert_eq!(texts.len(), 3);
    }

    #[test]
    fn keeps_newest_100_and_counts_total() {
        let mut s = listening();
        let rows: Vec<_> = (0..150).map(|i| row(true, (i % 128) as u8)).collect();
        upd(&mut s, MidiUpdate::Notes(rows, 0));
        assert_eq!(s.events.len(), MAX_SHOWN_EVENTS);
        assert_eq!(s.total, 150);
        assert_eq!(s.events.front().unwrap().note, 149 % 128);
    }

    #[test]
    fn clear_keeps_listening() {
        let mut s = listening();
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 60)], 0));
        clear(&mut s);
        assert!(s.events.is_empty() && s.total == 0);
        assert!(matches!(s.phase, MidiPhase::Listening { .. }));
    }

    #[test]
    fn stop_keeps_list_and_is_not_failure() {
        let mut s = listening();
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 60)], 0));
        stop(&mut s);
        assert_eq!(s.phase, MidiPhase::Idle);
        assert_eq!(s.events.len(), 1);
        assert_eq!(take_command(&s).map(|c| c.0), Some(MidiCommand::Stop));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn expire_start_only_when_starting_and_same_generation() {
        let mut s = MidiShowState::default();
        start(&mut s);
        expire_start(&mut s, 99);
        assert_eq!(s.phase, MidiPhase::Starting);
        let g = s.generation;
        expire_start(&mut s, g);
        assert!(error_text(&s).contains(START_TIMEOUT));
        // 遅れて届いた Started は世代違いで捨てられる
        apply_update(&mut s, g, MidiUpdate::Started(1, 0));
        assert!(matches!(s.phase, MidiPhase::Failed(_)));
    }

    #[test]
    fn clear_resets_dropped_baseline() {
        let mut s = listening();
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 60)], 7));
        clear(&mut s);
        assert_eq!(s.dropped, 0);
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 61)], 9));
        assert_eq!(s.dropped, 2);
    }

    #[test]
    fn dropped_is_shown_and_starting_has_no_buttons() {
        let mut s = listening();
        upd(&mut s, MidiUpdate::Notes(vec![row(true, 60)], 7));
        assert!(summary_text(&s).contains("取りこぼし 7 件"));
        let mut s2 = MidiShowState::default();
        start(&mut s2);
        assert_eq!(status_text(&s2), STARTING_STATUS);
    }

    #[test]
    fn note_name_and_row_text() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(127), "G9");
        assert_eq!(row_text(&row(true, 61)), "ノートオン ch1 61 (C#4) vel 100");
        assert_eq!(row_text(&row(false, 60)), "ノートオフ ch1 60 (C4)");
    }
}
