//! Phase 11（端末機能の拡充ループ、親 #905）型サンプル「生体認証」（Issue #906）。
//! 「認証する」ボタン → OS の生体認証 UI が開き、結果（認証した/キャンセル/使えない/
//! 失敗）を表示する 1 画面。`FANDHE_PROXY_APP=biometric_check` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod biometric_check;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の認証 UI 提示・入力検証は
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、
//! `device/biometric_auth.rs`）が [`take_request`] で要求を観測し [`finish_auth`] で
//! 結果を注入する。
//!
//! # 認証した・キャンセル・使えない・失敗を混同しない理由
//!
//! `share_text`/`file_pick` と同じ再発防止方針: キャンセルや失敗を黙って
//! 「認証しました」に丸めると、利用者は自分の認証が本当に成功したのか誤解しうる
//! （security.md A07「認証の失敗」）。本画面は認証した・キャンセル・使えない・失敗を
//! `status`（通常色。いずれも OS が判定した正常な結果）、それ以外の内部エラーを
//! `error`（`error_text` で `TextRole::Error`）の別フィールドへ書き分ける
//! （`share_text::ShareTextState` と同型）。
//!
//! ```text
//! Idle --「認証する」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_authenticating--> phase=Authenticating
//!   （OS の認証 UI が開く。利用者が閉じる/認証するまで戻らない）
//!   --finish_auth（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```
//!
//! 世代番号（`generation`）は [`take_request`]/[`finish_auth`] の照合に使い、
//! `Idle` へ戻った後や新しい要求で上書きされた古い完了を無視する
//! （`share_text::SharePhase` と同じ判断）。

use crate::{Screen, WidgetId};

/// 「認証する」ボタンの `WidgetId`。
const AUTH_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// OS の認証ダイアログへ渡す理由（表示用の固定文言）。
pub const AUTH_REASON: &str = "型サンプル「生体認証」の動作確認に使います";

/// 認証できたときの案内。
pub const AUTHENTICATED_STATUS: &str = "認証しました";
/// キャンセルされたときの案内（失敗ではない、モジュール doc 参照）。
pub const CANCELLED_STATUS: &str = "キャンセルしました";
/// この端末では使えないときの案内（未登録・非搭載・パスコード未設定。失敗ではない）。
pub const UNAVAILABLE_STATUS: &str = "この端末では生体認証を使えません（未登録または非搭載）";
/// 生体が一致しない・ロックアウトされたときの案内（OS が判定した正常な結果）。
pub const FAILED_STATUS: &str = "認証に失敗しました";
/// 内部エラー時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 認証操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが認証 UI を開いて処理中（[`mark_authenticating`] 済み）。
    Authenticating,
}

/// 認証結果（画面表示用。`fandhe_device::biometric_auth::AuthOutcome` と 4 腕対応する。
/// `fandhe-core` は `fandhe-device` に依存しないため、写像は `fandhe-egui-proxy` 側の
/// 責務にする、モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// OS が「本人確認に成功した」と判定した。確認方式は端末依存で、macOS/iOS は
    /// `LAPolicyDeviceOwnerAuthenticationWithBiometrics` 固定のため常に生体確認だが、
    /// Windows（`UserConsentVerifier`）は API 設計上 PIN 等の端末所有者認証も同じ成功
    /// 結果として返し、Web（WebAuthn の user verification）も PIN 等の生体以外の方式で
    /// 成立しうる（`fandhe-egui-proxy` 側 `outcome_from_verification` の doc・
    /// `fandhe-web-dom::biometric_auth` モジュール doc・`docs/device/capabilities/
    /// biometric-auth.json` の windows/web 欄参照。「生体認証に成功した」と読み替えない）。
    Authenticated,
    /// 利用者が認証 UI をキャンセルした。
    Cancelled,
    /// この端末では生体認証を使えない（未登録・非搭載・パスコード未設定）。
    Unavailable,
    /// 生体が一致しない、またはロックアウトされた。
    Failed,
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRequest {
    /// [`finish_auth`] へそのまま渡す世代番号。
    pub generation: u64,
    /// OS ダイアログへ渡す理由（固定文言、[`AUTH_REASON`]）。
    pub reason: String,
}

