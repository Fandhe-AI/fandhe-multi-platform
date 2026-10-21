//! Phase 11（端末機能の拡充ループ、親 #114）型サンプル「物・サービスの決済」（Issue #541）。
//! 「支払いに使えるか調べる」「決済シートを出す」の 2 ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=payment_request` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod payment_request;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS 呼び出しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/payment_request.rs`）が [`take_request`] で要求を観測して行い、[`finish`] で結果を注入する。
//! Web 分の状態・文言もここで定義済みで、後続の子（Web）は画面に触らない。
//!
//! # 決済の成立と言わない理由
//!
//! テスト環境の承認で、課金は発生せずトークンも破棄する。「決済が完了」と表示すると誤解させるため、
//! 固定の注意書きを常に出し、結果にも成立を示す語を使わない。キャンセルは失敗にせず通常色の案内にする。
//! 世代番号（`generation`）で古い完了や `Idle` で届いた完了を無視する（`passkey_signin` と同じ判断）。

use crate::{Screen, WidgetId};

/// 常に表示する注意書き。
pub const TEST_NOTICE: &str = "テスト環境・課金は発生しません";
/// 支払いに使えるときの案内。
pub const AVAILABLE_STATUS: &str = "この環境では Google Pay（テスト環境）で支払いに使えます";
/// テスト環境で承認されたときの案内（成立とは言わない）。
pub const TEST_AUTHORIZED_STATUS: &str =
    "テスト環境で承認されました（課金は発生していません。トークンは破棄しました）";
/// キャンセル・中断の案内（通常色）。
pub const CANCELLED_STATUS: &str = "キャンセルされたか、中断されました";
/// 非対応環境の案内。ネイティブと Web で共有する。
pub const UNSUPPORTED_STATUS: &str = "この環境では決済シートを使えません（Web は Chrome の Google Pay（テスト環境）のみ。iOS・macOS・Android は開発者登録・GMS の都合で保留中）";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 「支払いに使えるか調べる」ボタンの `WidgetId`。
pub const CHECK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「決済シートを出す」ボタンの `WidgetId`。
pub const PAY_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaymentPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_running`] 済み）。
    Running,
}

/// 要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaymentAction {
    /// 支払いに使えるかの確認。
    #[default]
    Check,
    /// 決済シートの表示。
    Pay,
}

/// OS が判定した結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 支払いに使える。
    Available,
    /// テスト環境で承認された（決済の成立ではない）。
    TestAuthorized,
    /// キャンセル・中断。
    Cancelled,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestView {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 要求の種類。
    pub action: PaymentAction,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaymentRequestState {
    /// 進行状態。
    pub phase: PaymentPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 直近の要求の種類。
    pub action: PaymentAction,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

fn request(state: &mut PaymentRequestState, action: PaymentAction) {
    if state.phase != PaymentPhase::Idle {
        return;
    }
    state.phase = PaymentPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.action = action;
    state.status = String::new();
    state.error = String::new();
}

/// 「支払いに使えるか調べる」のハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn check(state: &mut PaymentRequestState) {
    request(state, PaymentAction::Check);
}

/// 「決済シートを出す」のハンドラ。`Idle` 以外は no-op。
pub fn pay(state: &mut PaymentRequestState) {
    request(state, PaymentAction::Pay);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &PaymentRequestState) -> Option<RequestView> {
    (state.phase == PaymentPhase::Requested).then_some(RequestView {
        generation: state.generation,
        action: state.action,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut PaymentRequestState) {
    if state.phase == PaymentPhase::Requested {
        state.phase = PaymentPhase::Running;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish(
    state: &mut PaymentRequestState,
    generation: u64,
    result: Result<CoreOutcome, String>,
) {
    if state.phase == PaymentPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PaymentPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::Available) => state.status = AVAILABLE_STATUS.to_string(),
        Ok(CoreOutcome::TestAuthorized) => state.status = TEST_AUTHORIZED_STATUS.to_string(),
        Ok(CoreOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &PaymentRequestState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &PaymentRequestState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。注意書きは常に出し、処理中はボタンを外して同時に 2 件出さない。
pub fn current_screen(state: &PaymentRequestState) -> Screen<PaymentRequestState> {
    let screen = Screen::new("決済")
        .text(TEST_NOTICE)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != PaymentPhase::Idle {
        return screen;
    }
    screen
        .button(CHECK_BUTTON_ID, "支払いに使えるか調べる", check)
        .button(PAY_BUTTON_ID, "決済シートを出す", pay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(f: fn(&mut PaymentRequestState)) -> PaymentRequestState {
        let mut s = PaymentRequestState::default();
        f(&mut s);
        mark_running(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, String>) -> PaymentRequestState {
        let mut s = running(pay);
        let g = s.generation;
        finish(&mut s, g, r);
        assert_eq!(s.phase, PaymentPhase::Idle);
        s
    }

    #[test]
    fn generation_advances_and_old_display_is_cleared() {
        let mut s = PaymentRequestState {
            status: "x".into(),
            error: "y".into(),
            ..Default::default()
        };
        check(&mut s);
        assert_eq!(s.generation, 1);
        assert!(s.status.is_empty() && s.error.is_empty());
        let v = take_request(&s).unwrap();
        assert_eq!((v.generation, v.action), (1, PaymentAction::Check));
    }

    #[test]
    fn busy_press_is_noop() {
        let mut s = PaymentRequestState::default();
        assert!(take_request(&s).is_none());
        check(&mut s);
        pay(&mut s);
        assert_eq!((s.generation, s.action), (1, PaymentAction::Check));
        mark_running(&mut s);
        assert!(take_request(&s).is_none());
        pay(&mut s);
        assert_eq!((s.phase, s.generation), (PaymentPhase::Running, 1));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = running(check);
        let g = s.generation + 1;
        finish(&mut s, g, Ok(CoreOutcome::TestAuthorized));
        assert_eq!(s.phase, PaymentPhase::Running);
        assert!(s.status.is_empty());
        let mut idle = PaymentRequestState::default();
        finish(&mut idle, 0, Ok(CoreOutcome::TestAuthorized));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn results_are_distinct_and_never_claim_payment() {
        let all = [
            shown(Ok(CoreOutcome::Available)),
            shown(Ok(CoreOutcome::TestAuthorized)),
            shown(Ok(CoreOutcome::Cancelled)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err("x".into())),
        ];
        let texts: Vec<String> = all
            .iter()
            .map(|s| format!("{}|{}", s.status, s.error))
            .collect();
        for (i, a) in texts.iter().enumerate() {
            assert!(!a.contains("決済が完了") && !a.contains("支払いました"));
            for b in &texts[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(all[2].error.is_empty() && !all[2].status.is_empty());
        assert!(all[4].status.is_empty() && all[4].error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn notice_always_shown_and_buttons_hidden_while_busy() {
        let idle = format!("{:?}", current_screen(&PaymentRequestState::default()));
        let busy = format!("{:?}", current_screen(&running(pay)));
        assert!(idle.contains(TEST_NOTICE) && busy.contains(TEST_NOTICE));
        assert!(idle.contains("決済シートを出す") && idle.contains("支払いに使えるか調べる"));
        assert!(!busy.contains("決済シートを出す") && !busy.contains("支払いに使えるか調べる"));
    }
}
