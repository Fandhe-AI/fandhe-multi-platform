//! Phase 11（端末機能の拡充ループ、親 #108）型サンプル「音楽ライブラリ」（Issue #482）。
//! 「曲の一覧を読む」ボタン → 端末内の曲のタイトルとアーティストを OS の返す順で先頭 20 件だけ
//! 読んで一覧にする 1 画面。`FANDHE_PROXY_APP=music_library_list` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod music_library_list;` と対）。再生・アートワーク・総件数は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/music_library.rs` が [`take_request`] で要求を観測し [`finish_list`] で結果を注入する
//! （`pedometer_read.rs` と同型の状態機械）。
//!
//! 全対象 OS（macOS / Windows / iOS / Android）の文言・状態をここで確定済みで、後続の OS 実装
//! （#483 / #484 / #485）は本画面を触らない。起動時には読まない（許可ダイアログを開いただけで
//! 出さないため）。
//!
//! # 0 曲と失敗の区別
//!
//! 成功で 0 曲は [`EMPTY_STATUS`] を出し、失敗の文言とは別にする。失敗時は一覧も 0 曲の文言も
//! 残さない（`device-bridge-contracts.md`「6. 嘘の成功」）。
//!
//! ```text
//! Idle --「曲の一覧を読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_loading--> Loading
//!   --finish_list（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const LIST_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// `VirtualList` の 1 行あたりの固定高さ。
const ROW_EXTENT: f32 = 24.0;

/// 画面タイトル。
pub const TITLE: &str = "音楽ライブラリ";
/// 画面の説明。
pub const NOTE: &str =
    "端末内の曲のタイトルとアーティストを先頭 20 件だけ表示します（再生はしません）";
/// ボタンのラベル。
pub const LIST_LABEL: &str = "曲の一覧を読む";
/// 読み込み中の案内。
pub const LOADING_STATUS: &str = "読み込み中…";
/// 0 曲だったときの案内（失敗ではない）。
pub const EMPTY_STATUS: &str = "曲がありません（0 曲）";
/// 切り捨てたときの案内。
pub const TRUNCATED_TEXT: &str = "先頭 20 件だけ表示しています";
/// タイトルが無い曲の代替表示。
pub const NO_TITLE: &str = "（タイトルなし）";
/// アーティストが無い曲の代替表示。
pub const NO_ARTIST: &str = "（アーティスト不明）";
/// 許可されなかったときの案内。
pub const PERMISSION_DENIED_STATUS: &str = "許可されていません";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// 期限切れのときの案内。
pub const TIMEOUT_STATUS: &str = "時間内に読めませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{固定文言}` の形で表示する）。
pub const FAILED_PREFIX: &str = "読めませんでした: ";

/// 一覧の 1 行（タイトル, アーティスト。無い項目は `None`）。
pub type TrackRow = (Option<String>, Option<String>);

/// 読み取れた一覧。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackListing {
    /// 表示する行（OS の返す順の先頭 20 件まで）。
    pub rows: Vec<TrackRow>,
    /// 20 件を超えて切り捨てたか。
    pub truncated: bool,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から読んでいる最中（[`mark_loading`] 済み）。
    Loading,
}

/// 失敗の種別（`fandhe_device::music_library::MusicLibraryFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListFailure {
    /// 非対応。
    Unsupported,
    /// 権限拒否。
    PermissionDenied,
    /// 期限切れ。
    Timeout,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 音楽ライブラリ画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MusicLibraryListState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた一覧。失敗・読み込み中は `None`（0 曲は `Some` の空）。
    pub listing: Option<TrackListing>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空。
    pub error: String,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListRequest {
    /// [`finish_list`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn fetch(state: &mut MusicLibraryListState) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.listing = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &MusicLibraryListState) -> Option<ListRequest> {
    (state.phase == Phase::Requested).then_some(ListRequest {
        generation: state.generation,
    })
}

