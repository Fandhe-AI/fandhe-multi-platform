//! Phase 11（端末機能の拡充ループ、親 #92）型サンプル「パスキー・サインイン連携」（Issue #342）。
//! 「パスキーを登録」「パスキーでサインイン」の 2 ボタンだけの 1 画面。
//! `FANDHE_PROXY_APP=passkey_signin` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod passkey_signin;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS 呼び出しと応答の整合確認は `fandhe-egui-proxy` の effect ドライバ
//! （`device/passkey.rs`）が [`take_request`] で要求を観測して行い、[`finish`] で結果を注入する。
//! Windows・Web の分の状態・文言はここですべて定義する（後続の OS 別 issue は画面に触らない）。
//!
//! # サインインの成立と言わない理由
//!
//! サーバー検証なしのデモで、署名は検証しない。「署名付きの応答が返った」を「サインインした」と
//! 表示すると、認証が成立したと誤解させる。固定の注意書きを常に出し、結果名にも成立を示す語を使わない。
//! キャンセル・該当パスキーなし（`NotAllowed`）は成功にも失敗にも見せず通常色の案内にする。
//! 世代番号（`generation`）で古い完了や `Idle` で届いた完了を無視する（`print_page` と同じ判断）。
//! 資格情報 ID は「この画面で登録したものと同じか」の比較のためにメモリ内だけで持ち、`Debug` に出さない。

use crate::{Screen, WidgetId};

/// 常に表示する注意書き。
pub const DEMO_NOTICE: &str =
    "サーバー検証なしのデモです。署名は検証していません。この結果はサインインの成立を意味しません";
/// 登録できたときの案内。
pub const REGISTERED_STATUS: &str = "パスキーを登録しました";
/// サインインの応答が返ったときの案内（成立とは言わない）。
pub const ASSERTION_STATUS: &str = "署名付きの応答が返りました（サインインの成立ではありません）";
/// キャンセル・該当パスキーなし・期限切れ・同意拒否のときの案内（通常色）。
pub const NOT_ALLOWED_STATUS: &str = "キャンセルされたか、使えるパスキーがありません";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str =
    "この環境ではパスキーを使えません（認証器なし・secure context 外・OS が古い）";
/// 応答が今回の要求と一致しないときの文言（失敗として表示する）。
pub const MISMATCH_TEXT: &str = "応答が今回の要求と一致しないため、成功として扱いません";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 後片付けの案内の接頭辞（RP ID を後ろに付ける）。
pub const CLEANUP_PREFIX: &str =
    "登録したデモ用パスキーは OS・ブラウザに残ります。要求した RP ID: ";
/// 「この画面で登録したものと同じ」の比較結果。
pub const SAME_TEXT: &str = "この画面で登録したものと同じ資格情報です";
/// 同上（不一致）。
pub const DIFFERENT_TEXT: &str = "この画面で登録したものとは別の資格情報です";
/// 同上（未登録）。
pub const NOT_REGISTERED_TEXT: &str = "この画面ではまだ登録していません";

/// 「パスキーを登録」ボタンの `WidgetId`。
pub const REGISTER_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「パスキーでサインイン」ボタンの `WidgetId`。
pub const SIGN_IN_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasskeyPhase {
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
pub enum PasskeyAction {
    /// 登録。
    #[default]
    Register,
    /// サインイン要求。
    SignIn,
}

/// 資格情報 ID のメモ。`Debug` は値を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialMemo(pub Vec<u8>);

impl std::fmt::Debug for CredentialMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialMemo(<redacted>)")
    }
}

/// 整合確認を通った応答の要約。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreSummary {
    /// 要求した RP ID（応答の rpIdHash とは照合していない）。
    pub rp_id: String,
    /// 資格情報 ID。
    pub credential_id: CredentialMemo,
    /// ユーザーの存在。
    pub user_present: bool,
    /// ユーザーの検証。
    pub user_verified: bool,
    /// 署名のバイト数（登録では `None`）。
    pub signature_len: Option<usize>,
}

