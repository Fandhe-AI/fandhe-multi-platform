//! Phase 11（端末機能の拡充ループ、親 #145）型サンプル「書類スキャン」（Issue #747）。
//! OS のスキャン UI で書類を撮影し、補正された 1 ページ目の画像を表示する 1 画面。
//! `FANDHE_PROXY_APP=document_scan_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod document_scan_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/document_scan.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_scan`] で状態を進める。全対象 OS の分の状態・文言は
//! ここで定義するため、OS 別の子 issue（#748）はこのファイルを触らない。
//!
//! キャンセルは失敗ではなく状態として出す（契約表 10）。画像の中身と OS のエラー文は
//! エラー文に含めない（契約表 8）。画像の画素は持たず、`image_store` への識別子だけを持つ。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_scan（Running かつ世代一致のみ）--> Idle（result に Ok(結果) か Err(失敗)）
//! ```

use crate::{ImageWidget, Node, Screen, WidgetId};

const SCAN_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const IMAGE_ID: WidgetId = WidgetId::from_raw(2);

/// タイトル。
pub const TITLE: &str = "書類スキャン";
/// ボタンの文言。
pub const SCAN_LABEL: &str = "書類をスキャン";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "OS の撮影画面で書類を撮影し、補正された 1 ページ目を表示します（対応は iOS のみ。2 ページ目以降は表示しません。Android・macOS・Windows では使えません）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "撮影画面を開いています…";
/// キャンセルしたときの文言（エラーではなく状態として出す）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "スキャンできませんでした: ";
/// 非対応のときの案内（接頭辞なし）。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// カメラ権限が拒否されたときの案内。
pub const PERMISSION_DENIED_MESSAGE: &str =
    "カメラの使用が許可されていません（設定アプリで許可できます）";
/// 0 ページのときの案内。
pub const EMPTY_MESSAGE: &str = "ページがありませんでした";
/// 画像が上限を超えたときの案内。
pub const TOO_LARGE_MESSAGE: &str = "画像が大きすぎます";
/// 画像を復号できないときの案内。
pub const UNDECODABLE_MESSAGE: &str = "画像を表示できませんでした";
/// 前面でないときの案内。
pub const NOT_FOREGROUND_MESSAGE: &str = "アプリが前面にありませんでした";
/// 期限切れのときの案内。
pub const TIMED_OUT_MESSAGE: &str = "時間内に開けませんでした";
/// 表示前に中断されたときの案内。
pub const INTERRUPTED_MESSAGE: &str = "表示の前に中断されました";
/// それ以外の失敗の案内。
pub const INTERNAL_MESSAGE: &str = "原因不明のエラーが起きました";

/// 「{n} ページ中 1 ページ目を表示しています（{w}×{h}）」の文言。
fn scanned_status(page: &ShownPage) -> String {
    format!(
        "{} ページ中 1 ページ目を表示しています（{}×{}）",
        page.page_count, page.width, page.height
    )
}

/// 表示中の 1 ページ目の情報（画素は `image_store` にあり、ここには持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownPage {
    /// 画像ウィジェットが読むソース識別子（`scan:{世代}`）。
    pub source: String,
    /// 形式名（例: `JPEG`）。
    pub format: String,
    /// 元のバイト数。
    pub size: usize,
    /// 元の幅（画素）。
    pub width: u32,
    /// 元の高さ（画素）。
    pub height: u32,
    /// スキャン全体のページ数。
    pub page_count: u32,
}

/// スキャンできなかった理由（`fandhe_device::document_scan::DocumentScanFailure` から写す責務は
/// effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// 非対応。
    Unsupported,
    /// カメラ権限の拒否・制限。
    PermissionDenied,
    /// 0 ページ。
    Empty,
    /// 画像が上限超過。
    TooLarge,
    /// 画像を復号できない。
    Undecodable,
    /// 前面でない。
    NotForeground,
    /// 撮影 UI を出すまでの期限切れ。
    TimedOut,
    /// 表示前の中断。
    Interrupted,
    /// それ以外。
    Internal,
}

/// 成功側の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreOutcome {
    /// スキャンした。
    Scanned(ShownPage),
    /// 利用者が閉じた。
    Cancelled,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作、または完了後。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 撮影 UI を開いている（利用者が撮るか閉じるまで）。
    Running,
}

/// 書類スキャン画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentScanShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。未完了なら `None`。
    pub result: Option<Result<CoreOutcome, CoreFailure>>,
}

