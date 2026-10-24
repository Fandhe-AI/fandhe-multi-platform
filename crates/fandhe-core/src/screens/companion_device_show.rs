//! Phase 11（端末機能の拡充ループ、親 #143）型サンプル「付属機器のペアリング」（Issue #740）。
//! OS の機器選択画面で BLE 機器を 1 台選び、その名前（または名前がなかったこと）を表示する 1 画面。
//! `FANDHE_PROXY_APP=companion_device_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod companion_device_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/companion_device.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_pick`] で状態を進める。全対象 OS の分の状態・文言は
//! ここで定義するため、OS 別の子 issue（#741）はこのファイルを触らない。
//!
//! キャンセルは失敗ではなく状態として出す（契約表 10）。機器名と OS のエラー文はエラー文に
//! 含めず、`Debug` にも出さない（契約表 8）。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_pick（Running かつ世代一致のみ）--> Idle（result に Ok(結果) か Err(失敗)）
//! ```

use std::fmt;

use crate::{Screen, WidgetId};

const PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// タイトル。
pub const TITLE: &str = "付属機器のペアリング";
/// ボタンの文言。
pub const PICK_LABEL: &str = "機器を選ぶ";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "OS の機器選択画面で Bluetooth 機器を 1 台選び、名前を表示します（対応は Android 13 以上。選んだ後に関連付けは外します。iOS・macOS・Windows では使えません）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "機器選択画面を開いています…";
/// 名前なしで選んだときの文言。
pub const SELECTED_NAMELESS_STATUS: &str = "名前のない機器を選びました";
/// キャンセルしたときの文言（エラーではなく状態として出す）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "選べませんでした: ";
/// 非対応のときの案内（接頭辞なし）。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 位置情報サービスがオフのときの案内。
pub const LOCATION_OFF_MESSAGE: &str = "位置情報サービスがオフです";
/// Bluetooth がオフのときの案内。
pub const BLUETOOTH_OFF_MESSAGE: &str = "Bluetooth がオフです";
/// 機器が見つからないときの案内。
pub const NOT_FOUND_MESSAGE: &str = "近くに機器が見つかりませんでした";
/// 前面でないときの案内。
pub const NOT_FOREGROUND_MESSAGE: &str = "アプリが前面にありませんでした";
/// 期限切れのときの案内。
pub const TIMED_OUT_MESSAGE: &str = "時間内に開けませんでした";
/// 表示前に中断されたときの案内。
pub const INTERRUPTED_MESSAGE: &str = "表示の前に中断されました";
/// それ以外の失敗の案内。
pub const INTERNAL_MESSAGE: &str = "原因不明のエラーが起きました";

/// 「{name} を選びました」の文言。
fn selected_status(name: &str) -> String {
    format!("{name} を選びました")
}

/// 選べなかった理由（`fandhe_device::companion_device::CompanionDeviceFailure` から写す責務は
/// effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// 非対応。
    Unsupported,
    /// 位置情報サービスがオフ。
    LocationOff,
    /// Bluetooth がオフ。
    BluetoothOff,
    /// 機器が見つからない。
    NotFound,
    /// 前面でない。
    NotForeground,
    /// 選択画面を出すまでの期限切れ。
    TimedOut,
    /// 表示前の中断。
    Interrupted,
    /// それ以外。
    Internal,
}

/// 成功側の結果。`Debug` は機器名を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 機器を選んだ（名前がなければ `None`）。
    Selected(Option<String>),
    /// 利用者が閉じた。
    Cancelled,
}

impl fmt::Debug for CoreOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreOutcome::Selected(_) => f.write_str("Selected(<redacted>)"),
            CoreOutcome::Cancelled => f.write_str("Cancelled"),
        }
    }
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作、または完了後。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 選択画面を開いている（利用者が選ぶか閉じるまで）。
    Running,
}

/// 付属機器画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompanionDeviceShowState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。未完了なら `None`。
    pub result: Option<Result<CoreOutcome, CoreFailure>>,
}

