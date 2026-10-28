//! Phase 11 の拡充ループ型サンプル「クラウド保存先として OS のファイル画面に出る」
//! （親 #194、Issue #1019）。アプリ専用フォルダのファイルを一覧し、固定の見本を置き、
//! 行ごとに消す。`FANDHE_PROXY_APP=file_provider_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod file_provider_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/file_provider.rs` が [`take_request`] で要求を観測し、
//! [`mark_in_flight`] / [`finish`] / [`apply_unsupported`] で結果を注入する。状態・定数・文言は
//! 全対象 OS 分（Android の操作、iOS・macOS・Windows の「保留中」、その他の「非対応」）をここで
//! 確定し、後続の OS 実装（#1020）は画面に触らない。
//!
//! # 0 件・失敗・非対応を混同しない理由
//!
//! 0 件は正常な結果で、失敗・非対応を「0 件」と見せると利用者は原因を取り違える
//! （`device-bridge-contracts.md` 6）。ファイル名・中身・パスは画面のエラー文に出さない。
//!
//! # 状態遷移
//!
//! ```text
//! Requested(action) --mark_in_flight--> InFlight --finish（世代一致のみ）--> Idle
//! Idle --操作（generation += 1）--> Requested
//! ```
//!
//! 開いた直後は `Requested(Reload)` で始まり、Activity 再生成で状態が作り直されたときも読み直す。

use crate::screens::background_task::format_utc;
use crate::screens::screen_capture::format_size;
use crate::{Node, Screen, WidgetId};

const PUT_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const RELOAD_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 行ボタンの識別子の起点。ボタンの識別子と重ならないようにする。
const ROW_ID_BASE: u64 = 1000;
const ROW_EXTENT: f32 = 24.0;

/// 画面タイトル。
pub const TITLE: &str = "ファイル画面に出す保存先";
/// ファイル画面に出る表示名。Android の `strings.xml`（#1020）も同じ文字列を使う。
pub const ROOT_TITLE: &str = "Fandhe（この端末内のみ）";
/// 範囲の注記。
pub const NOTE: &str = "この端末内のみ。ネットワーク同期はしない";
/// 見本のファイル名（重複時は OS 側で「名前 (n).拡張子」へずらす）。
pub const SAMPLE_NAME: &str = "fandhe-sample.txt";
/// 見本の中身（ASCII 固定）。
pub const SAMPLE_CONTENT: &str = "fandhe file provider sample\n";
/// 見本を置くボタン。
pub const PUT_BUTTON_LABEL: &str = "見本を置く";
/// 読み直すボタン。
pub const RELOAD_BUTTON_LABEL: &str = "読み直す";
/// 処理中のボタン（押しても no-op の無効表示）。
pub const BUSY_BUTTON_LABEL: &str = "処理中…";
/// 行ボタンの接頭辞。
pub const DELETE_PREFIX: &str = "消す: ";
/// 更新時刻が無いときの表記。
pub const NO_TIME_TEXT: &str = "更新時刻なし";
/// 処理中の案内。
pub const BUSY_STATUS: &str = "処理中…";
/// 0 件の案内（成功）。
pub const EMPTY_STATUS: &str = "まだ何もありません";
/// 件数の接尾辞（`{件数}{接尾辞}`）。
pub const COUNT_SUFFIX: &str = " 件のファイルがあります";
/// 置けた。
pub const PLACED_STATUS: &str = "置きました";
/// 置けたが通知に失敗。
pub const PLACED_NOT_NOTIFIED_STATUS: &str =
    "置きましたが、ファイル画面への通知に失敗しました（開き直すと出ます）";
/// 消せた。
pub const DELETED_STATUS: &str = "消しました";
/// 消せたが通知に失敗。
pub const DELETED_NOT_NOTIFIED_STATUS: &str =
    "消しましたが、ファイル画面への通知に失敗しました（開き直すと出ます）";
/// 既に無かった。
pub const ALREADY_GONE_ERROR: &str = "既にファイルがありません";
/// 一覧の失敗。
pub const LIST_FAILED_ERROR: &str = "失敗しました: 一覧の読み取り";
/// 置けなかった。
pub const PUT_FAILED_ERROR: &str = "失敗しました: 置けませんでした（同名が多すぎる場合を含む）";
/// 名前の規則違反。
pub const INVALID_NAME_ERROR: &str = "失敗しました: ファイル名が使えません（255 バイトまで）";
/// 消せなかった。
pub const DELETE_FAILED_ERROR: &str = "失敗しました: 消せませんでした";
/// iOS・macOS・Windows の保留中。
pub const DEFERRED_ERROR: &str = "この OS では保留中です";
/// その他の非対応。
pub const UNSUPPORTED_ERROR: &str = "この端末・ブラウザでは使えません";

