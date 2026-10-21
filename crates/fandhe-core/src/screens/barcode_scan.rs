//! Phase 11（端末機能の拡充ループ）型サンプル「バーコード・QR 読み取り」（Issue #557、親 #116）。
//! 「写真を選んでコードを読む」ボタン → OS の画像選択 → 選んだ 1 枚のバーコード・QR を読んで
//! 中身の文字列を一覧表示する 1 画面（URL でも自動で開かない）。`FANDHE_PROXY_APP=barcode_scan` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod barcode_scan;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは本モジュール外
//! （`fandhe-egui-proxy` の `device/barcode_scan.rs`）が [`take_request`] で要求を観測し、
//! [`mark_scanning`]・[`finish_scan`] で状態を進める。全対象 OS（macOS / iOS / Web）
//! ※対象 OS はこの 3 つ。同じ UI・文言を使うため、OS 別の子 issue はこのファイルを触らない。
//!
//! # 成功・キャンセル・失敗を混同しない
//!
//! [`finish_scan`] の `Ok(Some)` は成功（空の一覧は「コードなし」の成功）、`Ok(None)` は
//! 写真選択のキャンセル、`Err` は失敗。成功・キャンセルは `status`（通常色）、失敗は
//! `error`（`TextRole::Error`）へ書き分け、失敗・キャンセルでは `payloads` を必ず空に戻す。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1)
//!   --effect--> Scanning (写真選択 → 読み取り)
//!   --finish_scan（世代一致のみ）--> Idle + 結果表示
//! ```

use crate::{Node, Screen, WidgetId};

/// ボタンの `WidgetId`。
const SCAN_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "「写真を選んでコードを読む」で画像を 1 枚選びます";
/// 選択・読み取りの最中の案内。
pub const SCANNING_STATUS: &str = "読み取っています…";
/// 写真選択がキャンセルされたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "選択しませんでした";
/// コードが 1 件も見つからなかったときの案内（成功。失敗色にしない）。
pub const NO_CODE_STATUS: &str = "コードは見つかりませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では使えません";
/// ブラウザがバーコード検出に対応しないときの案内（Web 用。#559）。
pub const BROWSER_UNSUPPORTED_STATUS: &str = "このブラウザではコードを読めません";
/// 画像の辺が上限を超えるときの案内。
pub const INVALID_INPUT_STATUS: &str = "画像が大きすぎます";

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarcodeScanPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 写真選択または読み取りの最中。
    Scanning,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側。
/// `fandhe-core` は `fandhe-device` に依存しないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanFailure {
    /// 非対応 OS。
    Unsupported,
    /// ブラウザがバーコード検出に対応しない（Web）。
    BrowserUnsupported,
    /// 画像の辺が上限を超える。
    InvalidInput,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// バーコード・QR 読み取り画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BarcodeScanState {
    /// 操作の進行状態。
    pub phase: BarcodeScanPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 読み取った中身の文字列。件数は `payloads.len()`。
    pub payloads: Vec<String>,
    /// 成功・キャンセル・処理中の案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。
    pub error: String,
}

