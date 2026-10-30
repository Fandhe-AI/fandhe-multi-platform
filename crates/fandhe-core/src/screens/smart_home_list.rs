//! Phase 11（端末機能の拡充ループ、親 #147）型サンプル「スマートホーム」（Issue #1079）。
//! 「家と機器の一覧を読む」ボタン → 端末に登録済みの家の数と機器の名前を OS の返す順で先頭 20 件だけ
//! 読んで一覧にする 1 画面。`FANDHE_PROXY_APP=smart_home_list` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod smart_home_list;` と対）。機器の操作・状態は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/smart_home.rs` が [`take_request`] で要求を観測し [`finish_list`] で結果を注入する
//! （`music_library_list.rs` と同型の状態機械）。
//!
//! 全対象 OS の文言・状態をここで確定済みで、後続の OS 実装（iOS の #1080）は本画面を触らない。
//! 起動時には読まない（他の画面で HomeKit の許可ダイアログを出さないため）。
//!
//! # 0 件と失敗の区別
//!
//! 家 0 件と、家はあるが機器 0 件は別の文言を出し、どちらも失敗の文言とは別にする。失敗時は一覧も
//! 0 件の文言も残さない（`device-bridge-contracts.md`「6. 嘘の成功」）。
//!
//! ```text
//! Idle --「家と機器の一覧を読む」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_loading--> Loading
//!   --finish_list（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// 読み取りボタンの `WidgetId`。
const LIST_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// `VirtualList` の 1 行あたりの固定高さ。
const ROW_EXTENT: f32 = 24.0;

/// 画面タイトル。
pub const TITLE: &str = "スマートホーム";
/// 画面の説明。
pub const NOTE: &str = "端末に登録済みの家と機器の名前を先頭 20 件だけ表示します（操作はしません）";
/// ボタンのラベル。
pub const LIST_LABEL: &str = "家と機器の一覧を読む";
/// 読み込み中の案内。
pub const LOADING_STATUS: &str = "読み込み中…";
/// 家が 0 件だったときの案内（失敗ではない）。
pub const NO_HOMES_STATUS: &str = "家が登録されていません（0 件）";
/// 家の件数の接頭辞（「家: N 件」の形で出す）。
pub const HOME_COUNT_PREFIX: &str = "家: ";
/// 切り捨てたときの案内。
pub const TRUNCATED_TEXT: &str = "先頭 20 件だけ表示しています";
/// 家の名前が無いときの代替表示。
pub const NO_HOME_NAME: &str = "（家の名前なし）";
/// 機器の名前が無いときの代替表示。
pub const NO_ACCESSORY_NAME: &str = "（機器の名前なし）";
/// 許可されなかったときの案内。
pub const PERMISSION_DENIED_STATUS: &str =
    "許可されていません。設定アプリの HomeKit から変えられます";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// 期限切れのときの案内。
pub const TIMEOUT_STATUS: &str = "応答がありませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{固定文言}` の形で表示する）。
pub const FAILED_PREFIX: &str = "読めませんでした: ";

/// 家は N 件あるが機器が 0 件のときの案内（失敗ではない）。
pub fn no_accessories_status(home_count: usize) -> String {
    format!("家は {home_count} 件ありますが、機器がありません（0 件）")
}

/// 一覧の 1 行（家の名前, 機器の名前。無い項目は `None`）。
pub type AccessoryRow = (Option<String>, Option<String>);

/// 読み取れた一覧。
///
/// `Debug` は家・機器の名前を出さない（件数と切り捨て有無のみ。ログ・panic 経由の漏えい防止）。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct HomeListing {
    /// 登録済みの家の数。
    pub home_count: usize,
    /// 表示する行（OS の返す順の先頭 20 件まで）。
    pub rows: Vec<AccessoryRow>,
    /// 20 件を超えて切り捨てたか。
    pub truncated: bool,
}

impl std::fmt::Debug for HomeListing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeListing")
            .field("home_count", &self.home_count)
            .field("rows", &self.rows.len())
            .field("truncated", &self.truncated)
            .finish()
    }
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

/// 失敗の種別（`fandhe_device::smart_home::SmartHomeFailure` から写す責務は effect 側）。
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

