//! Phase 11（端末機能の拡充ループ、親 #1143）型サンプル「フォルダの変更監視」（Issue #1410）。
//! 「監視を開始」でアプリ専用の作業フォルダ 1 つの直下のファイルを見張り、作成・更新・削除を
//! 新しい順の一覧に出す 1 画面。確認用に「ファイルを作る / 更新する / 削除する」ボタンで作業フォルダを
//! 自分で変えられる。`FANDHE_PROXY_APP=file_watch_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod file_watch_show;` と対）。任意フォルダの監視は #1408。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の `device/file_watch.rs` が
//! [`take_start`] / [`take_op`] で要求を観測し、[`finish_start`] / [`apply_scan`] / [`mark_failed`] /
//! [`finish_op`] で結果を注入する。
//!
//! **起動時には監視しない**（ボタン起点）。全対象 OS（macOS / Windows / iOS / Android / Web）の
//! 状態・文言をここで確定済みで、後続 OS の実装（#1411〜#1414）は本画面を触らない。
//!
//! # 世代
//!
//! 開始のたびに `watch_generation` を進め、停止後・差し替え後に届いた完了通知は世代で捨てる。
//! 操作要求は `op_generation` で同様に照合する。停止は失敗ではない（失敗文言を出さない）。
//!
//! ```text
//! Idle --「監視を開始」--> Starting(gen += 1) --finish_start(Ok)--> Watching
//! Watching --apply_scan(記録を追加)--> Watching
//! Watching/Starting --「監視を止める」--> Idle（記録は残し「停止中」）
//! Watching --mark_failed--> Idle + 「監視が止まりました」
//! ```

use std::collections::VecDeque;

use crate::{Node, Screen, WidgetId};

const TOGGLE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const CREATE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const UPDATE_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
const DELETE_BUTTON_ID: WidgetId = WidgetId::from_raw(4);

/// `VirtualList` の 1 行あたりの固定高さ。
const ROW_EXTENT: f32 = 24.0;

/// 保持する記録の上限。超えた古い記録は捨てて `dropped` に数える。
pub const MAX_RECORDS: usize = 200;

/// 画面タイトル。
pub const TITLE: &str = "フォルダの変更監視";
/// 開始ボタン。
pub const START_LABEL: &str = "監視を開始";
/// 停止ボタン。
pub const STOP_LABEL: &str = "監視を止める";
/// 作成ボタン。
pub const CREATE_LABEL: &str = "ファイルを作る";
/// 更新ボタン。
pub const UPDATE_LABEL: &str = "更新する";
/// 削除ボタン。
pub const DELETE_LABEL: &str = "削除する";
/// 開始中の案内。
pub const STARTING_STATUS: &str = "開始中…";
/// 監視中の案内。
pub const WATCHING_STATUS: &str = "監視中";
/// 利用者が止めた後の案内（失敗ではない）。
pub const STOPPED_STATUS: &str = "停止中";
/// 件数表示の接頭辞（`現在 N 件`）。
pub const COUNT_PREFIX: &str = "現在 ";
/// 件数表示の末尾。
pub const COUNT_SUFFIX: &str = " 件";
/// 走査の上限を超えたときの案内。
pub const TRUNCATED_TEXT: &str = "ほか多数（上限 64 件）";
/// 古い記録を捨てたときの案内。
pub const DROPPED_TEXT: &str = "古い記録は省略";
/// 操作の対象が無いときの案内（失敗ではない）。
pub const NO_TARGET_STATUS: &str = "対象がありません";
/// 実行中に監視が止まったときの失敗文言。
pub const MONITOR_STOPPED_ERROR: &str = "監視が止まりました";
/// 非対応のときの失敗文言。
pub const UNSUPPORTED_STATUS: &str = "この環境ではフォルダの変更を監視できません";
/// 失敗時の接頭辞（`{接頭辞}{固定文言}`）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// フォルダ表示の接頭辞。
pub const FOLDER_PREFIX: &str = "フォルダ: ";
/// Web で表示するフォルダ名（OPFS 内の専用領域でパスを持たない）。
pub const WEB_FOLDER_LABEL: &str = "ブラウザ内の専用領域";
/// 記録の種別ラベル。
pub const CREATED_LABEL: &str = "作成";
/// 記録の種別ラベル。
pub const MODIFIED_LABEL: &str = "更新";
/// 記録の種別ラベル。
pub const DELETED_LABEL: &str = "削除";

