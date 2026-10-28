//! Phase 11（端末機能の拡充ループ、親 #192）型サンプル「接続した機器からの取り込み」（Issue #981）。
//! USB 等で接続したカメラ機器・スキャナーから画像を 1 枚取り込み、表示する 1 画面。
//! `FANDHE_PROXY_APP=scanner_devices_import` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod scanner_devices_import;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/scanner_devices.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_import`] で状態を進める。全対象 OS の分の状態・文言は
//! ここで定義するため、OS 別の子 issue（#982〜#984）はこのファイルを触らない。
//!
//! キャンセルという結果は無い（OS の選択 UI を出さない。契約表 10）。機器名・画像の中身・OS の
//! エラー文は画面に出さない（契約表 8）。画像の画素は持たず、`image_store` への識別子だけを持つ。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_import（Running かつ世代一致のみ）--> Idle（result に Ok(画像) か Err(失敗)）
//! ```

use crate::{ImageWidget, Node, Screen, WidgetId};

const IMPORT_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const IMAGE_ID: WidgetId = WidgetId::from_raw(2);

/// タイトル。
pub const TITLE: &str = "機器からの取り込み";
/// ボタンの文言。
pub const IMPORT_LABEL: &str = "接続した機器から 1 枚取り込む";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "USB 等で接続した機器から最新の画像を 1 枚取り込んで表示します（macOS・iOS・Android はカメラ機器、Windows は WIA のスキャナー。Web では使えません。機器は先頭の 1 台だけを使います）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "機器を探して取り込んでいます…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "取り込めませんでした: ";
/// 非対応のときの案内（接頭辞なし）。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 機器が見つからないときの案内。
pub const NO_DEVICE_MESSAGE: &str = "接続した機器が見つかりませんでした";
/// 画像が無いときの案内。
pub const NO_IMAGE_MESSAGE: &str = "機器に画像がありませんでした";
/// 機器がロック中のときの案内。
pub const DEVICE_LOCKED_MESSAGE: &str = "機器がロックされています（ロックを解除してください）";
/// 権限が拒否されたときの案内。
pub const PERMISSION_DENIED_MESSAGE: &str = "機器の使用が許可されていません";
/// 期限切れのときの案内。
pub const TIMED_OUT_MESSAGE: &str = "時間内に取り込めませんでした";
/// 途中で中断されたときの案内。
pub const INTERRUPTED_MESSAGE: &str = "取り込みの途中で中断されました";
/// 画像が上限を超えたときの案内。
pub const TOO_LARGE_MESSAGE: &str = "画像が大きすぎます";
/// 画像を復号できないときの案内。
pub const UNDECODABLE_MESSAGE: &str = "画像を表示できませんでした";
/// それ以外の失敗の案内。
pub const FAILED_MESSAGE: &str = "原因不明のエラーが起きました";
/// 機器の種類のラベル（カメラ機器）。
pub const CAMERA_LABEL: &str = "カメラ機器";
/// 機器の種類のラベル（スキャナー）。
pub const SCANNER_LABEL: &str = "スキャナー";

/// 取り込み元の機器の種類（`fandhe_device::scanner_devices::DeviceKind` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreKind {
    /// カメラ機器。
    Camera,
    /// スキャナー。
    Scanner,
}

/// 「{種類}から取り込みました（見つかった機器 {n} 台、{w}×{h}）」の文言。
fn imported_status(image: &ShownImage) -> String {
    let kind = match image.kind {
        CoreKind::Camera => CAMERA_LABEL,
        CoreKind::Scanner => SCANNER_LABEL,
    };
    format!(
        "{kind}から取り込みました（見つかった機器 {} 台、{}×{}）",
        image.found, image.width, image.height
    )
}

/// 表示中の画像の情報（画素は `image_store` にあり、ここには持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownImage {
    /// 画像ウィジェットが読むソース識別子（`scanner:{世代}`）。
    pub source: String,
    /// 取り込み元の機器の種類。
    pub kind: CoreKind,
    /// 見つかった機器の台数（1 以上）。
    pub found: u32,
    /// 元の幅（画素）。
    pub width: u32,
    /// 元の高さ（画素）。
    pub height: u32,
}