/// 画面から出す操作。消す対象は押した時点の名前で持つ（一覧が変わって別のファイルを消さない）。
#[derive(Clone, PartialEq, Eq)]
pub enum FileAction {
    /// 読み直し。
    Reload,
    /// 見本を置く。
    PutSample,
    /// 名前のファイルを消す。
    Delete(String),
}

impl std::fmt::Debug for FileAction {
    /// ファイル名を出さない。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reload => f.write_str("Reload"),
            Self::PutSample => f.write_str("PutSample"),
            Self::Delete(_) => f.write_str("Delete"),
        }
    }
}

/// 進行状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// 未操作・完了済み。
    Idle,
    /// 要求が立ち、effect ドライバへまだ渡していない。
    Requested(FileAction),
    /// effect ドライバが処理中。
    InFlight,
}

/// 一覧の 1 行（表示用）。
#[derive(Clone, PartialEq, Eq)]
pub struct EntryView {
    /// ファイル名。
    pub name: String,
    /// バイト数。
    pub size: u64,
    /// 更新時刻（UNIX ミリ秒、表示専用）。
    pub modified_unix_ms: Option<u64>,
}

/// 一覧の見え方。
#[derive(Clone, PartialEq, Eq)]
pub enum View {
    /// まだ読んでいない。
    Unread,
    /// 読めた（0 件を含む）。
    Listed(Vec<EntryView>),
    /// 読めなかった。
    ListFailed,
    /// 非対応。`deferred` は iOS・macOS・Windows の「保留中」。
    Unsupported {
        /// 保留中か（`false` は非対応）。
        deferred: bool,
    },
}

/// 直近の操作の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpResult {
    /// 置けた。
    Placed {
        /// 通知に成功したか。
        notified: bool,
    },
    /// 消せた。
    Deleted {
        /// 通知に成功したか。
        notified: bool,
    },
    /// 既に無かった。
    AlreadyGone,
    /// 置けなかった。
    PutFailed,
    /// 名前の規則違反。
    InvalidName,
    /// 消せなかった。
    DeleteFailed,
}

impl std::fmt::Debug for View {
    /// ファイル名を出さない。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unread => f.write_str("Unread"),
            Self::Listed(v) => write!(f, "Listed({})", v.len()),
            Self::ListFailed => f.write_str("ListFailed"),
            Self::Unsupported { deferred } => write!(f, "Unsupported({deferred})"),
        }
    }
}

/// 画面の状態。`Debug` はファイル名を出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct FileProviderShowState {
    /// 進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代。
    pub generation: u64,
    /// 一覧。
    pub view: View,
    /// 直近の操作の結果（読み直しでは消える）。
    pub last_op: Option<OpResult>,
}

impl std::fmt::Debug for FileProviderShowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileProviderShowState")
            .field("phase", &self.phase)
            .field("generation", &self.generation)
            .field(
                "entries",
                &match &self.view {
                    View::Listed(v) => Some(v.len()),
                    _ => None,
                },
            )
            .field("last_op", &self.last_op)
            .finish()
    }
}

impl Default for FileProviderShowState {
    /// 開いた直後に一覧を 1 回読む。
    fn default() -> Self {
        Self {
            phase: Phase::Requested(FileAction::Reload),
            generation: 1,
            view: View::Unread,
            last_op: None,
        }
    }
}

/// effect ドライバへ渡す要求（世代付き）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 完了関数へそのまま渡す世代番号。
    pub generation: u64,
    /// 操作。
    pub action: FileAction,
}

fn request(state: &mut FileProviderShowState, action: FileAction) {
    if state.phase != Phase::Idle || matches!(state.view, View::Unsupported { .. }) {
        return;
    }
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
}

/// 「見本を置く」のハンドラ。処理中・非対応では no-op。
pub fn put_sample(state: &mut FileProviderShowState) {
    request(state, FileAction::PutSample);
}

/// 「読み直す」のハンドラ。処理中・非対応では no-op。
pub fn reload(state: &mut FileProviderShowState) {
    request(state, FileAction::Reload);
}

/// 行ボタンのハンドラ。押した時点の行の名前を操作に持たせる。範囲外・処理中では no-op。
pub fn delete_row(state: &mut FileProviderShowState, index: usize) {
    let name = match &state.view {
        View::Listed(v) => v.get(index).map(|e| e.name.clone()),
        _ => None,
    };
    if let Some(name) = name {
        request(state, FileAction::Delete(name));
    }
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &FileProviderShowState) -> Option<Request> {
    match &state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action: action.clone(),
        }),
        _ => None,
    }
}

