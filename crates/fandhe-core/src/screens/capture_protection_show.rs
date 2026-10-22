//! Phase 11（端末機能の拡充ループ、親 #120、Issue #582）型サンプル「画面キャプチャの防止・検知」。
//! ボタンで撮影・録画に写さない防止を掛ける / 外し、**実際に保護されているか**と、録画・
//! ミラーリング中か、スクリーンショットの回数を表示する 1 画面。
//! `FANDHE_PROXY_APP=capture_protection_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod capture_protection_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない。時刻も持たず、読み取り間隔は effect 側が測る）。
//! OS 呼び出しは本モジュール外（`fandhe-egui-proxy` の `device/capture_protection.rs`）が
//! [`take_request`] で状態を観測し、[`finish`] / [`apply_read`] で結果を注入する
//! （`wake_lock_toggle.rs` と同型）。全 OS 分の状態・文言（「検知のみ」「防止のみ」「非対応」を含む）は
//! 本モジュールで定義済みで、後続の iOS・Android は OS 別モジュールだけを埋める。
//!
//! # 表示は読み戻した値で決める
//!
//! 「保護中」と出すのは OS から読み戻した [`CaptureProtectionShowState::protected`] が
//! `Some(true)` のときだけ（`device-bridge-contracts.md`「6. 嘘の成功」）。要求値では出さない。
//! 録画・スクショ回数の `None` は「読めない」で、「録画されていない」「0 回」とは出さない。
//!
//! ```text
//! Idle --「撮影を防ぐ/防止をやめる」--> Requested, generation += 1
//!   --effect が take_request で観測 → mark_applying--> Applying
//!   --set 成功・失敗とも直後に読み戻し → finish（世代一致のみ）--> Idle + 結果表示
//! Idle 中は apply_read（定期 pull）で protected・録画・回数を更新する。世代・phase は動かさない。
//! ```

use crate::{Screen, WidgetId};

/// 「撮影を防ぐ」ボタンの `WidgetId`。
pub const ON_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「防止をやめる」ボタンの `WidgetId`。
pub const OFF_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 画面タイトル。
pub const TITLE: &str = "画面キャプチャの防止・検知";
/// 防止を掛けるボタンの文言。
pub const ON_LABEL: &str = "撮影を防ぐ";
/// 防止を外すボタンの文言。
pub const OFF_LABEL: &str = "防止をやめる";
/// 保護を読み戻せたときの案内。
pub const PROTECTED_STATUS: &str = "保護中です（このウィンドウは画面の撮影・録画に写りません）";
/// 未保護を読み戻せたときの案内。
pub const UNPROTECTED_STATUS: &str = "保護していません";
/// 保護の状態を読めないときの案内。
pub const PROTECTION_UNKNOWN_STATUS: &str = "保護の状態を読み取れません";
/// 防止が非対応のときの案内。
pub const PREVENT_UNSUPPORTED_STATUS: &str = "この OS では撮影を防げません";
/// 録画・ミラーリング中の案内。
pub const RECORDING_ON: &str = "録画・ミラーリング: 中です";
/// 録画・ミラーリングされていないときの案内。
pub const RECORDING_OFF: &str = "録画・ミラーリング: されていません";
/// 録画状態を読めないとき（背景にいる間など）の案内。
pub const RECORDING_UNKNOWN: &str = "録画・ミラーリング: 読み取れません";
/// 録画を検知できない OS の案内。
pub const RECORDING_UNSUPPORTED: &str = "録画・ミラーリング: この OS では検知できません";
/// スクリーンショット回数の前置き（`{前置き}{N} 回`）。撮影の後に届く通知で数える。
pub const SCREENSHOT_COUNT_PREFIX: &str =
    "スクリーンショット（撮影の後に検知。防止ではありません）: ";
/// スクショ回数を読めないときの案内。
pub const SCREENSHOT_UNKNOWN: &str = "スクリーンショット: 読み取れません";
/// スクショを検知できない OS の案内。
pub const SCREENSHOT_UNSUPPORTED: &str = "スクリーンショット: この OS では検知できません";
/// 検知のみの OS（iOS 等）向けの注記。
pub const DETECT_ONLY_NOTE: &str = "検知のみ（この OS では撮影を防げません）";
/// 防止のみの OS（Windows 等）向けの注記。
pub const PREVENT_ONLY_NOTE: &str =
    "防止のみ（この OS では録画・スクリーンショットを検知できません）";