/// 生体認証画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BiometricCheckState {
    /// 認証操作の進行状態。
    pub phase: AuthPhase,
    /// 直近に立てた要求の世代番号。要求のたびに進み、完了通知が「今の要求」を
    /// 指しているかの照合に使う（モジュール doc 参照）。
    pub generation: u64,
    /// 認証した・キャンセル・使えない・失敗の案内文言（通常色で表示）。
    pub status: String,
    /// 内部エラー理由（`TextRole::Error` で表示）。正常系は空文字列にする
    /// （成功と失敗を取り違えないための分離、モジュール doc 参照）。
    pub error: String,
}

/// 「認証する」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。
pub fn authenticate(state: &mut BiometricCheckState) {
    if state.phase != AuthPhase::Idle {
        return;
    }
    state.phase = AuthPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果表示（status/error）を残さない（`share_text::share` と同型）。
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測するための getter（状態は変えない。
/// 実際に `Authenticating` へ進めるのは [`mark_authenticating`]）。
pub fn take_request(state: &BiometricCheckState) -> Option<AuthRequest> {
    (state.phase == AuthPhase::Requested).then(|| AuthRequest {
        generation: state.generation,
        reason: AUTH_REASON.to_string(),
    })
}

/// 要求を認証 UI 表示中（`Authenticating`）へ進める。`Requested` 以外では何もしない
/// （`take_request` が `None` を返した後に誤って呼ばれても状態を壊さない）。
pub fn mark_authenticating(state: &mut BiometricCheckState) {
    if state.phase == AuthPhase::Requested {
        state.phase = AuthPhase::Authenticating;
    }
}

/// 認証結果を反映する。`generation` は [`take_request`] が返した世代番号（呼び出し元が
/// 認証 UI 表示中も保持し続ける契約）。現在の世代と一致しない（＝新しい要求で既に
/// 上書きされた）完了は無視する。`Idle`（要求が無い）状態で届いた完了も無視する。
pub fn finish_auth(
    state: &mut BiometricCheckState,
    generation: u64,
    result: Result<AuthOutcome, String>,
) {
    if state.phase == AuthPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = AuthPhase::Idle;
    match result {
        Ok(AuthOutcome::Authenticated) => {
            state.status = AUTHENTICATED_STATUS.to_string();
            state.error = String::new();
        }
        Ok(AuthOutcome::Cancelled) => {
            state.status = CANCELLED_STATUS.to_string();
            state.error = String::new();
        }
        Ok(AuthOutcome::Unavailable) => {
            state.status = UNAVAILABLE_STATUS.to_string();
            state.error = String::new();
        }
        Ok(AuthOutcome::Failed) => {
            state.status = FAILED_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &BiometricCheckState) -> String {
    state.status.clone()
}

/// 内部エラー時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &BiometricCheckState) -> String {
    state.error.clone()
}

/// 生体認証画面を組み立てる。`registry.rs` の `effect` 区画から `fn(&S) -> Screen<S>`
/// として呼ばれる契約。`Idle` のときのみボタンを出し、処理中
/// （`Requested`/`Authenticating`）は二重押しを防ぐため外す（`share_text::current_screen`
/// と同型）。
pub fn current_screen(state: &BiometricCheckState) -> Screen<BiometricCheckState> {
    let screen = Screen::new("生体認証")
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != AuthPhase::Idle {
        return screen;
    }
    screen.button(AUTH_BUTTON_ID, "認証する", authenticate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticate_advances_generation_and_phase() {
        let mut state = BiometricCheckState::default();
        authenticate(&mut state);
        assert_eq!(state.phase, AuthPhase::Requested);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn authenticate_is_noop_while_in_flight() {
        let mut state = BiometricCheckState {
            phase: AuthPhase::Authenticating,
            generation: 1,
            ..Default::default()
        };
        authenticate(&mut state);
        assert_eq!(state.phase, AuthPhase::Authenticating);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn authenticate_clears_previous_status_and_error() {
        let mut state = BiometricCheckState {
            status: AUTHENTICATED_STATUS.to_string(),
            error: format!("{FAILED_PREFIX}前回の失敗"),
            ..Default::default()
        };
        authenticate(&mut state);
        assert_eq!(state.status, "");
        assert_eq!(state.error, "");
    }

    #[test]
    fn take_request_returns_generation_and_reason_only_when_requested() {
        let mut state = BiometricCheckState::default();
        assert_eq!(take_request(&state), None);
        authenticate(&mut state);
        assert_eq!(
            take_request(&state),
            Some(AuthRequest {
                generation: 1,
                reason: AUTH_REASON.to_string(),
            })
        );
        mark_authenticating(&mut state);
        assert_eq!(take_request(&state), None);
    }

    #[test]
    fn finish_auth_authenticated_sets_status_without_error() {
        let mut state = BiometricCheckState::default();
        authenticate(&mut state);
        mark_authenticating(&mut state);
        finish_auth(&mut state, 1, Ok(AuthOutcome::Authenticated));
        assert_eq!(state.phase, AuthPhase::Idle);
        assert_eq!(state.status, AUTHENTICATED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_auth_cancelled_does_not_say_authenticated() {
        let mut state = BiometricCheckState::default();
        authenticate(&mut state);
        mark_authenticating(&mut state);
        finish_auth(&mut state, 1, Ok(AuthOutcome::Cancelled));
        assert_eq!(state.status, CANCELLED_STATUS);
        assert_ne!(state.status, AUTHENTICATED_STATUS);
        assert!(!state.status.contains("認証しました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_auth_unavailable_does_not_say_authenticated() {
        let mut state = BiometricCheckState::default();
        authenticate(&mut state);
        mark_authenticating(&mut state);
        finish_auth(&mut state, 1, Ok(AuthOutcome::Unavailable));
        assert_eq!(state.status, UNAVAILABLE_STATUS);
        assert_ne!(state.status, AUTHENTICATED_STATUS);
        assert!(!state.status.contains("認証しました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_auth_failed_does_not_say_authenticated() {
        let mut state = BiometricCheckState::default();
        authenticate(&mut state);
        mark_authenticating(&mut state);
        finish_auth(&mut state, 1, Ok(AuthOutcome::Failed));
        assert_eq!(state.status, FAILED_STATUS);
        assert_ne!(state.status, AUTHENTICATED_STATUS);
        assert!(!state.status.contains("認証しました"));
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_auth_error_sets_error_without_success_status() {
        let mut state = BiometricCheckState::default();
        authenticate(&mut state);
        mark_authenticating(&mut state);
        finish_auth(&mut state, 1, Err("内部エラー".to_string()));
        assert_eq!(state.status, "");
        assert!(state.error.starts_with(FAILED_PREFIX));
        assert!(!state.error.contains("認証しました"));
    }

    #[test]
    fn finish_auth_ignores_stale_generation() {
        let mut state = BiometricCheckState::default();
        authenticate(&mut state); // generation = 1
        mark_authenticating(&mut state);
        finish_auth(&mut state, 1, Ok(AuthOutcome::Cancelled)); // Idle に戻る

        authenticate(&mut state); // generation = 2
        finish_auth(&mut state, 1, Ok(AuthOutcome::Authenticated));
        // 古い世代の完了は無視され、Requested のまま
        assert_eq!(state.phase, AuthPhase::Requested);
        assert_eq!(state.status, "");
    }

    #[test]
    fn finish_auth_ignores_completion_when_idle() {
        let mut state = BiometricCheckState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        finish_auth(&mut state, 0, Ok(AuthOutcome::Authenticated));
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn buttons_are_hidden_while_request_is_in_flight() {
        use crate::click_button;
        let mut state = BiometricCheckState {
            phase: AuthPhase::Authenticating,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "認証する", &mut state));
    }

    #[test]
    fn button_is_clickable_when_idle() {
        use crate::click_button;
        let mut state = BiometricCheckState::default();
        let screen = current_screen(&state);
        assert!(click_button(&screen, "認証する", &mut state));
        assert_eq!(state.phase, AuthPhase::Requested);
    }
}