/// 読み込み中（`Loading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_loading(state: &mut MusicLibraryListState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Loading;
        state.status = LOADING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_list(
    state: &mut MusicLibraryListState,
    generation: u64,
    result: Result<TrackListing, ListFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    state.listing = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(l) => state.listing = Some(l),
        Err(ListFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ListFailure::PermissionDenied) => state.error = PERMISSION_DENIED_STATUS.to_string(),
        Err(ListFailure::Timeout) => state.error = TIMEOUT_STATUS.to_string(),
        Err(ListFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。読み込み中は案内、成功時は 0 曲の案内か切り捨ての案内。
pub fn status_text(state: &MusicLibraryListState) -> String {
    match &state.listing {
        Some(l) if l.rows.is_empty() => EMPTY_STATUS.to_string(),
        Some(l) if l.truncated => TRUNCATED_TEXT.to_string(),
        Some(_) => String::new(),
        None => state.status.clone(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &MusicLibraryListState) -> String {
    state.error.clone()
}

/// `VirtualList` の総行数。
pub fn row_count(state: &MusicLibraryListState) -> usize {
    state.listing.as_ref().map_or(0, |l| l.rows.len())
}

/// 1 行の表示文字列（「タイトル — アーティスト」）。範囲外は panic せず空。
pub fn row_text(state: &MusicLibraryListState, index: usize) -> String {
    state
        .listing
        .as_ref()
        .and_then(|l| l.rows.get(index))
        .map(|(title, artist)| {
            format!(
                "{} — {}",
                title.as_deref().unwrap_or(NO_TITLE),
                artist.as_deref().unwrap_or(NO_ARTIST)
            )
        })
        .unwrap_or_default()
}

/// `VirtualList` の 1 行。
pub fn track_row(state: &MusicLibraryListState, index: usize) -> Node<MusicLibraryListState> {
    Node::Text(row_text(state, index))
}

/// 画面を組み立てる。ボタンは常に出す（隠すと `VirtualList` のノードパスが変わりスクロール位置が
/// 残るため。`local_fonts_show` と同じ）。処理中の押下は [`fetch`] が無視する。
pub fn current_screen(_state: &MusicLibraryListState) -> Screen<MusicLibraryListState> {
    Screen::new(TITLE)
        .text(NOTE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .button(LIST_BUTTON_ID, LIST_LABEL, fetch)
        .virtual_list(row_count, track_row, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn listing(n: usize, truncated: bool) -> TrackListing {
        TrackListing {
            rows: (0..n)
                .map(|i| (Some(format!("T{i}")), Some(format!("A{i}"))))
                .collect(),
            truncated,
        }
    }

    fn loading() -> MusicLibraryListState {
        let mut s = MusicLibraryListState::default();
        fetch(&mut s);
        mark_loading(&mut s);
        s
    }

    #[test]
    fn startup_does_not_request() {
        let s = MusicLibraryListState::default();
        assert_eq!(take_request(&s), None);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn fetch_is_noop_while_in_flight() {
        let mut s = MusicLibraryListState::default();
        fetch(&mut s);
        assert_eq!(take_request(&s), Some(ListRequest { generation: 1 }));
        mark_loading(&mut s);
        assert_eq!(take_request(&s), None);
        fetch(&mut s);
        assert_eq!(s.phase, Phase::Loading);
        assert_eq!(s.generation, 1);
        assert_eq!(s.status, LOADING_STATUS);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = loading();
        finish_list(&mut s, 99, Ok(listing(1, false)));
        assert_eq!(s.phase, Phase::Loading);
        assert_eq!(s.listing, None);
        let mut idle = MusicLibraryListState::default();
        finish_list(&mut idle, 0, Ok(listing(1, false)));
        assert_eq!(idle.listing, None);
    }

    #[test]
    fn zero_tracks_is_success_distinct_from_failure() {
        let mut s = loading();
        finish_list(&mut s, 1, Ok(listing(0, false)));
        assert_eq!(s.listing, Some(TrackListing::default()));
        assert_eq!(status_text(&s), EMPTY_STATUS);
        assert_eq!(s.error, "");
    }

    #[test]
    fn every_failure_is_distinct_and_leaves_no_list_or_empty_text() {
        let cases = [
            (
                ListFailure::PermissionDenied,
                PERMISSION_DENIED_STATUS.to_string(),
            ),
            (ListFailure::Unsupported, UNSUPPORTED_STATUS.to_string()),
            (ListFailure::Timeout, TIMEOUT_STATUS.to_string()),
            (ListFailure::Failed("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (f, expected) in cases {
            let mut s = loading();
            s.listing = Some(listing(3, false));
            finish_list(&mut s, 1, Err(f));
            assert_eq!(s.phase, Phase::Idle);
            assert_eq!(s.listing, None);
            assert_eq!(status_text(&s), "");
            assert_eq!(s.error, expected);
        }
    }

    #[test]
    fn truncated_note_and_placeholders() {
        let mut s = loading();
        finish_list(&mut s, 1, Ok(listing(20, true)));
        assert_eq!(status_text(&s), TRUNCATED_TEXT);
        s.listing = Some(TrackListing {
            rows: vec![(None, None), (Some("t".into()), None)],
            truncated: false,
        });
        assert_eq!(row_text(&s, 0), format!("{NO_TITLE} — {NO_ARTIST}"));
        assert_eq!(row_text(&s, 1), format!("t — {NO_ARTIST}"));
        assert_eq!(row_text(&s, 9), "");
    }

    #[test]
    fn button_click_requests() {
        let mut s = MusicLibraryListState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, LIST_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Requested);
    }
}