/// 防止も検知もできない OS 向けの注記。
pub const ALL_UNSUPPORTED_NOTE: &str = "この OS では防止も検知もできません";
/// 注記。
pub const NOTE: &str = "外部カメラでの撮影は防げません。掛ける前に始まっていた録画は消せません。Windows 10 2004 より前では黒く写ります";
/// 失敗時の案内の接頭辞（`{接頭辞}{理由}`）。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// 掛けたと返ったのに保護が効いていなかったときの理由。
pub const NOT_PROTECTED_REASON: &str = "保護が効いていることを確かめられませんでした";
/// 外したと返ったのに保護が残っていたときの理由。
pub const STILL_PROTECTED_REASON: &str = "保護が残っています";
/// 解除後に保護状態を読み戻せず、解除を確認できなかったときの失敗理由。
pub const UNVERIFIED_REASON: &str = "解除を確認できません";

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapturePhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが OS へ要求を渡している最中。
    Applying,
}

/// 失敗の種別（`DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFailure {
    /// 非対応。
    Unsupported,
    /// それ以外の失敗（表示用の固定文言）。
    Failed(String),
}

/// 対応項目（`fandhe-device` の型を import しないため core 側で持つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SupportView {
    /// 防止ができるか。
    pub prevent: bool,
    /// 録画を検知できるか。
    pub recording: bool,
    /// スクショを検知できるか。
    pub screenshot: bool,
}

/// 定期 pull の読み取り結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadView {
    /// 対応項目。
    pub support: SupportView,
    /// OS から読み戻した保護状態。`None` は読めない。
    pub protected: Option<bool>,
    /// 録画・ミラーリング中か。`None` は読めない。
    pub recording: Option<bool>,
    /// スクリーンショットの回数。`None` は読めない。
    pub screenshot_count: Option<u64>,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRequest {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// `true` で防止を掛け、`false` で外す。
    pub on: bool,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaptureProtectionShowState {
    /// 操作の進行状態。
    pub phase: CapturePhase,
    /// 直近に立てた要求の世代番号。
    pub generation: u64,
    /// 処理中の要求の向き（`true` = 防止を掛ける）。
    pub requested_on: bool,
    /// 利用者が望んでいる状態。失敗時は読み戻した保護状態に揃える。
    pub desired: bool,
    /// OS から読み戻した保護状態。表示はこちらで決める。
    pub protected: Option<bool>,
    /// 対応項目。初回の読み取りまで `None`。
    pub support: Option<SupportView>,
    /// 録画・ミラーリング中か。
    pub recording: Option<bool>,
    /// スクリーンショットの回数。
    pub screenshot_count: Option<u64>,
    /// 失敗理由（`TextRole::Error` で表示）。空なら無し。
    pub error: String,
}

fn request(state: &mut CaptureProtectionShowState, on: bool) {
    if state.phase != CapturePhase::Idle {
        return;
    }
    state.phase = CapturePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.requested_on = on;
    state.desired = on;
    state.error = String::new();
}

/// 「撮影を防ぐ」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn turn_on(state: &mut CaptureProtectionShowState) {
    request(state, true);
}

