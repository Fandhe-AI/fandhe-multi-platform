//! `ImePurpose::Password` の学習・予測変換抑止をプラットフォーム API で代替担保する
//! ための状態機械（Issue #586）。
//!
//! `crate::lib` の `apply_ime_request`（`SetPurpose` を winit へ委譲するのみ）とは独立した
//! 経路として並列に呼び出す（呼び出し側契約は [`SensitiveInputTracker::observe`] 参照）。
//! `ImeRequest::SetPurpose` は winit 0.30 系では Wayland 以外（macOS・Windows・X11・iOS・
//! Android・Web）で no-op であり（`lib.rs` の `apply_ime_request` doc 参照）、`Password`
//! フィールドの入力が IME の学習辞書・予測変換候補へ残留しうる（OWASP A02）。本モジュールは
//! `SetPurpose` と `SetEnabled` の両方を観測し、「パスワードフィールドに実際にフォーカスが
//! ある」区間だけをプラットフォーム固有の抑止 API（現状 macOS の SecureEventInput、
//! [`crate::macos_secure_input`]）へ伝える。
//!
//! `ImeRequest::SetEnabled` を Password に紐づけて直接送出する方式は採らない
//! （iOS/Android ではソフトウェアキーボードごと隠れるため、PR #457）。
//! 本モジュールは `fandhe-core` 側の `SetEnabled` 契約には一切干渉しない（観測のみ）。

use fandhe_core::ui::{ImePurpose, ImeRequest};

/// 「パスワードフィールドへ実際にフォーカスがある」区間を判定する状態機械。
///
/// `purpose == Password && enabled` の論理積が真である間だけ「有効」とみなし、
/// 真偽が反転した回のみ [`Self::observe`] が `Some` を返す（同じ状態への
/// 重複要求で対の呼び出し（enable/disable）を崩さないため。§5 参照）。
#[derive(Debug, Default)]
pub struct SensitiveInputTracker {
    purpose: ImePurposeState,
    enabled: bool,
    active: bool,
}

/// `ImePurpose` はコアの `Copy` 型だが `Default` を実装しないため、
/// 未初期化状態を明示するラッパー（既定は「機微でない」＝ `Normal` 相当）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImePurposeState {
    Normal,
    Password,
    Terminal,
}

impl Default for ImePurposeState {
    fn default() -> Self {
        Self::Normal
    }
}

impl From<ImePurpose> for ImePurposeState {
    fn from(purpose: ImePurpose) -> Self {
        match purpose {
            ImePurpose::Normal => Self::Normal,
            ImePurpose::Password => Self::Password,
            ImePurpose::Terminal => Self::Terminal,
        }
    }
}

impl SensitiveInputTracker {
    /// 新規状態機械を作る（フォーカス外・`Normal` 相当の初期状態）。
    pub fn new() -> Self {
        Self::default()
    }

    /// `fandhe-core` が発行した [`ImeRequest`] を 1 件観測し、内部状態を更新する。
    ///
    /// 呼び出し側契約: `TextFieldWidget::take_ime_requests` が返す各要求をそのまま
    /// （順序を保って）本メソッドへ渡す。戻り値が `Some(true)` の回に一度だけ
    /// [`apply_sensitive_input`] へ `true` を、`Some(false)` の回に一度だけ `false` を渡すこと
    /// （プロセス全体に効く SecureEventInput 等の重複呼び出しを避けるため、`None` の回は
    /// 何もしない）。
    pub fn observe(&mut self, request: &ImeRequest) -> Option<bool> {
        match request {
            ImeRequest::SetPurpose(purpose) => self.purpose = (*purpose).into(),
            ImeRequest::SetEnabled(enabled) => self.enabled = *enabled,
            // フォーカス遷移の帰属セッション識別（Issue #587、`session` モジュール参照）
            // のみに関わる要求で、Password 抑止状態（purpose/enabled）を変更しない。
            ImeRequest::SetCursorArea { .. } | ImeRequest::BeginSession(_) => return None,
        }

        let new_active = self.purpose == ImePurposeState::Password && self.enabled;
        if new_active == self.active {
            return None;
        }
        self.active = new_active;
        Some(new_active)
    }