/// 要求を処理中へ進める。世代が違う・`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut FileProviderShowState, generation: u64) {
    if state.generation == generation && matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 完了を反映する。`op` は操作の結果（読み直しは `None`）、`list` は操作後に読み直した一覧。
/// 世代が違う・処理中でない完了は捨てる。
pub fn finish(
    state: &mut FileProviderShowState,
    generation: u64,
    op: Option<OpResult>,
    list: Result<Vec<EntryView>, ()>,
) {
    if state.generation != generation || state.phase != Phase::InFlight {
        return;
    }
    state.phase = Phase::Idle;
    state.last_op = op;
    state.view = match list {
        Ok(v) => View::Listed(v),
        Err(()) => View::ListFailed,
    };
}

/// 非対応を確定する（成功にしない）。世代が違う・処理中でないときは捨てる。
pub fn apply_unsupported(state: &mut FileProviderShowState, generation: u64, deferred: bool) {
    if state.generation != generation || state.phase != Phase::InFlight {
        return;
    }
    state.phase = Phase::Idle;
    state.last_op = None;
    state.view = View::Unsupported { deferred };
}

/// 案内（通常色）。
pub fn status_text(state: &FileProviderShowState) -> String {
    if state.phase != Phase::Idle {
        return BUSY_STATUS.to_string();
    }
    match state.last_op {
        Some(OpResult::Placed { notified: true }) => PLACED_STATUS.to_string(),
        Some(OpResult::Placed { notified: false }) => PLACED_NOT_NOTIFIED_STATUS.to_string(),
        Some(OpResult::Deleted { notified: true }) => DELETED_STATUS.to_string(),
        Some(OpResult::Deleted { notified: false }) => DELETED_NOT_NOTIFIED_STATUS.to_string(),
        _ => match &state.view {
            View::Listed(v) if v.is_empty() => EMPTY_STATUS.to_string(),
            View::Listed(v) => format!("{}{COUNT_SUFFIX}", v.len()),
            _ => String::new(),
        },
    }
}

/// 失敗理由（`TextRole::Error`）。操作の失敗と一覧の失敗は両方出す。
pub fn error_text(state: &FileProviderShowState) -> String {
    if state.phase != Phase::Idle {
        return String::new();
    }
    let mut lines: Vec<&str> = Vec::new();
    match state.last_op {
        Some(OpResult::AlreadyGone) => lines.push(ALREADY_GONE_ERROR),
        Some(OpResult::PutFailed) => lines.push(PUT_FAILED_ERROR),
        Some(OpResult::InvalidName) => lines.push(INVALID_NAME_ERROR),
        Some(OpResult::DeleteFailed) => lines.push(DELETE_FAILED_ERROR),
        _ => {}
    }
    match state.view {
        View::ListFailed => lines.push(LIST_FAILED_ERROR),
        View::Unsupported { deferred: true } => lines.push(DEFERRED_ERROR),
        View::Unsupported { deferred: false } => lines.push(UNSUPPORTED_ERROR),
        _ => {}
    }
    lines.join("\n")
}

fn row_count(state: &FileProviderShowState) -> usize {
    match &state.view {
        View::Listed(v) => v.len(),
        _ => 0,
    }
}

fn row_label(e: &EntryView) -> String {
    let time = e
        .modified_unix_ms
        .map_or_else(|| NO_TIME_TEXT.to_string(), format_utc);
    format!(
        "{DELETE_PREFIX}{}（{}・{time}）",
        e.name,
        format_size(e.size)
    )
}

fn row_node(state: &FileProviderShowState, index: usize) -> Node<FileProviderShowState> {
    match &state.view {
        View::Listed(v) if index < v.len() => Node::IndexedButton {
            label: row_label(&v[index]),
            index,
            on_click: delete_row,
            id: WidgetId::from_raw(ROW_ID_BASE + index as u64),
        },
        _ => Node::Text(String::new()),
    }
}