/// OS が判定した結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreOutcome {
    /// 登録の応答が返った。
    Registered(CoreSummary),
    /// サインインの署名付き応答が返った（成立ではない）。
    AssertionReturned(CoreSummary),
    /// キャンセル・該当なし・期限切れ・同意拒否。
    NotAllowed,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestView {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 要求の種類。
    pub action: PasskeyAction,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PasskeySigninState {
    /// 進行状態。
    pub phase: PasskeyPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 直近の要求の種類。
    pub action: PasskeyAction,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
    /// 詳細行（RP ID・UP / UV・署名の長さ・登録との一致・後片付け）。
    pub detail: String,
    /// この画面で登録した資格情報（メモリ内だけ）。
    pub registered: Option<CredentialMemo>,
}

fn request(state: &mut PasskeySigninState, action: PasskeyAction) {
    if state.phase != PasskeyPhase::Idle {
        return;
    }
    state.phase = PasskeyPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.action = action;
    state.status = String::new();
    state.error = String::new();
    state.detail = String::new();
}

/// 「パスキーを登録」のハンドラ。`Idle` 以外は二重要求防止のため no-op。
pub fn register(state: &mut PasskeySigninState) {
    request(state, PasskeyAction::Register);
}

/// 「パスキーでサインイン」のハンドラ。`Idle` 以外は no-op。
pub fn sign_in(state: &mut PasskeySigninState) {
    request(state, PasskeyAction::SignIn);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &PasskeySigninState) -> Option<RequestView> {
    (state.phase == PasskeyPhase::Requested).then_some(RequestView {
        generation: state.generation,
        action: state.action,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut PasskeySigninState) {
    if state.phase == PasskeyPhase::Requested {
        state.phase = PasskeyPhase::Running;
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "あり" } else { "なし" }
}

fn detail_of(s: &CoreSummary, same: &str) -> String {
    let sig = s
        .signature_len
        .map(|n| format!(" / 署名: {n} バイト"))
        .unwrap_or_default();
    format!(
        "要求した RP ID: {} / 本人の存在: {} / 本人の検証: {}{sig}\n{same}\n{CLEANUP_PREFIX}{}",
        s.rp_id,
        yes_no(s.user_present),
        yes_no(s.user_verified),
        s.rp_id
    )
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish(
    state: &mut PasskeySigninState,
    generation: u64,
    result: Result<CoreOutcome, String>,
) {
    if state.phase == PasskeyPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PasskeyPhase::Idle;
    state.status = String::new();
    state.error = String::new();
    state.detail = String::new();
    match result {
        Ok(CoreOutcome::Registered(s)) => {
            state.status = REGISTERED_STATUS.to_string();
            state.detail = detail_of(&s, SAME_TEXT);
            state.registered = Some(s.credential_id);
        }
        Ok(CoreOutcome::AssertionReturned(s)) => {
            state.status = ASSERTION_STATUS.to_string();
            let same = match &state.registered {
                None => NOT_REGISTERED_TEXT,
                Some(r) if *r == s.credential_id => SAME_TEXT,
                Some(_) => DIFFERENT_TEXT,
            };
            state.detail = detail_of(&s, same);
        }
        Ok(CoreOutcome::NotAllowed) => state.status = NOT_ALLOWED_STATUS.to_string(),
        Ok(CoreOutcome::Unsupported) => state.status = UNSUPPORTED_STATUS.to_string(),
        Err(m) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &PasskeySigninState) -> String {
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &PasskeySigninState) -> String {
    state.error.clone()
}

/// 詳細文言（`DynamicText` の `get`）。
pub fn detail_text(state: &PasskeySigninState) -> String {
    state.detail.clone()
}

/// 画面を組み立てる。注意書きは常に出し、処理中はボタンを外して同時に 2 件出さない。
pub fn current_screen(state: &PasskeySigninState) -> Screen<PasskeySigninState> {
    let screen = Screen::new("パスキー")
        .text(DEMO_NOTICE)
        .dynamic_text(status_text)
        .dynamic_text(detail_text)
        .error_text(error_text);
    if state.phase != PasskeyPhase::Idle {
        return screen;
    }
    screen
        .button(REGISTER_BUTTON_ID, "パスキーを登録", register)
        .button(SIGN_IN_BUTTON_ID, "パスキーでサインイン", sign_in)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: u8) -> CoreSummary {
        CoreSummary {
            rp_id: "localhost".into(),
            credential_id: CredentialMemo(vec![id; 4]),
            user_present: true,
            user_verified: true,
            signature_len: Some(70),
        }
    }

    fn running(f: fn(&mut PasskeySigninState)) -> PasskeySigninState {
        let mut s = PasskeySigninState::default();
        f(&mut s);
        mark_running(&mut s);
        s
    }

    fn shown(r: Result<CoreOutcome, String>) -> PasskeySigninState {
        let mut s = running(sign_in);
        let g = s.generation;
        finish(&mut s, g, r);
        assert_eq!(s.phase, PasskeyPhase::Idle);
        s
    }

    #[test]
    fn generation_advances_and_old_display_is_cleared() {
        let mut s = PasskeySigninState {
            status: "x".into(),
            error: "y".into(),
            detail: "z".into(),
            ..Default::default()
        };
        register(&mut s);
        assert_eq!(s.generation, 1);
        assert!(s.status.is_empty() && s.error.is_empty() && s.detail.is_empty());
        let v = take_request(&s).unwrap();
        assert_eq!((v.generation, v.action), (1, PasskeyAction::Register));
    }

    #[test]
    fn busy_press_is_noop() {
        let mut s = PasskeySigninState::default();
        assert!(take_request(&s).is_none());
        register(&mut s);
        sign_in(&mut s);
        assert_eq!((s.generation, s.action), (1, PasskeyAction::Register));
        mark_running(&mut s);
        assert!(take_request(&s).is_none());
        sign_in(&mut s);
        assert_eq!((s.phase, s.generation), (PasskeyPhase::Running, 1));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = running(register);
        let g = s.generation + 1;
        finish(&mut s, g, Ok(CoreOutcome::Unsupported));
        assert_eq!(s.phase, PasskeyPhase::Running);
        assert!(s.status.is_empty());
        let mut idle = PasskeySigninState::default();
        finish(&mut idle, 0, Ok(CoreOutcome::Unsupported));
        assert!(idle.status.is_empty());
    }

    #[test]
    fn results_are_distinct_and_never_claim_sign_in() {
        let all = [
            shown(Ok(CoreOutcome::Registered(summary(1)))),
            shown(Ok(CoreOutcome::AssertionReturned(summary(1)))),
            shown(Ok(CoreOutcome::NotAllowed)),
            shown(Ok(CoreOutcome::Unsupported)),
            shown(Err("x".into())),
        ];
        let texts: Vec<String> = all
            .iter()
            .map(|s| format!("{}|{}|{}", s.status, s.error, s.detail))
            .collect();
        for (i, a) in texts.iter().enumerate() {
            assert!(!a.contains("サインインしました") && !a.contains("認証に成功"));
            for b in &texts[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(all[2].error.is_empty() && !all[2].status.is_empty());
        assert!(all[4].status.is_empty() && all[4].error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn same_credential_check_matches_unmatched_and_unregistered() {
        let mut s = shown(Ok(CoreOutcome::AssertionReturned(summary(1))));
        assert!(s.detail.contains(NOT_REGISTERED_TEXT));
        s = shown(Ok(CoreOutcome::Registered(summary(1))));
        sign_in(&mut s);
        mark_running(&mut s);
        let g = s.generation;
        finish(&mut s, g, Ok(CoreOutcome::AssertionReturned(summary(1))));
        assert!(s.detail.contains(SAME_TEXT));
        assert!(s.detail.contains("要求した RP ID: localhost"));
        sign_in(&mut s);
        mark_running(&mut s);
        let g = s.generation;
        finish(&mut s, g, Ok(CoreOutcome::AssertionReturned(summary(2))));
        assert!(s.detail.contains(DIFFERENT_TEXT));
    }

    #[test]
    fn notice_always_shown_and_buttons_hidden_while_busy() {
        let idle = format!("{:?}", current_screen(&PasskeySigninState::default()));
        let busy = format!("{:?}", current_screen(&running(register)));
        assert!(idle.contains("サーバー検証なし") && busy.contains("サーバー検証なし"));
        assert!(idle.contains("パスキーを登録") && idle.contains("パスキーでサインイン"));
        assert!(!busy.contains("パスキーを登録") && !busy.contains("パスキーでサインイン"));
    }

    #[test]
    fn debug_hides_credential_id() {
        let s = shown(Ok(CoreOutcome::Registered(CoreSummary {
            credential_id: CredentialMemo(vec![0xAB; 4]),
            ..summary(0)
        })));
        let d = format!("{s:?} {:?}", summary(0xAB));
        assert!(!d.contains("171") && d.contains("redacted"));
    }
}