/// スマートホーム画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmartHomeListState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた一覧。失敗・読み込み中は `None`（0 件は `Some` の空）。
    pub listing: Option<HomeListing>,
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
pub fn fetch(state: &mut SmartHomeListState) {
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
pub fn take_request(state: &SmartHomeListState) -> Option<ListRequest> {
    (state.phase == Phase::Requested).then_some(ListRequest {
        generation: state.generation,
    })
}

/// 読み込み中（`Loading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_loading(state: &mut SmartHomeListState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Loading;
        state.status = LOADING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_list(
    state: &mut SmartHomeListState,
    generation: u64,
    result: Result<HomeListing, ListFailure>,
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

/// 案内文言（`DynamicText` の `get`）。読み込み中は案内、成功時は家 0 件・機器 0 件の案内か
/// 「家: N 件」（切り捨てがあれば案内を続ける）。
pub fn status_text(state: &SmartHomeListState) -> String {
    match &state.listing {
        Some(l) if l.home_count == 0 => NO_HOMES_STATUS.to_string(),
        Some(l) if l.rows.is_empty() => no_accessories_status(l.home_count),
        Some(l) if l.truncated => {
            format!("{HOME_COUNT_PREFIX}{} 件。{TRUNCATED_TEXT}", l.home_count)
        }
        Some(l) => format!("{HOME_COUNT_PREFIX}{} 件", l.home_count),
        None => state.status.clone(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &SmartHomeListState) -> String {
    state.error.clone()
}

/// `VirtualList` の総行数。
pub fn row_count(state: &SmartHomeListState) -> usize {
    state.listing.as_ref().map_or(0, |l| l.rows.len())
}

/// 1 行の表示文字列（「家の名前 — 機器の名前」）。範囲外は panic せず空。
pub fn row_text(state: &SmartHomeListState, index: usize) -> String {
    state
        .listing
        .as_ref()
        .and_then(|l| l.rows.get(index))
        .map(|(home, name)| {
            format!(
                "{} — {}",
                home.as_deref().unwrap_or(NO_HOME_NAME),
                name.as_deref().unwrap_or(NO_ACCESSORY_NAME)
            )
        })
        .unwrap_or_default()
}

/// `VirtualList` の 1 行。
pub fn accessory_row(state: &SmartHomeListState, index: usize) -> Node<SmartHomeListState> {
    Node::Text(row_text(state, index))
}

/// 画面を組み立てる。ボタンは常に出す（隠すと `VirtualList` のノードパスが変わりスクロール位置が
/// 残るため。`music_library_list` と同じ）。処理中の押下は [`fetch`] が無視する。
pub fn current_screen(_state: &SmartHomeListState) -> Screen<SmartHomeListState> {
    Screen::new(TITLE)
        .text(NOTE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .button(LIST_BUTTON_ID, LIST_LABEL, fetch)
        .virtual_list(row_count, accessory_row, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn listing(homes: usize, n: usize, truncated: bool) -> HomeListing {
        HomeListing {
            home_count: homes,
            rows: (0..n)
                .map(|i| (Some(format!("H{i}")), Some(format!("A{i}"))))
                .collect(),
            truncated,
        }
    }

    fn loading() -> SmartHomeListState {
        let mut s = SmartHomeListState::default();
        fetch(&mut s);
        mark_loading(&mut s);
        s
    }

    #[test]
    fn debug_output_hides_names() {
        let state = SmartHomeListState {
            listing: Some(listing(1, 2, false)),
            ..Default::default()
        };
        let out = format!("{state:?}");
        assert!(!out.contains("H0") && !out.contains("A1"), "{out}");
    }

    #[test]
    fn startup_does_not_request() {
        let s = SmartHomeListState::default();
        assert_eq!(take_request(&s), None);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn fetch_is_noop_while_in_flight() {
        let mut s = SmartHomeListState::default();
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
        finish_list(&mut s, 99, Ok(listing(1, 1, false)));
        assert_eq!(s.phase, Phase::Loading);
        assert_eq!(s.listing, None);
        let mut idle = SmartHomeListState::default();
        finish_list(&mut idle, 0, Ok(listing(1, 1, false)));
        assert_eq!(idle.listing, None);
    }

    #[test]
    fn zero_homes_and_zero_accessories_are_distinct_successes() {
        let mut s = loading();
        finish_list(&mut s, 1, Ok(listing(0, 0, false)));
        assert_eq!(status_text(&s), NO_HOMES_STATUS);
        assert_eq!(s.error, "");
        let mut s = loading();
        finish_list(&mut s, 1, Ok(listing(2, 0, false)));
        assert_eq!(status_text(&s), no_accessories_status(2));
        assert_ne!(status_text(&s), NO_HOMES_STATUS);
        assert_eq!(s.error, "");
    }

    #[test]
    fn every_failure_is_distinct_and_leaves_no_list() {
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
            s.listing = Some(listing(1, 3, false));
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
        finish_list(&mut s, 1, Ok(listing(2, 20, true)));
        assert_eq!(
            status_text(&s),
            format!("{HOME_COUNT_PREFIX}2 件。{TRUNCATED_TEXT}")
        );
        s.listing = Some(HomeListing {
            home_count: 1,
            rows: vec![(None, None), (Some("h".into()), None)],
            truncated: false,
        });
        assert_eq!(status_text(&s), format!("{HOME_COUNT_PREFIX}1 件"));
        assert_eq!(
            row_text(&s, 0),
            format!("{NO_HOME_NAME} — {NO_ACCESSORY_NAME}")
        );
        assert_eq!(row_text(&s, 1), format!("h — {NO_ACCESSORY_NAME}"));
        assert_eq!(row_text(&s, 9), "");
    }

    #[test]
    fn button_click_requests() {
        let mut s = SmartHomeListState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, LIST_LABEL, &mut s));
        assert_eq!(s.phase, Phase::Requested);
    }
}