/// 監視の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WatchPhase {
    /// 未開始・停止済み・失敗後。
    #[default]
    Idle,
    /// 開始要求が立ち、結果待ち。
    Starting,
    /// 監視中。
    Watching,
}

/// 記録の種別（device の `ChangeKind` を effect が写す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// 作成。
    Created,
    /// 更新。
    Modified,
    /// 削除。
    Deleted,
}

/// 変更記録 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRecord {
    /// 通し番号（新しいほど大きい）。
    pub seq: u64,
    /// 種別。
    pub kind: RecordKind,
    /// 表示用のファイル名。
    pub name: String,
}

/// 確認用の操作の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// サンプルを作る。
    Create,
    /// 最新のサンプルを更新する。
    Update,
    /// 最新のサンプルを削除する。
    Delete,
}

/// effect ドライバへ渡す操作要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpRequest {
    /// [`finish_op`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 種類。
    pub kind: OpKind,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchFailure {
    /// 非対応。
    Unsupported,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 開始成功時の初期情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartInfo {
    /// 表示用のフォルダ名。
    pub folder: String,
    /// 開始時点のファイル数。
    pub count: usize,
    /// 走査の上限で切り捨てたか。
    pub truncated: bool,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileWatchShowState {
    /// 進行状態。
    pub phase: WatchPhase,
    /// 直近に立てた開始要求の世代。
    pub watch_generation: u64,
    /// 未完了の操作要求。
    pub op: Option<OpRequest>,
    /// 直近に立てた操作要求の世代。
    pub op_generation: u64,
    /// 表示用のフォルダ名。
    pub folder: String,
    /// 現在のファイル数。
    pub file_count: usize,
    /// 走査の上限で切り捨てたか。
    pub truncated: bool,
    /// 変更記録（新しい順）。
    pub records: VecDeque<ChangeRecord>,
    /// 次に振る通し番号。
    pub next_seq: u64,
    /// 上限で捨てた記録の数。
    pub dropped: u64,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。
    pub error: String,
}

/// 開始ボタンのハンドラ。`Idle` のときだけ受け付け、前回の記録を消す。
pub fn start(state: &mut FileWatchShowState) {
    if state.phase != WatchPhase::Idle {
        return;
    }
    state.phase = WatchPhase::Starting;
    state.watch_generation = state.watch_generation.wrapping_add(1);
    state.op = None;
    state.records.clear();
    state.dropped = 0;
    state.file_count = 0;
    state.truncated = false;
    state.folder.clear();
    state.error.clear();
    state.status = STARTING_STATUS.to_string();
}

/// 停止ボタンのハンドラ。利用者の停止は正常で、記録は残す。`Idle` では何もしない。
pub fn stop(state: &mut FileWatchShowState) {
    if state.phase == WatchPhase::Idle {
        return;
    }
    state.phase = WatchPhase::Idle;
    state.op = None;
    state.error.clear();
    state.status = STOPPED_STATUS.to_string();
}

/// 開始 / 停止ボタンのハンドラ（ボタンは 1 つで、状態でラベルが変わる）。
pub fn toggle(state: &mut FileWatchShowState) {
    if state.phase == WatchPhase::Idle {
        start(state);
    } else {
        stop(state);
    }
}

fn request_op(state: &mut FileWatchShowState, kind: OpKind) {
    if state.phase != WatchPhase::Watching || state.op.is_some() {
        return;
    }
    state.op_generation = state.op_generation.wrapping_add(1);
    state.op = Some(OpRequest {
        generation: state.op_generation,
        kind,
    });
    state.error.clear();
}

/// 作成ボタンのハンドラ。
pub fn create(state: &mut FileWatchShowState) {
    request_op(state, OpKind::Create);
}

/// 更新ボタンのハンドラ。
pub fn update(state: &mut FileWatchShowState) {
    request_op(state, OpKind::Update);
}

/// 削除ボタンのハンドラ。
pub fn delete(state: &mut FileWatchShowState) {
    request_op(state, OpKind::Delete);
}

/// 未処理の開始要求の世代を観測する（状態は変えない）。
pub fn take_start(state: &FileWatchShowState) -> Option<u64> {
    (state.phase == WatchPhase::Starting).then_some(state.watch_generation)
}

/// 未処理の操作要求を観測する（状態は変えない）。
pub fn take_op(state: &FileWatchShowState) -> Option<OpRequest> {
    state.op
}

/// 開始の結果を反映する。`Starting` かつ世代が一致するときだけ。
pub fn finish_start(
    state: &mut FileWatchShowState,
    generation: u64,
    result: Result<StartInfo, WatchFailure>,
) {
    if state.phase != WatchPhase::Starting || state.watch_generation != generation {
        return;
    }
    match result {
        Ok(info) => {
            state.phase = WatchPhase::Watching;
            state.folder = info.folder;
            state.file_count = info.count;
            state.truncated = info.truncated;
            state.status = WATCHING_STATUS.to_string();
        }
        Err(f) => {
            state.phase = WatchPhase::Idle;
            state.status.clear();
            state.error = failure_text(f);
        }
    }
}

/// 再走査の結果を反映する。`Watching` かつ世代が一致するときだけ。1 件ごとに通し番号を振り、
/// 上限を超えた古い記録は捨てて数える。
pub fn apply_scan(
    state: &mut FileWatchShowState,
    generation: u64,
    changes: Vec<(RecordKind, String)>,
    count: usize,
    truncated: bool,
) {
    if state.phase != WatchPhase::Watching || state.watch_generation != generation {
        return;
    }
    for (kind, name) in changes {
        let seq = state.next_seq;
        state.next_seq += 1;
        state.records.push_front(ChangeRecord { seq, kind, name });
    }
    while state.records.len() > MAX_RECORDS {
        state.records.pop_back();
        state.dropped += 1;
    }
    state.file_count = count;
    state.truncated = truncated;
}

/// OS の監視が止まったことを反映する（失敗）。`Starting` / `Watching` かつ世代が一致するときだけ。
/// 開始完了（`finish_start`）より先に届いた失敗も受け、後続の `finish_start` は `Idle` で捨てる。
pub fn mark_failed(state: &mut FileWatchShowState, generation: u64) {
    if state.phase == WatchPhase::Idle || state.watch_generation != generation {
        return;
    }
    state.phase = WatchPhase::Idle;
    state.op = None;
    state.status.clear();
    state.error = MONITOR_STOPPED_ERROR.to_string();
}

/// 操作の結果を反映する。世代が合わない完了（停止・差し替え後）は捨てる。`Ok(false)` は対象無し。
pub fn finish_op(
    state: &mut FileWatchShowState,
    generation: u64,
    result: Result<bool, WatchFailure>,
) {
    if state.op.map(|o| o.generation) != Some(generation) {
        return;
    }
    state.op = None;
    match result {
        Ok(true) => state.status = WATCHING_STATUS.to_string(),
        Ok(false) => state.status = NO_TARGET_STATUS.to_string(),
        Err(f) => state.error = failure_text(f),
    }
}

fn failure_text(f: WatchFailure) -> String {
    match f {
        WatchFailure::Unsupported => UNSUPPORTED_STATUS.to_string(),
        WatchFailure::Failed(m) => format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言。監視中はフォルダ名・件数・切り捨ての案内を足す。
pub fn status_text(state: &FileWatchShowState) -> String {
    let mut lines = vec![state.status.clone()];
    if state.phase == WatchPhase::Watching {
        if !state.folder.is_empty() {
            lines.push(format!("{FOLDER_PREFIX}{}", state.folder));
        }
        lines.push(format!("{COUNT_PREFIX}{}{COUNT_SUFFIX}", state.file_count));
        if state.truncated {
            lines.push(TRUNCATED_TEXT.to_string());
        }
    }
    if state.dropped > 0 {
        lines.push(DROPPED_TEXT.to_string());
    }
    lines.retain(|l| !l.is_empty());
    lines.join("\n")
}

/// 失敗時の案内文言（`TextRole::Error`）。
pub fn error_text(state: &FileWatchShowState) -> String {
    state.error.clone()
}

/// `VirtualList` の総行数。
pub fn record_count(state: &FileWatchShowState) -> usize {
    state.records.len()
}

/// 種別の表示ラベル。
pub fn kind_label(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Created => CREATED_LABEL,
        RecordKind::Modified => MODIFIED_LABEL,
        RecordKind::Deleted => DELETED_LABEL,
    }
}

/// `VirtualList` の 1 行（`#seq 種別 名前`）。範囲外は panic せず空テキスト。
pub fn record_row(state: &FileWatchShowState, index: usize) -> Node<FileWatchShowState> {
    Node::Text(
        state
            .records
            .get(index)
            .map(|r| format!("#{} {} {}", r.seq, kind_label(r.kind), r.name))
            .unwrap_or_default(),
    )
}

/// 画面を組み立てる。ボタンは常に同じ並びで出す（出し分けると `VirtualList` のノードパスが変わり、
/// egui が持つスクロール位置が残るため）。無効なときの押下はハンドラが無視する。
pub fn current_screen(state: &FileWatchShowState) -> Screen<FileWatchShowState> {
    let toggle_label = if state.phase == WatchPhase::Idle {
        START_LABEL
    } else {
        STOP_LABEL
    };
    Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .button(TOGGLE_BUTTON_ID, toggle_label, toggle)
        .button(CREATE_BUTTON_ID, CREATE_LABEL, create)
        .button(UPDATE_BUTTON_ID, UPDATE_LABEL, update)
        .button(DELETE_BUTTON_ID, DELETE_LABEL, delete)
        .virtual_list(record_count, record_row, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn info() -> StartInfo {
        StartInfo {
            folder: "f".into(),
            count: 2,
            truncated: false,
        }
    }

    fn watching() -> FileWatchShowState {
        let mut s = FileWatchShowState::default();
        start(&mut s);
        finish_start(&mut s, 1, Ok(info()));
        s
    }

    #[test]
    fn startup_does_not_watch() {
        let s = FileWatchShowState::default();
        assert_eq!(take_start(&s), None);
        assert_eq!(s.phase, WatchPhase::Idle);
    }

    #[test]
    fn start_stop_restart_clears_records() {
        let mut s = watching();
        assert_eq!(s.phase, WatchPhase::Watching);
        apply_scan(&mut s, 1, vec![(RecordKind::Created, "a".into())], 3, false);
        stop(&mut s);
        assert_eq!(s.phase, WatchPhase::Idle);
        assert_eq!(s.records.len(), 1);
        assert_eq!(status_text(&s), format!("{STOPPED_STATUS}"));
        assert_eq!(s.error, "");
        start(&mut s);
        assert_eq!(take_start(&s), Some(2));
        assert!(s.records.is_empty());
    }

    #[test]
    fn idle_ops_are_ignored() {
        let mut s = FileWatchShowState::default();
        create(&mut s);
        assert_eq!(take_op(&s), None);
        let mut s = watching();
        create(&mut s);
        let first = take_op(&s);
        update(&mut s);
        assert_eq!(take_op(&s), first, "処理中は二重に積まない");
    }

    #[test]
    fn stale_generations_are_dropped() {
        let mut s = FileWatchShowState::default();
        start(&mut s);
        finish_start(&mut s, 99, Ok(info()));
        assert_eq!(s.phase, WatchPhase::Starting);
        finish_start(&mut s, 1, Ok(info()));
        apply_scan(&mut s, 99, vec![(RecordKind::Created, "a".into())], 9, true);
        assert!(s.records.is_empty() && s.file_count == 2 && !s.truncated);
        mark_failed(&mut s, 99);
        assert_eq!(s.phase, WatchPhase::Watching);
        create(&mut s);
        finish_op(&mut s, 99, Err(WatchFailure::Failed("x".into())));
        assert!(take_op(&s).is_some() && s.error.is_empty());
        stop(&mut s);
        assert_eq!(take_op(&s), None);
        finish_op(&mut s, 1, Ok(false));
        assert_eq!(status_text(&s), STOPPED_STATUS);
        // 停止後に届いた古い完了・走査・失敗は反映しない。
        apply_scan(&mut s, 1, vec![(RecordKind::Created, "a".into())], 9, true);
        mark_failed(&mut s, 1);
        assert!(s.records.is_empty() && s.error.is_empty());
    }

    #[test]
    fn newest_first_with_seq_and_cap() {
        let mut s = watching();
        apply_scan(
            &mut s,
            1,
            (0..MAX_RECORDS + 3)
                .map(|i| (RecordKind::Modified, format!("n{i}")))
                .collect(),
            1,
            false,
        );
        assert_eq!(s.records.len(), MAX_RECORDS);
        assert_eq!(s.dropped, 3);
        assert_eq!(s.records[0].seq, (MAX_RECORDS + 2) as u64);
        assert!(status_text(&s).contains(DROPPED_TEXT));
        assert!(matches!(record_row(&s, 0), Node::Text(t) if t.starts_with("#202 更新 n202")));
        assert!(matches!(record_row(&s, 9999), Node::Text(t) if t.is_empty()));
    }

    #[test]
    fn failure_before_finish_start_is_kept() {
        let mut s = FileWatchShowState::default();
        start(&mut s);
        let g = s.watch_generation;
        mark_failed(&mut s, g);
        finish_start(
            &mut s,
            g,
            Ok(StartInfo {
                folder: "f".into(),
                count: 0,
                truncated: false,
            }),
        );
        assert_eq!(s.phase, WatchPhase::Idle);
        assert_eq!(s.error, MONITOR_STOPPED_ERROR);
    }

    #[test]
    fn mark_failed_is_failure_but_stop_is_not() {
        let mut s = watching();
        mark_failed(&mut s, 1);
        assert_eq!(s.phase, WatchPhase::Idle);
        assert_eq!(s.error, MONITOR_STOPPED_ERROR);
        let mut s = watching();
        stop(&mut s);
        assert!(s.error.is_empty());
    }

    #[test]
    fn failure_texts_are_distinct_and_never_watching() {
        let mut texts = Vec::new();
        for f in [WatchFailure::Unsupported, WatchFailure::Failed("x".into())] {
            let mut s = FileWatchShowState::default();
            start(&mut s);
            finish_start(&mut s, 1, Err(f));
            assert_eq!(s.phase, WatchPhase::Idle);
            assert_eq!(status_text(&s), "");
            texts.push(s.error);
        }
        assert_eq!(texts[0], UNSUPPORTED_STATUS);
        assert!(texts[1].starts_with(FAILED_PREFIX));
        // 0 件の成功は件数で示し、失敗の文言とは別。
        let mut s = FileWatchShowState::default();
        start(&mut s);
        finish_start(
            &mut s,
            1,
            Ok(StartInfo {
                folder: String::new(),
                count: 0,
                truncated: true,
            }),
        );
        let t = status_text(&s);
        assert!(t.contains("現在 0 件") && t.contains(TRUNCATED_TEXT));
        assert!(s.error.is_empty());
    }

    #[test]
    fn op_results_map_to_texts() {
        let mut s = watching();
        delete(&mut s);
        finish_op(&mut s, 1, Ok(false));
        assert_eq!(s.status, NO_TARGET_STATUS);
        assert!(s.error.is_empty());
        create(&mut s);
        finish_op(&mut s, 2, Err(WatchFailure::Unsupported));
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        assert_eq!(s.phase, WatchPhase::Watching);
    }

    #[test]
    fn buttons_keep_position_and_label_switches() {
        let mut s = FileWatchShowState::default();
        let screen = current_screen(&s);
        assert!(!click_button(&screen, STOP_LABEL, &mut s));
        assert!(click_button(&screen, START_LABEL, &mut s));
        assert_eq!(s.phase, WatchPhase::Starting);
        finish_start(&mut s, 1, Ok(info()));
        let screen = current_screen(&s);
        assert!(click_button(&screen, CREATE_LABEL, &mut s));
        assert!(take_op(&s).is_some());
        assert!(click_button(&screen, STOP_LABEL, &mut s));
        assert_eq!(s.phase, WatchPhase::Idle);
        // 停止中でもボタンは残り、押下はハンドラが無視する。
        let screen = current_screen(&s);
        assert!(click_button(&screen, DELETE_LABEL, &mut s));
        assert_eq!(take_op(&s), None);
    }
}
