//! Phase 11（端末機能の拡充ループ、親 #1145）型サンプル「OS 搭載フォント」（Issue #1376）。
//! 「一覧を取得」ボタンで端末にあるフォントのファミリー名を OS から 1 回読んで一覧にし、
//! 「再取得」で読み直す 1 画面。`FANDHE_PROXY_APP=local_fonts_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod local_fonts_show;` と対）。範囲は名前の一覧のみで、選んで描画に
//! 使うことは扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節）。OS 呼び出しは `fandhe-egui-proxy` の
//! `device/local_fonts.rs` が [`take_request`] で要求を観測し [`finish_read`] で結果を注入する
//! （`audio_devices_show.rs` と同型の状態機械）。
//!
//! **起動時には読まない**。初期状態は `Idle` でボタンを出し、押下で初めて要求を立てる。Web の
//! `queryLocalFonts()` はユーザー操作の外では `SecurityError` になり、開いただけで許可ダイアログを
//! 出さないためにも全 OS で揃える。
//!
//! 全対象 OS（macOS / Windows / iOS / Android / Web）の文言・状態をここで確定済みで、後続の
//! OS 実装（#1377 / #1378 / #1379）は本画面を触らない。[`ReadFailure::PermissionDenied`] は
//! Web だけが使う。
//!
//! # 0 件と失敗の区別
//!
//! 成功で 0 件は「0 件」と明示し、失敗の文言とは別に出す（`device-bridge-contracts.md`「6. 嘘の成功」）。
//! 非対応・拒否・失敗では行を残さない。
//!
//! ```text
//! 起動 = Idle（generation 0）
//! Idle --「一覧を取得」/「再取得」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_reading（ワーカー OS のみ。同期 OS は省略可）--> Reading
//!   --finish_read（世代が一致する完了のみ反映）--> Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// 取得ボタンの `WidgetId`。
const FETCH_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// `VirtualList` の 1 行あたりの固定高さ。
const ROW_EXTENT: f32 = 24.0;

/// 画面タイトル。
pub const TITLE: &str = "OS 搭載フォント";
/// 初回のボタンのラベル。
pub const FETCH_LABEL: &str = "一覧を取得";
/// 取得後のボタンのラベル。
pub const REFETCH_LABEL: &str = "再取得";
/// 読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 件数表示の末尾（`{n} 件`）。
pub const COUNT_SUFFIX: &str = " 件";
/// 上限で切り捨てたときの案内。
pub const TRUNCATED_TEXT: &str = "ほか多数（上限 1,000 件）";
/// 非対応のときの案内（失敗として表示する）。
pub const UNSUPPORTED_STATUS: &str = "この環境ではフォントの一覧を取得できません";
/// 許可されなかったときの案内（Web だけが使う。失敗とは別の文言）。
pub const PERMISSION_DENIED_STATUS: &str = "許可されませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 読み取った一覧。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalFontsReading {
    /// ファミリー名（整形・重複除去・昇順は device 側で済み）。
    pub families: Vec<String>,
    /// 上限で切り捨てたか。
    pub truncated: bool,
}

