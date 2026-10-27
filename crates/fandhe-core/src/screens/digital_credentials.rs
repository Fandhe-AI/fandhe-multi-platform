//! Phase 11（端末機能の拡充ループ、親 #186）型サンプル「デジタル身分証の提示要求」（Issue #958）。
//! 「身分証の提示を求める（18 歳以上か）」ボタン 1 つだけの 1 画面。
//! `FANDHE_PROXY_APP=digital_credentials` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod digital_credentials;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS 呼び出しは `fandhe-egui-proxy` の effect
//! ドライバが [`take_request`] で要求を観測して行い、[`finish`] で結果を注入する。
//! Web 分の状態・文言もここで定義済みで、後続の子（Web）は画面に触らない。
//!
//! # 本人確認と言わない理由
//!
//! 応答の中身は読まず検証もしない。「確認できた」と表示すると誤解させるため、固定の注意書きを常に出し、
//! 結果にも確認を示す語を使わない。共有されなかった・取り消しは失敗にせず通常色の案内にする。
//! 世代番号（`generation`）で古い完了や `Idle` で届いた完了を無視する。

use crate::{Screen, WidgetId};

/// 常に表示する注意書き。
pub const DEMO_NOTICE: &str =
    "検証なしのデモ: 応答の中身は読まず、本人確認・年齢確認にはなりません";
/// ウォレットが応答を返したときの案内。
pub const RESPONDED_STATUS: &str = "ウォレットが応答を返しました（内容は検証も表示もしていません）";
/// 共有されなかったときの案内（通常色）。
pub const NOT_SHARED_STATUS: &str =
    "共有されませんでした（取り消し、または該当する身分証がありません）";
/// 画面を離れて要求を取り消したときの案内（通常色）。
pub const CANCELLED_STATUS: &str = "画面を離れたため、要求を取り消しました";
/// 非対応環境の案内。ネイティブと Web で共有する。
pub const UNSUPPORTED_STATUS: &str = "この環境では身分証の提示要求を使えません（Web は Chrome 系のみ。iOS は Apple の許可待ち、Android は Google Play services 依存のため保留中。macOS・Windows・Linux は対象外）";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 「身分証の提示を求める」ボタンの `WidgetId`。
pub const REQUEST_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_running`] 済み）。
    Running,
}

/// OS が判定した結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOutcome {
    /// ウォレットが応答を返した（内容は未検証）。
    Responded,
    /// 共有されなかった。
    NotShared,
    /// 取り消し。
    Cancelled,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestView {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DigitalCredentialsState {
    /// 進行状態。
    pub phase: CredentialPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

/// ボタンのハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn request(state: &mut DigitalCredentialsState) {
    if state.phase != CredentialPhase::Idle {
        return;
    }
    state.phase = CredentialPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &DigitalCredentialsState) -> Option<RequestView> {
    (state.phase == CredentialPhase::Requested).then_some(RequestView {
        generation: state.generation,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut DigitalCredentialsState) {
    if state.phase == CredentialPhase::Requested {
        state.phase = CredentialPhase::Running;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish(
    state: &mut DigitalCredentialsState,
    generation: u64,
    result: Result<CoreOutcome, String>,
) {
    if state.phase == CredentialPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = CredentialPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(CoreOutcome::Responded) => state.status = RESPONDED_STATUS.to_string(),
        Ok(CoreOutcome::NotShared) => state.status = NOT_SHARED_STATUS.to_string(),
        Ok(CoreOutcome::Cancelled) => state.status = CANCELLED_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &DigitalCredentialsState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &DigitalCredentialsState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。注意書きは常に出し、処理中はボタンを外して同時に 2 件出さない。
pub fn current_screen(state: &DigitalCredentialsState) -> Screen<DigitalCredentialsState> {
    let screen = Screen::new("デジタル身分証")
        .text(DEMO_NOTICE)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != CredentialPhase::Idle {
        return screen;
    }
    screen.button(
        REQUEST_BUTTON_ID,
        "身分証の提示を求める（18 歳以上か）",
        request,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> DigitalCredentialsState {
        let mut s = DigitalCredentialsState::default();
        request(&mut s);
        mark_running(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, String>) -> DigitalCredentialsState {
        let mut s = running();
        let g = s.generation;
        finish(&mut s, g, r);
        assert_eq!(s.phase, CredentialPhase::Idle);
        s
    }

    #[test]
    fn generation_advances_and_old_display_is_cleared() {
        let mut s = DigitalCredentialsState {
            status: "x".into(),
            error: "y".into(),
            ..Default::default()
        };
        request(&mut s);
        assert_eq!(s.generation, 1);
        assert!(s.status.is_empty() && s.error.is_empty());
        assert_eq!(take_request(&s).unwrap().generation, 1);
    }

    #[test]
    fn busy_press_is_noop() {
        let mut s = running();
        request(&mut s);
        assert_eq!((s.phase, s.generation), (CredentialPhase::Running, 1));
        assert!(take_request(&s).is_none());
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = running();
        let g = s.generation + 1;
        finish(&mut s, g, Ok(CoreOutcome::Responded));
        assert_eq!(s.phase, CredentialPhase::Running);
        assert!(s.status.is_empty());
        let mut idle = DigitalCredentialsState::default();
        finish(&mut idle, 0, Ok(CoreOutcome::Responded));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn results_are_distinct_and_never_claim_verification() {
        let all = [
            shown(Ok(CoreOutcome::Responded)),
            shown(Ok(CoreOutcome::NotShared)),
            shown(Ok(CoreOutcome::Cancelled)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err("x".into())),
        ];
        let texts: Vec<String> = all
            .iter()
            .map(|s| format!("{}|{}", s.status, s.error))
            .collect();
        for (i, a) in texts.iter().enumerate() {
            assert!(!a.contains("本人確認できました") && !a.contains("年齢を確認しました"));
            for b in &texts[i + 1..] {
                assert_ne!(a, b);
            }
        }
        for s in &all[1..3] {
            assert!(s.error.is_empty() && !s.status.is_empty());
        }
        assert!(all[4].status.is_empty() && all[4].error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn notice_always_shown_and_button_hidden_while_busy() {
        let idle = format!("{:?}", current_screen(&DigitalCredentialsState::default()));
        let busy = format!("{:?}", current_screen(&running()));
        assert!(idle.contains(DEMO_NOTICE) && busy.contains(DEMO_NOTICE));
        assert!(idle.contains("身分証の提示を求める"));
        assert!(!busy.contains("身分証の提示を求める"));
    }
}
