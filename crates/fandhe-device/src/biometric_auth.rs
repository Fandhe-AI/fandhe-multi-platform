//! 生体認証の抽象（Phase 11 の拡充ループ、親 #905、Issue #906）。
//!
//! `Share`/`FilePicker` と同じ request/response の一発呼び出し型。結果は 4 値:
//! 認証した / キャンセル / 使えない（未登録・非搭載・パスコード未設定） / 失敗
//! （不一致・ロックアウト）を区別し、キャンセル・失敗・使えないが「認証した」へ
//! 化けない契約をテストで固定する。
//!
//! macOS/iOS は OS がコールバック型（`LAContext.evaluatePolicy` の reply が非同期で
//! 届く）のため本トレイトを **通らず**、`fandhe_core::screens::biometric_check`
//! （端末依存ゼロの状態機械）へ直接書き戻す（`share.rs` モジュール doc と同じ判断）。
//! Windows（#907、`UserConsentVerifier` の `IAsyncOperation` 完了ハンドラ）も同じ
//! 判断で本トレイトを通らず直接書き戻す。Android（#908）も同様の非同期経路になる見込み。
//! 対応不可プラットフォーム（Linux、本 Issue 時点の Android）は
//! `Err(DeviceError::Unsupported("biometric_auth"))`。
//!
//! 認証結果以外の情報（生体データ・テンプレート・端末識別子）は保持・ログ出力しない
//! （security.md A09）。本トレイト・実装は端末ローカルの可否確認であり、サーバー側の
//! 本人認証基盤の代替ではない。

use crate::DeviceError;

/// 生体認証の結果。キャンセル・使えない・失敗は「認証した」ではない（モジュール doc の契約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// OS の成功コールバック（または同期成功）が届いた。
    Authenticated,
    /// 利用者が認証 UI をキャンセルした（フォールバックへの遷移要求も含めて畳む）。
    Cancelled,
    /// 生体認証が未登録・非搭載・パスコード未設定などで利用できない。
    Unavailable,
    /// 生体が一致しない、またはロックアウトされた。
    Failed,
}

/// 認証理由（OS ダイアログに表示する文言）を検証する。trim 後に空、または内部に
/// NUL を含む場合は `InvalidInput("biometric reason")`（`CString` 化する呼び出し元
/// での失敗を事前に防ぐ、`share::ShareRequest::new` と同じ SSOT 方針）。
pub fn validate_reason(reason: &str) -> Result<&str, DeviceError> {
    let trimmed = reason.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(DeviceError::InvalidInput("biometric reason"));
    }
    Ok(trimmed)
}

/// 生体認証の契約。
pub trait BiometricAuth: Send + Sync {
    /// この端末で生体認証を使えるか（未登録・非搭載を区別しない可否判定のみ）。
    fn available(&self) -> Result<bool, DeviceError>;
    /// OS の認証 UI を 1 回出す。固定タイムアウトを掛けない。
    fn authenticate(&self, reason: &str) -> Result<AuthOutcome, DeviceError>;
}

/// 対応不可プラットフォーム用（Linux、本 Issue 時点の Android）。常に
/// `Unsupported("biometric_auth")` を返す。
pub struct UnsupportedBiometricAuth;

impl BiometricAuth for UnsupportedBiometricAuth {
    fn available(&self) -> Result<bool, DeviceError> {
        Err(DeviceError::Unsupported("biometric_auth"))
    }

    fn authenticate(&self, _reason: &str) -> Result<AuthOutcome, DeviceError> {
        Err(DeviceError::Unsupported("biometric_auth"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_reason_rejects_empty() {
        let err = validate_reason("").unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("biometric reason")));
    }

    #[test]
    fn validate_reason_rejects_blank_only() {
        let err = validate_reason("   ").unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("biometric reason")));
    }

    #[test]
    fn validate_reason_rejects_embedded_nul() {
        let err = validate_reason("a\0b").unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("biometric reason")));
    }

    #[test]
    fn validate_reason_trims() {
        assert_eq!(validate_reason("  hello  ").unwrap(), "hello");
    }

    #[test]
    fn unsupported_biometric_auth_returns_unsupported_error() {
        let err = UnsupportedBiometricAuth.available().unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("biometric_auth")));
        let err = UnsupportedBiometricAuth.authenticate("test").unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("biometric_auth")));
    }

    /// モック実装（`Cancelled` 固定）が `Authenticated` へ化けないことを固定する
    /// （契約の 4 値を取り違えない、`share.rs::AlwaysCancelledShare` と同じ目的）。
    struct AlwaysCancelledAuth;

    impl BiometricAuth for AlwaysCancelledAuth {
        fn available(&self) -> Result<bool, DeviceError> {
            Ok(true)
        }
        fn authenticate(&self, _reason: &str) -> Result<AuthOutcome, DeviceError> {
            Ok(AuthOutcome::Cancelled)
        }
    }

    #[test]
    fn mock_cancelled_auth_does_not_become_authenticated() {
        let outcome = AlwaysCancelledAuth.authenticate("test").unwrap();
        assert_eq!(outcome, AuthOutcome::Cancelled);
        assert_ne!(outcome, AuthOutcome::Authenticated);
    }

    /// モック実装（`Unavailable` 固定）が `Authenticated` へ化けないことを固定する。
    struct AlwaysUnavailableAuth;

    impl BiometricAuth for AlwaysUnavailableAuth {
        fn available(&self) -> Result<bool, DeviceError> {
            Ok(false)
        }
        fn authenticate(&self, _reason: &str) -> Result<AuthOutcome, DeviceError> {
            Ok(AuthOutcome::Unavailable)
        }
    }

    #[test]
    fn mock_unavailable_auth_does_not_become_authenticated() {
        let outcome = AlwaysUnavailableAuth.authenticate("test").unwrap();
        assert_eq!(outcome, AuthOutcome::Unavailable);
        assert_ne!(outcome, AuthOutcome::Authenticated);
    }

    /// モック実装（`Failed` 固定）が `Authenticated` へ化けないことを固定する。
    struct AlwaysFailedAuth;

    impl BiometricAuth for AlwaysFailedAuth {
        fn available(&self) -> Result<bool, DeviceError> {
            Ok(true)
        }
        fn authenticate(&self, _reason: &str) -> Result<AuthOutcome, DeviceError> {
            Ok(AuthOutcome::Failed)
        }
    }

    #[test]
    fn mock_failed_auth_does_not_become_authenticated() {
        let outcome = AlwaysFailedAuth.authenticate("test").unwrap();
        assert_eq!(outcome, AuthOutcome::Failed);
        assert_ne!(outcome, AuthOutcome::Authenticated);
    }
}