    /// [`Self::observe`] が `Some(requested_active)` を返した後、呼び出し側が実際に
    /// [`apply_sensitive_input`] を呼んだ結果を報告する契約（Issue #586。
    /// `requested_active` は `observe` の戻り値をそのまま渡す）。
    ///
    /// - `requested_active: true`（有効化）が失敗した場合: `active` を実際の状態（無効）へ
    ///   戻し、「保護済み」という誤った前提での処理継続や、対応しない `disable()` の発行
    ///   （Drop・次回 `observe` からの二重無効化要求）を防ぐ。
    /// - `requested_active: false`（無効化）が失敗した場合: `active` を `true` のまま
    ///   維持する（fail-closed。無効化の失敗を無視すると、`DisableSecureEventInput` が
    ///   失敗してもプロセス全体の SecureEventInput ロックが他アプリの IME 入力も含め
    ///   残存したまま、tracker は成功済みとみなし `Drop` でも再試行されなくなる）。
    ///   `active` を維持することで、次に本当に状態が変化した際（次の `observe` 呼び出し、
    ///   または `Drop`）に無効化が再試行される。
    pub fn record_apply_result(&mut self, requested_active: bool, succeeded: bool) {
        if succeeded {
            return;
        }
        // 要求どおりの状態遷移に失敗した = 実際の状態は要求と逆のまま。
        self.active = !requested_active;
    }
}

impl Drop for SensitiveInputTracker {
    /// ウィンドウ・フィールドが破棄される際、有効化したままプロセス全体のグローバル状態
    /// （macOS の SecureEventInput 等）を取りこぼさないよう、有効なら必ず無効化する
    /// （A05 セキュリティ設定ミス対策。二重 disable は各プラットフォーム実装側の責務で
    /// 冪等に扱う、`macos_secure_input` 参照）。
    fn drop(&mut self) {
        if self.active {
            apply_sensitive_input(false);
        }
    }
}

/// [`SensitiveInputTracker::observe`] が返した真偽値を、実際のプラットフォーム抑止 API へ
/// 反映する。macOS 以外は現状抑止手段がなく no-op（`docs/architecture/ime-known-constraints.md`
/// §3.3 に非対応として明記済み）。
///
/// 戻り値は「要求どおりの状態に実際に到達したか」（`active: true` を要求して
/// `EnableSecureEventInput` が成功したか、`active: false` を要求して
/// `DisableSecureEventInput` が成功したか）。この戻り値と渡した `active` は
/// [`SensitiveInputTracker::record_apply_result`] へそのまま渡す契約とする（Issue #586。
/// 無効化側の失敗を握りつぶすと、`DisableSecureEventInput` が実際には失敗しても
/// プロセス全体の SecureEventInput ロックが残存したまま tracker は成功済みとみなし
/// 再試行されなくなる）。非対応プラットフォームでは抑止できていない実態を反映し
/// `false` を返す。
pub fn apply_sensitive_input(active: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        if active {
            crate::macos_secure_input::enable().is_ok()
        } else {
            crate::macos_secure_input::disable().is_ok()
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = active;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_activates_only_when_password_and_enabled() {
        let mut tracker = SensitiveInputTracker::new();
        assert_eq!(
            tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Password)),
            None
        );
        assert_eq!(tracker.observe(&ImeRequest::SetEnabled(true)), Some(true));
    }

    #[test]
    fn observe_ignores_cursor_area() {
        let mut tracker = SensitiveInputTracker::new();
        assert_eq!(
            tracker.observe(&ImeRequest::SetCursorArea {
                origin: (0.0, 0.0),
                size: (0.0, 0.0)
            }),
            None
        );
    }

    #[test]
    fn observe_deactivates_on_focus_lost() {
        let mut tracker = SensitiveInputTracker::new();
        tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Password));
        tracker.observe(&ImeRequest::SetEnabled(true));
        assert_eq!(tracker.observe(&ImeRequest::SetEnabled(false)), Some(false));
    }

    #[test]
    fn observe_deactivates_on_purpose_switch_away_from_password() {
        let mut tracker = SensitiveInputTracker::new();
        tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Password));
        tracker.observe(&ImeRequest::SetEnabled(true));
        assert_eq!(
            tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Normal)),
            Some(false)
        );
    }

    #[test]
    fn observe_does_not_activate_for_non_password_purpose_while_enabled() {
        let mut tracker = SensitiveInputTracker::new();
        assert_eq!(tracker.observe(&ImeRequest::SetEnabled(true)), None);
        assert_eq!(
            tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Terminal)),
            None
        );
    }

    #[test]
    fn observe_does_not_repeat_same_state() {
        let mut tracker = SensitiveInputTracker::new();
        tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Password));
        assert_eq!(tracker.observe(&ImeRequest::SetEnabled(true)), Some(true));
        // 同じ purpose を再送しても既に active のため反応しない（重複 enable 抑止）。
        assert_eq!(
            tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Password)),
            None
        );
    }

    #[test]
    fn drop_disables_when_still_active() {
        // Drop 自体は apply_sensitive_input(false) を呼ぶのみで panic しないことを確認する
        // （macOS 以外では apply_sensitive_input は no-op、macOS でも安全に冪等）。
        let mut tracker = SensitiveInputTracker::new();
        tracker.observe(&ImeRequest::SetPurpose(ImePurpose::Password));
        tracker.observe(&ImeRequest::SetEnabled(true));
        drop(tracker);
    }
}
