//! Phase 11 の拡充ループ（親 #1134）型サンプル「複数ディスプレイ」（Issue #1341）。
//! `FANDHE_PROXY_APP=multi_display_show` で選ばれ、`fandhe-egui-proxy` の `screens/registry.rs`
//! の `effect` 区画（native）と `fandhe-web-dom` の `demo.rs`（Web）から呼ばれる。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `fandhe-egui-proxy` の `device/multi_display.rs` が一定間隔で行い [`apply_read`] で注入する。
//! Web は許可ダイアログを要するためボタン起点（[`request_screens`] / [`take_request`] /
//! [`finish_request`]）で、世代を照合して古い完了を捨てる。
//!
//! 嘘の一覧を出さない: 失敗・非対応・許可なしでは前回の一覧を残さず、許可なしを空の一覧に
//! しない。Web の `mount_with_effect` は初期状態から 1 回だけ画面を組むため、一覧は
//! `dynamic_text` 1 本の複数行文字列で返す。

use crate::{Screen, WidgetId};

/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 題名。
pub const TITLE: &str = "複数ディスプレイ";
/// Web の許可要求ボタンの ID。
const REQUEST_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// Web の許可要求ボタンの文言。
pub const REQUEST_LABEL: &str = "画面の一覧を読む（許可を求めます）";
/// 反映遅れ・論理ディスプレイ・ミラーリングの注記。
pub const NOTE: &str = "接続・切断の反映は最大 1 秒遅れます。一覧は OS が有効と認める論理ディスプレイで、物理ディスプレイの数と一致しないことがあります。ミラーリング時の扱いは OS によって異なります。";

/// 画面 1 枚の写し（`fandhe_device::multi_display::ScreenInfo` の写し）。
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenRow {
    /// 幅（物理ピクセル。Web は約）。
    pub width_px: u32,
    /// 高さ（物理ピクセル。Web は約）。
    pub height_px: u32,
    /// 倍率（`None` は不明）。
    pub scale: Option<f32>,
    /// 主画面か。
    pub is_primary: bool,
    /// 表示名。
    pub name: Option<String>,
}