/// 「防止をやめる」ボタンのハンドラ。`Idle` 以外では no-op。
pub fn turn_off(state: &mut CaptureProtectionShowState) {
    request(state, false);
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &CaptureProtectionShowState) -> Option<CaptureRequest> {
    (state.phase == CapturePhase::Requested).then_some(CaptureRequest {
        generation: state.generation,
        on: state.requested_on,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_applying(state: &mut CaptureProtectionShowState) {
    if state.phase == CapturePhase::Requested {
        state.phase = CapturePhase::Applying;
    }
}

/// 結果を反映する。`Idle` / `Requested` で届いた完了、世代が一致しない完了は捨てる。
///
/// `protected` は完了直後に読み戻した値で、常に反映する。`Ok` でも読み戻しが要求と食い違えば
/// 失敗にする（嘘の成功の禁止）。解除でも読み戻せない（`None`）場合は確認不能として失敗にする。失敗時の `desired` は読み戻した保護状態に合わせる。
pub fn finish(
    state: &mut CaptureProtectionShowState,
    generation: u64,
    result: Result<(), CaptureFailure>,
    protected: Option<bool>,
) {
    if matches!(state.phase, CapturePhase::Idle | CapturePhase::Requested)
        || state.generation != generation
    {
        return;
    }
    state.phase = CapturePhase::Idle;
    state.protected = protected;
    let result = match (result, state.requested_on) {
        (Ok(()), true) if protected != Some(true) => {
            Err(CaptureFailure::Failed(NOT_PROTECTED_REASON.to_string()))
        }
        (Ok(()), false) if protected == Some(true) => {
            Err(CaptureFailure::Failed(STILL_PROTECTED_REASON.to_string()))
        }
        (Ok(()), false) if protected.is_none() => {
            Err(CaptureFailure::Failed(UNVERIFIED_REASON.to_string()))
        }
        (other, _) => other,
    };
    match result {
        Ok(()) => state.error = String::new(),
        Err(CaptureFailure::Unsupported) => {
            state.desired = false;
            state.error = PREVENT_UNSUPPORTED_STATUS.to_string();
        }
        Err(CaptureFailure::Failed(message)) => {
            state.desired = protected == Some(true);
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 定期 pull の結果を反映する。世代・phase は動かさない。
///
/// 対応項目・録画・回数は常に更新する。保護状態は処理中の読み戻しを上書きしないよう
/// `Idle` のときだけ更新する。
pub fn apply_read(state: &mut CaptureProtectionShowState, read: ReadView) {
    state.support = Some(read.support);
    state.recording = read.recording;
    state.screenshot_count = read.screenshot_count;
    if state.phase == CapturePhase::Idle {
        state.protected = read.protected;
    }
}

/// 保護状態の案内文言。「保護中」は読み戻しが `Some(true)` のときだけ。
pub fn protection_text(state: &CaptureProtectionShowState) -> String {
    match (state.protected, state.support) {
        (Some(true), _) => PROTECTED_STATUS,
        (_, Some(s)) if !s.prevent => PREVENT_UNSUPPORTED_STATUS,
        (None, _) => PROTECTION_UNKNOWN_STATUS,
        (Some(false), _) => UNPROTECTED_STATUS,
    }
    .to_string()
}

/// 録画・ミラーリングの案内文言。
pub fn recording_text(state: &CaptureProtectionShowState) -> String {
    match (state.support, state.recording) {
        (Some(s), _) if !s.recording => RECORDING_UNSUPPORTED,
        (_, Some(true)) => RECORDING_ON,
        (_, Some(false)) => RECORDING_OFF,
        (_, None) => RECORDING_UNKNOWN,
    }
    .to_string()
}

/// スクリーンショットの案内文言。
pub fn screenshot_text(state: &CaptureProtectionShowState) -> String {
    match (state.support, state.screenshot_count) {
        (Some(s), _) if !s.screenshot => SCREENSHOT_UNSUPPORTED.to_string(),
        (_, Some(n)) => format!("{SCREENSHOT_COUNT_PREFIX}{n} 回"),
        (_, None) => SCREENSHOT_UNKNOWN.to_string(),
    }
}

/// 対応項目の注記（防止のみ / 検知のみ / 非対応）。両方できる OS と未取得のときは空。
pub fn support_note_text(state: &CaptureProtectionShowState) -> String {
    match state.support {
        Some(s) => {
            let detect = s.recording || s.screenshot;
            match (s.prevent, detect) {
                (true, false) => PREVENT_ONLY_NOTE,
                (false, true) => DETECT_ONLY_NOTE,
                (false, false) => ALL_UNSUPPORTED_NOTE,
                (true, true) => "",
            }
        }
        None => "",
    }
    .to_string()
}

/// 失敗時の案内文言（`TextRole::Error`）。
pub fn error_text(state: &CaptureProtectionShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。ボタンは `Idle` のときだけ出す。防止が非対応でも出す
/// （押すと「この OS では撮影を防げません」になる）。
pub fn current_screen(state: &CaptureProtectionShowState) -> Screen<CaptureProtectionShowState> {
    let screen = Screen::new(TITLE)
        .dynamic_text(protection_text)
        .dynamic_text(recording_text)
        .dynamic_text(screenshot_text)
        .dynamic_text(support_note_text)
        .error_text(error_text)
        .text(NOTE);
    if state.phase != CapturePhase::Idle {
        return screen;
    }
    screen
        .button(ON_BUTTON_ID, ON_LABEL, turn_on)
        .button(OFF_BUTTON_ID, OFF_LABEL, turn_off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    const PREVENT_ONLY: SupportView = SupportView {
        prevent: true,
        recording: false,
        screenshot: false,
    };
    const DETECT_ONLY: SupportView = SupportView {
        prevent: false,
        recording: true,
        screenshot: true,
    };

    fn applying(on: bool) -> CaptureProtectionShowState {
        let mut s = CaptureProtectionShowState::default();
        request(&mut s, on);
        mark_applying(&mut s);
        s
    }

    #[test]
    fn request_advances_generation_and_carries_direction() {
        let mut s = CaptureProtectionShowState::default();
        assert_eq!(take_request(&s), None);
        turn_on(&mut s);
        assert_eq!(
            take_request(&s),
            Some(CaptureRequest {
                generation: 1,
                on: true
            })
        );
        mark_applying(&mut s);
        assert_eq!(take_request(&s), None);
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = applying(true);
        turn_off(&mut s);
        assert_eq!(s.phase, CapturePhase::Applying);
        assert!(s.requested_on);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn finish_ok_shows_protected_then_unprotected() {
        let mut s = applying(true);
        finish(&mut s, 1, Ok(()), Some(true));
        assert_eq!(protection_text(&s), PROTECTED_STATUS);
        request(&mut s, false);
        mark_applying(&mut s);
        finish(&mut s, 2, Ok(()), Some(false));
        assert_eq!(protection_text(&s), UNPROTECTED_STATUS);
        assert_eq!(s.error, "");
        assert_eq!(s.phase, CapturePhase::Idle);
    }

    #[test]
    fn on_ok_without_effect_is_failure_not_protected() {
        for read in [Some(false), None] {
            let mut s = applying(true);
            finish(&mut s, 1, Ok(()), read);
            assert_eq!(s.error, format!("{FAILED_PREFIX}{NOT_PROTECTED_REASON}"));
            assert_ne!(protection_text(&s), PROTECTED_STATUS);
            assert!(!s.desired);
        }
    }

    #[test]
    fn off_ok_but_unreadable_is_failure_not_success() {
        let mut s = applying(false);
        finish(&mut s, 1, Ok(()), None);
        assert_eq!(s.error, format!("{FAILED_PREFIX}{UNVERIFIED_REASON}"));
        assert!(!s.desired);
    }

    #[test]
    fn off_ok_but_still_protected_is_failure_and_stays_protected() {
        let mut s = applying(false);
        finish(&mut s, 1, Ok(()), Some(true));
        assert_eq!(s.error, format!("{FAILED_PREFIX}{STILL_PROTECTED_REASON}"));
        assert_eq!(protection_text(&s), PROTECTED_STATUS);
        assert!(s.desired);
    }

    #[test]
    fn failed_on_stays_unprotected_and_failed_off_stays_protected() {
        let mut s = applying(true);
        finish(
            &mut s,
            1,
            Err(CaptureFailure::Failed("x".into())),
            Some(false),
        );
        assert_eq!(s.error, format!("{FAILED_PREFIX}x"));
        assert_eq!(protection_text(&s), UNPROTECTED_STATUS);
        assert!(!s.desired);

        let mut s = applying(false);
        finish(
            &mut s,
            1,
            Err(CaptureFailure::Failed("x".into())),
            Some(true),
        );
        assert_eq!(protection_text(&s), PROTECTED_STATUS);
        assert!(s.desired);
    }

    #[test]
    fn unsupported_is_not_success() {
        let mut s = applying(true);
        finish(&mut s, 1, Err(CaptureFailure::Unsupported), None);
        assert_eq!(s.error, PREVENT_UNSUPPORTED_STATUS);
        assert_ne!(protection_text(&s), PROTECTED_STATUS);
        assert!(!s.desired);
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = applying(true);
        finish(&mut s, 99, Ok(()), Some(true));
        assert_eq!(s.phase, CapturePhase::Applying);
        assert_eq!(s.protected, None);
        let mut idle = CaptureProtectionShowState::default();
        finish(&mut idle, 0, Ok(()), Some(true));
        assert_eq!(idle.protected, None);
        let mut req = CaptureProtectionShowState::default();
        turn_on(&mut req);
        finish(&mut req, 1, Ok(()), Some(true));
        assert_eq!(req.phase, CapturePhase::Requested);
    }

    #[test]
    fn apply_read_does_not_overwrite_protected_while_busy() {
        let mut s = applying(true);
        apply_read(
            &mut s,
            ReadView {
                support: PREVENT_ONLY,
                protected: Some(false),
                recording: Some(true),
                screenshot_count: Some(2),
            },
        );
        assert_eq!(s.protected, None);
        assert_eq!(s.recording, Some(true));
        assert_eq!(s.screenshot_count, Some(2));
        assert_eq!(s.generation, 1);
        assert_eq!(s.phase, CapturePhase::Applying);

        let mut idle = CaptureProtectionShowState::default();
        apply_read(
            &mut idle,
            ReadView {
                support: PREVENT_ONLY,
                protected: Some(true),
                ..ReadView::default()
            },
        );
        assert_eq!(idle.protected, Some(true));
    }

    #[test]
    fn notes_follow_support() {
        let mut s = CaptureProtectionShowState::default();
        assert_eq!(support_note_text(&s), "");
        s.support = Some(PREVENT_ONLY);
        assert_eq!(support_note_text(&s), PREVENT_ONLY_NOTE);
        s.support = Some(DETECT_ONLY);
        assert_eq!(support_note_text(&s), DETECT_ONLY_NOTE);
        s.support = Some(SupportView::default());
        assert_eq!(support_note_text(&s), ALL_UNSUPPORTED_NOTE);
        s.support = Some(SupportView {
            prevent: true,
            recording: true,
            screenshot: true,
        });
        assert_eq!(support_note_text(&s), "");
    }

    #[test]
    fn unsupported_items_show_unsupported() {
        let mut s = CaptureProtectionShowState {
            support: Some(PREVENT_ONLY),
            ..Default::default()
        };
        assert_eq!(recording_text(&s), RECORDING_UNSUPPORTED);
        assert_eq!(screenshot_text(&s), SCREENSHOT_UNSUPPORTED);
        s.support = Some(DETECT_ONLY);
        assert_eq!(protection_text(&s), PREVENT_UNSUPPORTED_STATUS);
    }

    #[test]
    fn unreadable_values_are_unknown_not_false_or_zero() {
        let mut s = CaptureProtectionShowState {
            support: Some(DETECT_ONLY),
            ..Default::default()
        };
        assert_eq!(recording_text(&s), RECORDING_UNKNOWN);
        assert_eq!(screenshot_text(&s), SCREENSHOT_UNKNOWN);
        s.recording = Some(false);
        s.screenshot_count = Some(0);
        assert_eq!(recording_text(&s), RECORDING_OFF);
        assert_eq!(
            screenshot_text(&s),
            format!("{SCREENSHOT_COUNT_PREFIX}0 回")
        );
        s.recording = Some(true);
        assert_eq!(recording_text(&s), RECORDING_ON);
        let unread = CaptureProtectionShowState {
            support: Some(PREVENT_ONLY),
            ..Default::default()
        };
        assert_eq!(protection_text(&unread), PROTECTION_UNKNOWN_STATUS);
    }

    #[test]
    fn buttons_only_when_idle() {
        let mut s = CaptureProtectionShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, ON_LABEL, &mut s));
        let screen = current_screen(&s);
        assert!(!click_button(&screen, OFF_LABEL, &mut s));
        mark_applying(&mut s);
        let screen = current_screen(&s);
        assert!(!click_button(&screen, OFF_LABEL, &mut s));
    }
}