/// ボタンのハンドラ。`Idle` 以外は no-op。前回の結果は新しい要求の時点で消す。
pub fn request_scan(state: &mut BarcodeScanState) {
    if state.phase != BarcodeScanPhase::Idle {
        return;
    }
    state.phase = BarcodeScanPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.payloads.clear();
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。戻り値の世代番号は
/// [`finish_scan`] へそのまま渡す契約。
pub fn take_request(state: &BarcodeScanState) -> Option<u64> {
    (state.phase == BarcodeScanPhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_scanning(state: &mut BarcodeScanState) {
    if state.phase == BarcodeScanPhase::Requested {
        state.phase = BarcodeScanPhase::Scanning;
        state.status = SCANNING_STATUS.to_string();
    }
}

/// 結果を反映する。`Ok(Some)` は読み取り結果（空はコードなしの成功）、`Ok(None)` はキャンセル、
/// `Err` は失敗。`Idle` のとき・世代が一致しないときは無視する。
pub fn finish_scan(
    state: &mut BarcodeScanState,
    generation: u64,
    result: Result<Option<Vec<String>>, ScanFailure>,
) {
    if state.phase == BarcodeScanPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = BarcodeScanPhase::Idle;
    state.error = String::new();
    match result {
        Ok(Some(payloads)) => {
            state.status = if payloads.is_empty() {
                NO_CODE_STATUS.to_string()
            } else {
                format!("{} 件", payloads.len())
            };
            state.payloads = payloads;
        }
        Ok(None) => {
            state.status = CANCELLED_STATUS.to_string();
            state.payloads.clear();
        }
        Err(failure) => {
            state.status = String::new();
            state.payloads.clear();
            state.error = match failure {
                ScanFailure::Unsupported => UNSUPPORTED_STATUS.to_string(),
                ScanFailure::BrowserUnsupported => BROWSER_UNSUPPORTED_STATUS.to_string(),
                ScanFailure::InvalidInput => INVALID_INPUT_STATUS.to_string(),
                ScanFailure::Failed(message) => format!("{FAILED_PREFIX}{message}"),
            };
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &BarcodeScanState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &BarcodeScanState) -> String {
    state.error.clone()
}

/// 読み取り結果を `Node::Text` 一覧へ展開する（`.list()` の `expand`）。
pub fn payload_nodes(state: &BarcodeScanState) -> Vec<Node<BarcodeScanState>> {
    state.payloads.iter().cloned().map(Node::Text).collect()
}

/// バーコード・QR 読み取り画面を組み立てる。ボタンは `Idle` のときだけ出す。
pub fn current_screen(state: &BarcodeScanState) -> Screen<BarcodeScanState> {
    let screen = Screen::new("バーコード・QR 読み取り")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(payload_nodes);
    if state.phase != BarcodeScanPhase::Idle {
        return screen;
    }
    screen.button(SCAN_BUTTON_ID, "写真を選んでコードを読む", request_scan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanning() -> BarcodeScanState {
        let mut s = BarcodeScanState::default();
        request_scan(&mut s);
        mark_scanning(&mut s);
        s
    }

    #[test]
    fn request_advances_generation_and_clears_previous_result() {
        let mut s = BarcodeScanState {
            payloads: vec!["old".into()],
            status: "1 件".into(),
            ..Default::default()
        };
        request_scan(&mut s);
        assert_eq!(s.phase, BarcodeScanPhase::Requested);
        assert_eq!(s.generation, 1);
        assert!(s.payloads.is_empty() && s.status.is_empty());
        assert_eq!(take_request(&s), Some(1));
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = scanning();
        request_scan(&mut s);
        assert_eq!(s.phase, BarcodeScanPhase::Scanning);
        assert_eq!(s.generation, 1);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn success_shows_payload_count() {
        let mut s = scanning();
        finish_scan(&mut s, 1, Ok(Some(vec!["a".into(), "b".into()])));
        assert_eq!(s.phase, BarcodeScanPhase::Idle);
        assert_eq!(s.status, "2 件");
        assert_eq!(s.payloads, ["a", "b"]);
        assert!(s.error.is_empty());
    }

    #[test]
    fn empty_result_is_no_code_success_not_error() {
        let mut s = scanning();
        finish_scan(&mut s, 1, Ok(Some(vec![])));
        assert_eq!(s.status, NO_CODE_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn cancel_is_not_failure_and_clears_payloads() {
        let mut s = scanning();
        finish_scan(&mut s, 1, Ok(None));
        assert_eq!(s.status, CANCELLED_STATUS);
        assert!(s.error.is_empty() && s.payloads.is_empty());
    }

    #[test]
    fn each_failure_sets_error_only() {
        let cases = [
            (ScanFailure::Unsupported, UNSUPPORTED_STATUS.to_string()),
            (
                ScanFailure::BrowserUnsupported,
                BROWSER_UNSUPPORTED_STATUS.to_string(),
            ),
            (ScanFailure::InvalidInput, INVALID_INPUT_STATUS.to_string()),
            (ScanFailure::Failed("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (failure, expected) in cases {
            let mut s = scanning();
            s.payloads = vec!["stale".into()];
            finish_scan(&mut s, 1, Err(failure));
            assert_eq!(s.error, expected);
            assert!(s.status.is_empty() && s.payloads.is_empty());
        }
    }

    #[test]
    fn stale_generation_and_idle_completion_are_ignored() {
        let mut s = scanning();
        finish_scan(&mut s, 99, Ok(Some(vec!["x".into()])));
        assert_eq!(s.phase, BarcodeScanPhase::Scanning);
        assert!(s.payloads.is_empty());
        let mut idle = BarcodeScanState::default();
        finish_scan(&mut idle, 0, Ok(Some(vec!["x".into()])));
        assert!(idle.payloads.is_empty());
    }

    #[test]
    fn mark_scanning_only_from_requested() {
        let mut s = BarcodeScanState::default();
        mark_scanning(&mut s);
        assert_eq!(s.phase, BarcodeScanPhase::Idle);
    }

    #[test]
    fn button_only_when_idle() {
        let idle = BarcodeScanState::default();
        let busy = scanning();
        assert_eq!(current_screen(&idle).len(), current_screen(&busy).len() + 1);
    }
}