/// 読めなかった理由。`DeviceError` からの写像は呼び出し元の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 利用者が許可しなかった（Web）。失敗とは別扱い。
    PermissionDenied,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出す状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DisplayView {
    /// native の初期。最初の値が届く前。
    #[default]
    Reading,
    /// Web の初期。ボタン待ち。
    AwaitingRequest,
    /// Web の許可ダイアログ待ち。
    Requesting,
    /// 一覧。
    Listed(Vec<ScreenRow>),
    /// 許可なし（Web）。
    PermissionDenied,
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「複数ディスプレイ」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MultiDisplayShowState {
    /// 現在の表示状態。
    pub view: DisplayView,
    /// Web の要求の世代。
    pub generation: u64,
    /// サイズが「約」の値か（Web）。
    pub size_is_approximate: bool,
}

/// Web の初期状態（ボタン待ち・サイズは約）。
pub fn web_initial_state() -> MultiDisplayShowState {
    MultiDisplayShowState {
        view: DisplayView::AwaitingRequest,
        generation: 0,
        size_is_approximate: true,
    }
}

/// 読み取り結果を反映する。毎回 view を置き換えるため、失敗時に前回の一覧は残らない。
pub fn apply_read(state: &mut MultiDisplayShowState, result: Result<Vec<ScreenRow>, ReadFailure>) {
    state.view = match result {
        Ok(rows) => DisplayView::Listed(rows),
        Err(ReadFailure::Unsupported) => DisplayView::Unsupported,
        Err(ReadFailure::PermissionDenied) => DisplayView::PermissionDenied,
        Err(ReadFailure::Failed(m)) => DisplayView::Failed(m),
    };
}

/// Web のボタンのハンドラ。`Requesting` 中は何もせず、それ以外は世代を進めて `Requesting` にする。
pub fn request_screens(state: &mut MultiDisplayShowState) {
    if state.view == DisplayView::Requesting {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.view = DisplayView::Requesting;
}

/// 未処理の要求があればその世代を返す（Web の effect が観測する）。
pub fn take_request(state: &MultiDisplayShowState) -> Option<u64> {
    (state.view == DisplayView::Requesting).then_some(state.generation)
}

/// Web の完了を反映する。世代が一致し `Requesting` のときだけ [`apply_read`] する。
pub fn finish_request(
    state: &mut MultiDisplayShowState,
    generation: u64,
    result: Result<Vec<ScreenRow>, ReadFailure>,
) {
    if state.view != DisplayView::Requesting || state.generation != generation {
        return;
    }
    apply_read(state, result);
}

fn row_text(i: usize, r: &ScreenRow, approx: bool) -> String {
    let name = r.name.clone().unwrap_or_else(|| format!("画面 {}", i + 1));
    let about = if approx { "約 " } else { "" };
    let scale = r
        .scale
        .map_or("倍率 不明".to_string(), |s| format!("×{s:.1}"));
    let primary = if r.is_primary { "・主画面" } else { "" };
    format!(
        "{name}: {about}{} × {}（{scale}）{primary}",
        r.width_px, r.height_px
    )
}

/// 一覧の文言（`Listed` 以外は空）。1 画面 1 行。
pub fn list_text(state: &MultiDisplayShowState) -> String {
    match &state.view {
        DisplayView::Listed(rows) => rows
            .iter()
            .enumerate()
            .map(|(i, r)| row_text(i, r, state.size_is_approximate))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// 状態の文言。
pub fn status_text(state: &MultiDisplayShowState) -> String {
    match &state.view {
        DisplayView::Reading => "読み取り中".to_string(),
        DisplayView::AwaitingRequest => {
            "ボタンを押すと、ブラウザが画面の情報の許可を求めます".to_string()
        }
        DisplayView::Requesting => "許可を待っています".to_string(),
        DisplayView::Listed(rows) => format!("接続中の画面: {} 枚", rows.len()),
        DisplayView::PermissionDenied => {
            "許可されませんでした。もう一度押すと再度許可を求めます".to_string()
        }
        DisplayView::Unsupported => "この環境では画面の一覧を読めません".to_string(),
        DisplayView::Failed(_) => "画面の一覧を読めませんでした".to_string(),
    }
}

/// 失敗時のみ非空。
pub fn error_text(state: &MultiDisplayShowState) -> String {
    match &state.view {
        DisplayView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 「複数ディスプレイ」画面。ボタンは Web の操作が要る状態（待ち・許可中・許可なし）だけ付く。
pub fn current_screen(state: &MultiDisplayShowState) -> Screen<MultiDisplayShowState> {
    let screen = Screen::new(TITLE)
        .dynamic_text(status_text)
        .dynamic_text(list_text)
        .error_text(error_text)
        .text(NOTE);
    match state.view {
        DisplayView::AwaitingRequest | DisplayView::Requesting | DisplayView::PermissionDenied => {
            screen.button(REQUEST_BUTTON_ID, REQUEST_LABEL, request_screens)
        }
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{click_button, render_headless};

    fn row(primary: bool, name: Option<&str>, scale: Option<f32>) -> ScreenRow {
        ScreenRow {
            width_px: 3024,
            height_px: 1964,
            scale,
            is_primary: primary,
            name: name.map(String::from),
        }
    }

    #[test]
    fn list_formats_name_primary_scale() {
        let mut s = MultiDisplayShowState::default();
        apply_read(
            &mut s,
            Ok(vec![
                row(true, Some("内蔵"), Some(2.0)),
                row(false, None, None),
            ]),
        );
        assert_eq!(
            list_text(&s),
            "内蔵: 3024 × 1964（×2.0）・主画面\n画面 2: 3024 × 1964（倍率 不明）"
        );
    }

    #[test]
    fn approximate_adds_about() {
        let mut s = web_initial_state();
        apply_read(&mut s, Ok(vec![row(false, None, Some(1.0))]));
        assert!(list_text(&s).contains("約 3024"));
    }

    #[test]
    fn failures_drop_previous_list() {
        for f in [
            ReadFailure::Unsupported,
            ReadFailure::PermissionDenied,
            ReadFailure::Failed("画面一覧の取得".into()),
        ] {
            let mut s = MultiDisplayShowState::default();
            apply_read(&mut s, Ok(vec![row(true, None, None)]));
            apply_read(&mut s, Err(f));
            assert_eq!(list_text(&s), "");
            assert!(!matches!(s.view, DisplayView::Listed(_)));
        }
        let mut s = MultiDisplayShowState::default();
        apply_read(&mut s, Err(ReadFailure::Failed("画面一覧の取得".into())));
        assert_eq!(error_text(&s), "失敗しました: 画面一覧の取得");
    }

    #[test]
    fn double_press_while_requesting_keeps_generation() {
        let mut s = web_initial_state();
        request_screens(&mut s);
        request_screens(&mut s);
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), Some(1));
    }

    #[test]
    fn stale_generation_is_ignored() {
        let mut s = web_initial_state();
        request_screens(&mut s);
        finish_request(&mut s, 0, Ok(vec![row(true, None, None)]));
        assert_eq!(s.view, DisplayView::Requesting);
        finish_request(&mut s, 1, Ok(vec![row(true, None, None)]));
        assert!(matches!(s.view, DisplayView::Listed(_)));
        // 完了後の遅れた完了は無視される。
        finish_request(&mut s, 1, Err(ReadFailure::Unsupported));
        assert!(matches!(s.view, DisplayView::Listed(_)));
    }

    #[test]
    fn permission_denied_is_not_an_empty_list() {
        let mut s = web_initial_state();
        request_screens(&mut s);
        finish_request(&mut s, 1, Err(ReadFailure::PermissionDenied));
        assert_eq!(s.view, DisplayView::PermissionDenied);
        assert!(status_text(&s).contains("許可されませんでした"));
        assert_eq!(list_text(&s), "");
        // 再度押すと再要求できる。
        request_screens(&mut s);
        assert_eq!(take_request(&s), Some(2));
    }

    #[test]
    fn button_only_for_web_states() {
        let mut native = MultiDisplayShowState::default();
        assert!(!click_button(
            &current_screen(&native),
            REQUEST_LABEL,
            &mut native
        ));
        let mut web = web_initial_state();
        assert!(click_button(&current_screen(&web), REQUEST_LABEL, &mut web));
        assert_eq!(web.view, DisplayView::Requesting);
    }

    #[test]
    fn render_headless_shows_note() {
        let s = MultiDisplayShowState::default();
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(joined.contains("読み取り中"));
        assert!(joined.contains("論理ディスプレイ"));
    }
}