fn is_busy(state: &CompanionDeviceShowState) -> bool {
    state.phase != Phase::Idle
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_pick(state: &mut CompanionDeviceShowState) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.result = None;
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_pick`] へ
/// そのまま渡す。
pub fn take_request(state: &CompanionDeviceShowState) -> Option<u64> {
    (state.phase == Phase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut CompanionDeviceShowState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み・未要求）は無視する。反映したかどうかを返す。
pub fn finish_pick(
    state: &mut CompanionDeviceShowState,
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

/// 状態の案内文言（処理中、選んだ、キャンセル）。
pub fn status_text(state: &CompanionDeviceShowState) -> String {
    if is_busy(state) {
        return RUNNING_STATUS.to_string();
    }
    match &state.result {
        Some(Ok(CoreOutcome::Selected(Some(name)))) => selected_status(name),
        Some(Ok(CoreOutcome::Selected(None))) => SELECTED_NAMELESS_STATUS.to_string(),
        Some(Ok(CoreOutcome::Cancelled)) => CANCELLED_STATUS.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。機器名は含めない。
pub fn error_text(state: &CompanionDeviceShowState) -> String {
    let Some(Err(f)) = state.result else {
        return String::new();
    };
    let message = match f {
        CoreFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        CoreFailure::LocationOff => LOCATION_OFF_MESSAGE,
        CoreFailure::BluetoothOff => BLUETOOTH_OFF_MESSAGE,
        CoreFailure::NotFound => NOT_FOUND_MESSAGE,
        CoreFailure::NotForeground => NOT_FOREGROUND_MESSAGE,
        CoreFailure::TimedOut => TIMED_OUT_MESSAGE,
        CoreFailure::Interrupted => INTERRUPTED_MESSAGE,
        CoreFailure::Internal => INTERNAL_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す（取り消しボタンは置かない）。
pub fn current_screen(state: &CompanionDeviceShowState) -> Screen<CompanionDeviceShowState> {
    let screen = Screen::new(TITLE)
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if is_busy(state) {
        return screen;
    }
    screen.button(PICK_BUTTON_ID, PICK_LABEL, request_pick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> CompanionDeviceShowState {
        let mut s = CompanionDeviceShowState::default();
        request_pick(&mut s);
        mark_running(&mut s);
        s
    }

    const ALL: [CoreFailure; 8] = [
        CoreFailure::Unsupported,
        CoreFailure::LocationOff,
        CoreFailure::BluetoothOff,
        CoreFailure::NotFound,
        CoreFailure::NotForeground,
        CoreFailure::TimedOut,
        CoreFailure::Interrupted,
        CoreFailure::Internal,
    ];

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = CompanionDeviceShowState::default();
        request_pick(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_pick(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
        request_pick(&mut s);
        assert_eq!(s.generation, 1);
        assert!(finish_pick(&mut s, 1, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Idle);
        request_pick(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(s.result, None);
        assert_eq!(status_text(&CompanionDeviceShowState::default()), "");
    }

    #[test]
    fn stale_and_idle_finish_are_ignored() {
        let mut s = running();
        assert!(!finish_pick(&mut s, 0, Ok(CoreOutcome::Cancelled)));
        assert_eq!(s.phase, Phase::Running);
        assert!(finish_pick(&mut s, 1, Err(CoreFailure::TimedOut)));
        // 期限で確定済みの後に届いた成功は適用しない
        assert!(!finish_pick(
            &mut s,
            1,
            Ok(CoreOutcome::Selected(Some("x".into())))
        ));
        assert_eq!(s.result, Some(Err(CoreFailure::TimedOut)));
        let mut idle = CompanionDeviceShowState::default();
        assert!(!finish_pick(&mut idle, 0, Ok(CoreOutcome::Cancelled)));
        assert_eq!(idle.result, None);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = CompanionDeviceShowState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn every_failure_shows_error_and_no_status() {
        for f in ALL {
            let mut s = running();
            finish_pick(&mut s, 1, Err(f));
            let t = error_text(&s);
            assert!(!t.is_empty(), "{f:?}");
            assert_eq!(status_text(&s), "", "{f:?}");
            if f != CoreFailure::Unsupported {
                assert!(t.starts_with(FAILED_PREFIX), "{t}");
            }
        }
    }

    #[test]
    fn cancelled_is_a_state_not_an_error() {
        let mut s = running();
        finish_pick(&mut s, 1, Ok(CoreOutcome::Cancelled));
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), CANCELLED_STATUS);
    }

    #[test]
    fn selected_shows_name_or_nameless_text() {
        let mut s = running();
        finish_pick(&mut s, 1, Ok(CoreOutcome::Selected(Some("Band".into()))));
        assert_eq!(status_text(&s), "Band を選びました");
        assert_eq!(error_text(&s), "");
        let mut n = running();
        finish_pick(&mut n, 1, Ok(CoreOutcome::Selected(None)));
        assert_eq!(status_text(&n), SELECTED_NAMELESS_STATUS);
    }

    #[test]
    fn unsupported_is_not_shown_as_success() {
        let mut s = running();
        finish_pick(&mut s, 1, Err(CoreFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
        assert_eq!(status_text(&s), "");
    }

    #[test]
    fn debug_hides_name() {
        let mut s = running();
        finish_pick(
            &mut s,
            1,
            Ok(CoreOutcome::Selected(Some("SecretBand".into()))),
        );
        let d = format!("{s:?}");
        assert!(!d.contains("Secret"), "{d}");
    }

    #[test]
    fn button_hidden_while_busy() {
        // 固定 3 ノード（案内・状態・失敗）+ ボタン 1 つ
        let count = |s: &CompanionDeviceShowState| current_screen(s).len() - 3;
        let mut s = CompanionDeviceShowState::default();
        assert_eq!(count(&s), 1);
        request_pick(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_pick(&mut s, 1, Err(CoreFailure::Interrupted));
        assert_eq!(count(&s), 1);
    }
}