/// 読み取り操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 要求が立ち、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS から読んでいる最中（[`mark_reading`] 済み）。
    Reading,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// 非対応。
    Unsupported,
    /// 許可されなかった（Web だけが使う）。
    PermissionDenied,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalFontsShowState {
    /// 操作の進行状態。
    pub phase: ReadPhase,
    /// 直近に立てた要求の世代番号。完了通知が「今の要求」を指しているかの照合に使う。
    pub generation: u64,
    /// 読み取れた一覧。失敗・読み取り中は `None`（前回値を残さない）。
    pub reading: Option<LocalFontsReading>,
    /// 案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功時は空。
    pub error: String,
    /// 一度でも取得を押したか（ボタンのラベル切り替え用）。
    pub attempted: bool,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRequest {
    /// [`finish_read`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// ボタンのハンドラ。`Idle` 以外では二重要求を防ぐため no-op。
pub fn fetch(state: &mut LocalFontsShowState) {
    if state.phase != ReadPhase::Idle {
        return;
    }
    state.phase = ReadPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.attempted = true;
    // 前回の結果を残さない。
    state.reading = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない）。
pub fn take_request(state: &LocalFontsShowState) -> Option<ReadRequest> {
    (state.phase == ReadPhase::Requested).then_some(ReadRequest {
        generation: state.generation,
    })
}

/// 読み取り中（`Reading`）へ進める。`Requested` 以外では何もしない。
pub fn mark_reading(state: &mut LocalFontsShowState) {
    if state.phase == ReadPhase::Requested {
        state.phase = ReadPhase::Reading;
        state.status = READING_STATUS.to_string();
    }
}

/// 結果を反映する。現在の世代と一致しない、または `Idle` で届いた完了は無視する。
pub fn finish_read(
    state: &mut LocalFontsShowState,
    generation: u64,
    result: Result<LocalFontsReading, ReadFailure>,
) {
    if state.phase == ReadPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = ReadPhase::Idle;
    state.reading = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(r) => state.reading = Some(r),
        Err(ReadFailure::Unsupported) => state.error = UNSUPPORTED_STATUS.to_string(),
        Err(ReadFailure::PermissionDenied) => state.error = PERMISSION_DENIED_STATUS.to_string(),
        Err(ReadFailure::Failed(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。読み取り中は案内、成功時は件数（切り捨て時は続けて案内）。
pub fn status_text(state: &LocalFontsShowState) -> String {
    match &state.reading {
        Some(r) if r.truncated => format!("{}{COUNT_SUFFIX}\n{TRUNCATED_TEXT}", r.families.len()),
        Some(r) => format!("{}{COUNT_SUFFIX}", r.families.len()),
        None => state.status.clone(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &LocalFontsShowState) -> String {
    state.error.clone()
}

/// `VirtualList` の総行数。
pub fn family_count(state: &LocalFontsShowState) -> usize {
    state.reading.as_ref().map_or(0, |r| r.families.len())
}

/// `VirtualList` の 1 行。範囲外は panic せず空テキスト。
pub fn family_row(state: &LocalFontsShowState, index: usize) -> Node<LocalFontsShowState> {
    let name = state
        .reading
        .as_ref()
        .and_then(|r| r.families.get(index))
        .cloned()
        .unwrap_or_default();
    Node::Text(name)
}

/// 画面を組み立てる。ボタンは常に出す（処理中に隠すと `VirtualList` のノードパスが変わり、
/// egui が持つスクロール位置が再取得後に残るため）。処理中の押下は `fetch` が無視する。
pub fn current_screen(state: &LocalFontsShowState) -> Screen<LocalFontsShowState> {
    let label = if state.attempted {
        REFETCH_LABEL
    } else {
        FETCH_LABEL
    };
    Screen::new(TITLE)
        .dynamic_text(status_text)
        .error_text(error_text)
        .button(FETCH_BUTTON_ID, label, fetch)
        .virtual_list(family_count, family_row, ROW_EXTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn reading(n: usize, truncated: bool) -> LocalFontsReading {
        LocalFontsReading {
            families: (0..n).map(|i| format!("F{i}")).collect(),
            truncated,
        }
    }

    #[test]
    fn startup_does_not_request() {
        let state = LocalFontsShowState::default();
        assert_eq!(take_request(&state), None);
        assert_eq!(state.phase, ReadPhase::Idle);
    }

    #[test]
    fn fetch_requests_and_is_noop_while_in_flight() {
        let mut state = LocalFontsShowState::default();
        fetch(&mut state);
        assert_eq!(take_request(&state), Some(ReadRequest { generation: 1 }));
        mark_reading(&mut state);
        fetch(&mut state);
        assert_eq!(state.phase, ReadPhase::Reading);
        assert_eq!(state.generation, 1);
        assert_eq!(state.status, READING_STATUS);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut state = LocalFontsShowState::default();
        fetch(&mut state);
        finish_read(&mut state, 99, Ok(reading(1, false)));
        assert_eq!(state.phase, ReadPhase::Requested);
        assert_eq!(state.reading, None);
        let mut idle = LocalFontsShowState::default();
        finish_read(&mut idle, 0, Ok(reading(1, false)));
        assert_eq!(idle.reading, None);
    }

    #[test]
    fn failure_clears_previous_reading() {
        let mut state = LocalFontsShowState::default();
        fetch(&mut state);
        finish_read(&mut state, 1, Ok(reading(2, false)));
        assert_eq!(family_count(&state), 2);
        fetch(&mut state);
        assert_eq!(state.reading, None);
        finish_read(&mut state, 2, Err(ReadFailure::Failed("x".into())));
        assert_eq!(family_count(&state), 0);
        assert_eq!(status_text(&state), "");
        assert_eq!(state.error, format!("{FAILED_PREFIX}x"));
    }

    #[test]
    fn failures_have_distinct_texts_and_no_success_text() {
        let mut texts = Vec::new();
        for (i, f) in [
            ReadFailure::Unsupported,
            ReadFailure::PermissionDenied,
            ReadFailure::Failed("x".into()),
        ]
        .into_iter()
        .enumerate()
        {
            let mut state = LocalFontsShowState::default();
            fetch(&mut state);
            finish_read(&mut state, 1, Err(f));
            assert_eq!(state.phase, ReadPhase::Idle, "{i}");
            assert_eq!(status_text(&state), "");
            assert_eq!(state.reading, None);
            texts.push(state.error);
        }
        assert_eq!(texts[0], UNSUPPORTED_STATUS);
        assert_eq!(texts[1], PERMISSION_DENIED_STATUS);
        assert!(texts[2].starts_with(FAILED_PREFIX));
    }

    #[test]
    fn zero_is_shown_as_zero_not_failure() {
        let mut state = LocalFontsShowState::default();
        fetch(&mut state);
        finish_read(&mut state, 1, Ok(reading(0, false)));
        assert_eq!(status_text(&state), "0 件");
        assert_eq!(state.error, "");
    }

    #[test]
    fn truncated_adds_notice() {
        let mut state = LocalFontsShowState::default();
        fetch(&mut state);
        finish_read(&mut state, 1, Ok(reading(3, true)));
        assert_eq!(status_text(&state), format!("3 件\n{TRUNCATED_TEXT}"));
    }

    #[test]
    fn button_stays_in_flight_and_label_switches() {
        let mut state = LocalFontsShowState::default();
        let screen = current_screen(&state);
        assert!(!click_button(&screen, REFETCH_LABEL, &mut state));
        assert!(click_button(&screen, FETCH_LABEL, &mut state));
        // 処理中もボタンは残り（パス不変）、押しても要求は増えない。
        let screen = current_screen(&state);
        let before = state.phase;
        assert!(click_button(&screen, REFETCH_LABEL, &mut state));
        assert_eq!(state.phase, before);
        finish_read(&mut state, 1, Ok(reading(1, false)));
        let screen = current_screen(&state);
        assert!(!click_button(&screen, FETCH_LABEL, &mut state));
        assert!(click_button(&screen, REFETCH_LABEL, &mut state));
    }

    #[test]
    fn out_of_range_row_does_not_panic() {
        let mut state = LocalFontsShowState::default();
        assert!(matches!(family_row(&state, 5), Node::Text(t) if t.is_empty()));
        fetch(&mut state);
        finish_read(&mut state, 1, Ok(reading(1, false)));
        assert!(matches!(family_row(&state, 0), Node::Text(t) if t == "F0"));
        assert!(matches!(family_row(&state, 9), Node::Text(t) if t.is_empty()));
    }
}