/// 取り込めなかった理由（`fandhe_device::scanner_devices::ScannerImportFailure` から写す責務は
/// effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// 非対応。
    Unsupported,
    /// 機器が無い。
    NoDevice,
    /// 画像が無い。
    NoImage,
    /// 機器がロック中。
    DeviceLocked,
    /// 権限の拒否・制限。
    PermissionDenied,
    /// 期限切れ。
    TimedOut,
    /// 途中の中断。
    Interrupted,
    /// 画像が上限超過。
    TooLarge,
    /// 画像を復号できない。
    Undecodable,
    /// それ以外。
    Failed,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作、または完了後。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 機器を探して取り込んでいる。
    Running,
}

/// 機器取り込み画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScannerDevicesImportState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。未完了なら `None`。
    pub result: Option<Result<ShownImage, CoreFailure>>,
}

fn is_busy(state: &ScannerDevicesImportState) -> bool {
    state.phase != Phase::Idle
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_import(state: &mut ScannerDevicesImportState) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.result = None;
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_import`] へ
/// そのまま渡す。
pub fn take_request(state: &ScannerDevicesImportState) -> Option<u64> {
    (state.phase == Phase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut ScannerDevicesImportState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み・未要求）は無視する。反映したかどうかを返す。
pub fn finish_import(
    state: &mut ScannerDevicesImportState,
    generation: u64,
    result: Result<ShownImage, CoreFailure>,
) -> bool {
    if state.phase != Phase::Running || state.generation != generation {
        return false;
    }
    state.phase = Phase::Idle;
    state.result = Some(result);
    true
}

/// 画像ウィジェットが読むソース識別子。成功時のみ。それ以外は空文字列。
pub fn image_source(state: &ScannerDevicesImportState) -> String {
    match &state.result {
        Some(Ok(i)) => i.source.clone(),
        _ => String::new(),
    }
}

/// 状態の案内文言（処理中、取り込めた）。
pub fn status_text(state: &ScannerDevicesImportState) -> String {
    if is_busy(state) {
        return RUNNING_STATUS.to_string();
    }
    match &state.result {
        Some(Ok(i)) => imported_status(i),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。画像・機器名・OS の文言は含めない。
pub fn error_text(state: &ScannerDevicesImportState) -> String {
    let Some(Err(f)) = state.result else {
        return String::new();
    };
    let message = match f {
        CoreFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        CoreFailure::NoDevice => NO_DEVICE_MESSAGE,
        CoreFailure::NoImage => NO_IMAGE_MESSAGE,
        CoreFailure::DeviceLocked => DEVICE_LOCKED_MESSAGE,
        CoreFailure::PermissionDenied => PERMISSION_DENIED_MESSAGE,
        CoreFailure::TimedOut => TIMED_OUT_MESSAGE,
        CoreFailure::Interrupted => INTERRUPTED_MESSAGE,
        CoreFailure::TooLarge => TOO_LARGE_MESSAGE,
        CoreFailure::Undecodable => UNDECODABLE_MESSAGE,
        CoreFailure::Failed => FAILED_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 画面を組み立てる。画像ノードは常に置き、ボタンは処理中以外のときだけ出す
/// （再押下を受け付けない。取り消しボタンは置かない）。
pub fn current_screen(state: &ScannerDevicesImportState) -> Screen<ScannerDevicesImportState> {
    let screen = Screen::new(TITLE)
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .with_node(Node::Widget(Box::new(ImageWidget::bound(
            IMAGE_ID,
            image_source,
            "取り込んだ画像",
        ))));
    if is_busy(state) {
        return screen;
    }
    screen.button(IMPORT_BUTTON_ID, IMPORT_LABEL, request_import)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> ScannerDevicesImportState {
        let mut s = ScannerDevicesImportState::default();
        request_import(&mut s);
        mark_running(&mut s);
        s
    }

    fn image() -> ShownImage {
        ShownImage {
            source: "scanner:1".into(),
            kind: CoreKind::Camera,
            found: 2,
            width: 640,
            height: 480,
        }
    }

    const ALL: [CoreFailure; 10] = [
        CoreFailure::Unsupported,
        CoreFailure::NoDevice,
        CoreFailure::NoImage,
        CoreFailure::DeviceLocked,
        CoreFailure::PermissionDenied,
        CoreFailure::TimedOut,
        CoreFailure::Interrupted,
        CoreFailure::TooLarge,
        CoreFailure::Undecodable,
        CoreFailure::Failed,
    ];

    #[test]
    fn transitions_idle_requested_running_idle() {
        let mut s = ScannerDevicesImportState::default();
        assert_eq!(take_request(&s), None);
        request_import(&mut s);
        assert_eq!((s.phase, take_request(&s)), (Phase::Requested, Some(1)));
        mark_running(&mut s);
        assert_eq!(s.phase, Phase::Running);
        assert_eq!(take_request(&s), None);
        assert!(finish_import(&mut s, 1, Ok(image())));
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn request_while_busy_is_ignored() {
        let mut s = running();
        request_import(&mut s);
        assert_eq!((s.phase, s.generation), (Phase::Running, 1));
    }

    #[test]
    fn stale_or_non_running_finish_is_ignored() {
        let mut s = running();
        assert!(!finish_import(&mut s, 99, Ok(image())));
        assert_eq!((s.phase, s.result.clone()), (Phase::Running, None));
        let mut idle = ScannerDevicesImportState::default();
        assert!(!finish_import(&mut idle, 0, Ok(image())));
        assert!(idle.result.is_none());
        // 確定済みの後に届いた古い完了は上書きしない。
        let mut s = running();
        finish_import(&mut s, 1, Err(CoreFailure::TimedOut));
        assert!(!finish_import(&mut s, 1, Ok(image())));
        assert_eq!(s.result, Some(Err(CoreFailure::TimedOut)));
    }

    #[test]
    fn new_request_clears_previous_result() {
        let mut s = running();
        finish_import(&mut s, 1, Ok(image()));
        request_import(&mut s);
        assert!(s.result.is_none());
        assert_eq!(s.generation, 2);
    }

    #[test]
    fn failure_shows_error_only() {
        for f in ALL {
            let mut s = running();
            finish_import(&mut s, 1, Err(f));
            assert!(!error_text(&s).is_empty(), "{f:?}");
            assert_eq!(status_text(&s), "");
            assert_eq!(image_source(&s), "");
        }
        let mut s = running();
        finish_import(&mut s, 1, Err(CoreFailure::NoDevice));
        assert!(error_text(&s).starts_with(FAILED_PREFIX));
        let mut s = running();
        finish_import(&mut s, 1, Err(CoreFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
    }

    #[test]
    fn success_shows_status_and_source() {
        let mut s = running();
        finish_import(&mut s, 1, Ok(image()));
        assert_eq!(
            status_text(&s),
            "カメラ機器から取り込みました（見つかった機器 2 台、640×480）"
        );
        assert_eq!(image_source(&s), "scanner:1");
        assert_eq!(error_text(&s), "");
        let mut s = running();
        let scanner = ShownImage {
            kind: CoreKind::Scanner,
            ..image()
        };
        finish_import(&mut s, 1, Ok(scanner));
        assert!(status_text(&s).starts_with(SCANNER_LABEL));
    }

    #[test]
    fn running_shows_status_and_hides_button() {
        let s = running();
        assert_eq!(status_text(&s), RUNNING_STATUS);
        // 固定 4 ノード（案内・状態・失敗・画像）+ ボタン 1 つ
        let count = |s: &ScannerDevicesImportState| current_screen(s).len() - 4;
        assert_eq!(count(&s), 0);
        let mut s = s;
        finish_import(&mut s, 1, Err(CoreFailure::Interrupted));
        assert_eq!(count(&s), 1);
        assert_eq!(count(&ScannerDevicesImportState::default()), 1);
    }
}