fn is_busy(state: &DocumentScanShowState) -> bool {
    state.phase != Phase::Idle
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_scan(state: &mut DocumentScanShowState) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.result = None;
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_scan`] へ
/// そのまま渡す。
pub fn take_request(state: &DocumentScanShowState) -> Option<u64> {
    (state.phase == Phase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut DocumentScanShowState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み・未要求）は無視する。反映したかどうかを返す。
pub fn finish_scan(
    state: &mut DocumentScanShowState,
    generation: u64,
    result: Result<CoreOutcome, CoreFailure>,
) -> bool {
    if state.phase != Phase::Running || state.generation != generation {
        return false;
    }
    state.phase = Phase::Idle;
    state.result = Some(result);
    true
}

/// 画像ウィジェットが読むソース識別子。スキャン成功時のみ。それ以外は空文字列。
pub fn image_source(state: &DocumentScanShowState) -> String {
    match &state.result {
        Some(Ok(CoreOutcome::Scanned(p))) => p.source.clone(),
        _ => String::new(),
    }
}

/// 状態の案内文言（処理中、スキャンした、キャンセル）。
pub fn status_text(state: &DocumentScanShowState) -> String {
    if is_busy(state) {
        return RUNNING_STATUS.to_string();
    }
    match &state.result {
        Some(Ok(CoreOutcome::Scanned(p))) => scanned_status(p),
        Some(Ok(CoreOutcome::Cancelled)) => CANCELLED_STATUS.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。画像・OS の文言は含めない。
pub fn error_text(state: &DocumentScanShowState) -> String {
    let Some(Err(f)) = state.result else {
        return String::new();
    };
    let message = match f {
        CoreFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        CoreFailure::PermissionDenied => PERMISSION_DENIED_MESSAGE,
        CoreFailure::Empty => EMPTY_MESSAGE,
        CoreFailure::TooLarge => TOO_LARGE_MESSAGE,
        CoreFailure::Undecodable => UNDECODABLE_MESSAGE,
        CoreFailure::NotForeground => NOT_FOREGROUND_MESSAGE,
        CoreFailure::TimedOut => TIMED_OUT_MESSAGE,
        CoreFailure::Interrupted => INTERRUPTED_MESSAGE,
        CoreFailure::Internal => INTERNAL_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 画面を組み立てる。画像ノードは常に置き、ボタンは処理中以外のときだけ出す
/// （取り消しボタンは置かない）。
pub fn current_screen(state: &DocumentScanShowState) -> Screen<DocumentScanShowState> {
    let screen = Screen::new(TITLE)
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .with_node(Node::Widget(Box::new(ImageWidget::bound(
            IMAGE_ID,
            image_source,
            "スキャンした書類",
        ))));
    if is_busy(state) {
        return screen;
    }
    screen.button(SCAN_BUTTON_ID, SCAN_LABEL, request_scan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> DocumentScanShowState {
        let mut s = DocumentScanShowState::default();
        request_scan(&mut s);
        mark_running(&mut s);
        s
    }

    fn page() -> ShownPage {
        ShownPage {
            source: "scan:1".into(),
            format: "JPEG".into(),
            size: 10,
            width: 640,
            height: 480,
            page_count: 3,
        }
    }

    const ALL: [CoreFailure; 9] = [
        CoreFailure::Unsupported,
        CoreFailure::PermissionDenied,
        CoreFailure::Empty,
        CoreFailure::TooLarge,
        CoreFailure::Undecodable,
        CoreFailure::NotForeground,
        CoreFailure::TimedOut,
        CoreFailure::Interrupted,
        CoreFailure::Internal,
    ];

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = DocumentScanShowState::default();
        request_scan(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_scan(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
        request_scan(&mut s);
        assert_eq!(s.generation, 1);
        assert!(finish_scan(&mut s, 1, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Idle);
        request_scan(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(s.result, None);
    }

    #[test]
    fn stale_and_idle_finish_are_ignored() {
        let mut s = running();
        assert!(!finish_scan(&mut s, 0, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Running);
        assert!(finish_scan(&mut s, 1, Err(CoreFailure::TimedOut)));
        // 期限で確定済みの後に届いた成功は適用しない
        assert!(!finish_scan(&mut s, 1, Ok(CoreOutcome::Scanned(page()))));
        assert_eq!(s.result, Some(Err(CoreFailure::TimedOut)));
        let mut idle = DocumentScanShowState::default();
        assert!(!finish_scan(&mut idle, 0, Ok(CoreOutcome::Cancelled)));
        assert_eq!(idle.result, None);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = DocumentScanShowState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn cancelled_is_a_state_not_an_error() {
        let mut s = running();
        finish_scan(&mut s, 1, Ok(CoreOutcome::Cancelled));
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), CANCELLED_STATUS);
        assert_eq!(image_source(&s), "");
    }

    #[test]
    fn every_failure_shows_error_and_no_status_or_image() {
        for f in ALL {
            let mut s = running();
            finish_scan(&mut s, 1, Err(f));
            let t = error_text(&s);
            assert!(!t.is_empty(), "{f:?}");
            assert_eq!(status_text(&s), "", "{f:?}");
            assert_eq!(image_source(&s), "", "{f:?}");
            if f == CoreFailure::Unsupported {
                assert_eq!(t, UNSUPPORTED_MESSAGE);
            } else {
                assert!(t.starts_with(FAILED_PREFIX), "{t}");
            }
        }
    }

    #[test]
    fn scanned_shows_page_info_and_image_source() {
        let mut s = running();
        finish_scan(&mut s, 1, Ok(CoreOutcome::Scanned(page())));
        assert_eq!(
            status_text(&s),
            "3 ページ中 1 ページ目を表示しています（640×480）"
        );
        assert_eq!(error_text(&s), "");
        assert_eq!(image_source(&s), "scan:1");
    }

    #[test]
    fn button_hidden_while_busy_and_image_node_always_present() {
        // 固定 4 ノード（案内・状態・失敗・画像）+ ボタン 1 つ
        let count = |s: &DocumentScanShowState| current_screen(s).len() - 4;
        let mut s = DocumentScanShowState::default();
        assert_eq!(count(&s), 1);
        request_scan(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_scan(&mut s, 1, Err(CoreFailure::Interrupted));
        assert_eq!(count(&s), 1);
    }
}