/// 現在の状態から画面を組む。非対応では操作ボタンを出さない。
pub fn current_screen(state: &FileProviderShowState) -> Screen<FileProviderShowState> {
    let screen = Screen::new(TITLE)
        .text(ROOT_TITLE)
        .text(NOTE)
        .dynamic_text(status_text)
        .error_text(error_text);
    if matches!(state.view, View::Unsupported { .. }) {
        return screen;
    }
    let idle = state.phase == Phase::Idle;
    let label = |s: &'static str| if idle { s } else { BUSY_BUTTON_LABEL };
    screen
        .button(PUT_BUTTON_ID, label(PUT_BUTTON_LABEL), put_sample)
        .button(RELOAD_BUTTON_ID, label(RELOAD_BUTTON_LABEL), reload)
        .virtual_list(row_count, row_node, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn ev(name: &str) -> EntryView {
        EntryView {
            name: name.into(),
            size: 10,
            modified_unix_ms: Some(0),
        }
    }

    fn listed(names: &[&str]) -> FileProviderShowState {
        let mut s = FileProviderShowState::default();
        mark_in_flight(&mut s, 1);
        finish(&mut s, 1, None, Ok(names.iter().map(|n| ev(n)).collect()));
        s
    }

    #[test]
    fn requests_reload_at_open() {
        let s = FileProviderShowState::default();
        assert_eq!(
            take_request(&s),
            Some(Request {
                generation: 1,
                action: FileAction::Reload
            })
        );
    }

    #[test]
    fn stale_or_idle_completion_is_dropped() {
        let mut s = FileProviderShowState::default();
        mark_in_flight(&mut s, 1);
        let before = s.clone();
        finish(&mut s, 0, Some(OpResult::PutFailed), Ok(vec![]));
        apply_unsupported(&mut s, 0, false);
        assert_eq!(s, before);
        let mut idle = listed(&["a"]);
        let before = idle.clone();
        finish(&mut idle, 1, None, Err(()));
        apply_unsupported(&mut idle, 1, true);
        assert_eq!(idle, before);
    }

    #[test]
    fn mark_in_flight_ignores_stale_generation() {
        let mut s = FileProviderShowState::default();
        mark_in_flight(&mut s, 9);
        assert!(take_request(&s).is_some());
    }

    #[test]
    fn empty_failed_and_unsupported_texts_differ() {
        let empty = listed(&[]);
        assert_eq!(status_text(&empty), EMPTY_STATUS);
        assert_eq!(error_text(&empty), "");

        let mut failed = FileProviderShowState::default();
        mark_in_flight(&mut failed, 1);
        finish(&mut failed, 1, None, Err(()));
        assert_eq!(error_text(&failed), LIST_FAILED_ERROR);
        assert_ne!(status_text(&failed), EMPTY_STATUS);

        for (deferred, text) in [(true, DEFERRED_ERROR), (false, UNSUPPORTED_ERROR)] {
            let mut u = FileProviderShowState::default();
            mark_in_flight(&mut u, 1);
            apply_unsupported(&mut u, 1, deferred);
            assert_eq!(error_text(&u), text);
            assert_ne!(status_text(&u), EMPTY_STATUS);
            assert_eq!(take_request(&u), None);
        }
    }

    #[test]
    fn placed_without_notify_shows_both_success_and_notify_failure() {
        let mut s = listed(&[]);
        put_sample(&mut s);
        mark_in_flight(&mut s, 2);
        finish(
            &mut s,
            2,
            Some(OpResult::Placed { notified: false }),
            Err(()),
        );
        assert_eq!(status_text(&s), PLACED_NOT_NOTIFIED_STATUS);
        assert_eq!(error_text(&s), LIST_FAILED_ERROR);
    }

    #[test]
    fn busy_and_unsupported_are_noop() {
        let mut s = FileProviderShowState::default();
        let before = s.clone();
        put_sample(&mut s);
        reload(&mut s);
        delete_row(&mut s, 0);
        assert_eq!(s, before);
        assert!(click_button(&current_screen(&s), BUSY_BUTTON_LABEL, &mut s));
        assert_eq!(s, before);

        let mut u = FileProviderShowState::default();
        mark_in_flight(&mut u, 1);
        apply_unsupported(&mut u, 1, false);
        let before = u.clone();
        put_sample(&mut u);
        reload(&mut u);
        assert_eq!(u, before);
    }

    #[test]
    fn delete_row_captures_name_and_ignores_out_of_range() {
        let mut s = listed(&["a", "b"]);
        delete_row(&mut s, 5);
        assert_eq!(s.phase, Phase::Idle);
        delete_row(&mut s, 1);
        assert_eq!(
            take_request(&s).map(|r| r.action),
            Some(FileAction::Delete("b".into()))
        );
    }

    #[test]
    fn debug_hides_names() {
        let s = listed(&["secret-name"]);
        assert!(!format!("{s:?}").contains("secret-name"));
        assert!(!format!("{:?}", FileAction::Delete("secret-name".into())).contains("secret"));
    }
}
