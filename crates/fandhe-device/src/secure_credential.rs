//! 資格情報（パスワード・トークン等）の安全な保存の抽象（Phase 11 の拡充ループ、
//! 親 #921、Issue #922）。`storage::FsStorage` は平文ファイル保存のため資格情報の
//! 保存先には使えない（`storage.rs` doc 参照）。本トレイトは OS のセキュアストア
//! （macOS/iOS Keychain・Android Keystore・Windows PasswordVault 等）への保存 /
//! 読み出し / 削除を抽象化する。
//!
//! 契約: 「無い」は `Ok(None)`（`load`）、削除は冪等（`delete` は無くても `Ok(())`）、
//! 失敗は `Err`（`biometric_auth.rs` と同じ「使えない/失敗を成功に化けさせない」方針）。
//! macOS/iOS の実装は `fandhe-egui-proxy` 側の FFI（`apple_secure_credential.rs`）に置く
//! （本クレートは `#![forbid(unsafe_code)]` のため FFI を持たない、`lib.rs` doc 参照）。
//!
//! [`Secret`] は `Debug` で値そのものを出さない（バイト数のみ）。呼び出し元
//! （`fandhe-core` の画面状態）も同様に値をマスクする契約（security.md A09）。

use crate::{DeviceError, storage};

/// 保存できる秘密値の最大バイト数（UTF-8）。超過は `InvalidInput` で明示失敗させ、
/// 固定長バッファを使う FFI 実装（macOS Keychain 等）側の上限と揃える。
pub const SECRET_MAX_BYTES: usize = 4096;

/// key の最大文字数。`storage::validate_key` の許可文字集合に加え、OS 側 API
/// （Keychain の `kSecAttrAccount` 等）へ渡す識別子として妥当な長さに制限する。
const KEY_MAX_LEN: usize = 64;

/// 秘密値を保持する型。`Debug` はバイト数のみを出し、値そのものは表示しない
/// （security.md A09、呼び出し元のログ・エラー文言・画面表示から値が漏れないための
/// 最終防波堤）。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// 秘密値を保持する `Secret` を構築する。検証済みの値を渡す契約
    /// （[`validate_secret`] は呼び出し元で先に通す）。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 保持している秘密値そのものを取り出す（呼び出し元が明示的に要求した場合のみ
    /// 使う契約。`fandhe-core` 側の画面は表示時にマスクへ戻す、`credential_vault.rs`
    /// の `reveal` トグル参照）。
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// 秘密値を検証する。空文字列・内部 NUL・[`SECRET_MAX_BYTES`] 超過を拒否する
/// （NUL 拒否は `CString` 化する FFI 実装での失敗を事前に防ぐ、`biometric_auth::
/// validate_reason` と同じ SSOT 方針）。
pub fn validate_secret(secret: &str) -> Result<&str, DeviceError> {
    if secret.is_empty() || secret.contains('\0') || secret.len() > SECRET_MAX_BYTES {
        return Err(DeviceError::InvalidInput("secure_credential secret"));
    }
    Ok(secret)
}

/// key を検証する。`storage::validate_key`（許可文字集合・パストラバーサル拒否）に
/// 加え、[`KEY_MAX_LEN`] 超過を拒否する。
pub fn validate_key(key: &str) -> Result<&str, DeviceError> {
    storage::validate_key(key)?;
    if key.len() > KEY_MAX_LEN {
        return Err(DeviceError::InvalidKey(key.to_string()));
    }
    Ok(key)
}

/// 資格情報の安全な保存の契約。実装は OS のセキュアストアへの保存を担い、
/// key はアプリ固有プレフィックス配下に閉じる（呼び出し元の外部入力をそのまま
/// OS 識別子へ連結しない、実装側の責務）。
pub trait SecureCredential: Send + Sync {
    /// `key` に `secret` を保存する。既存値があれば置き換える。
    fn store(&self, key: &str, secret: &str) -> Result<(), DeviceError>;
    /// `key` の値を読む。未存在は `Ok(None)`（エラーにしない契約）。
    fn load(&self, key: &str) -> Result<Option<Secret>, DeviceError>;
    /// `key` の値を削除する。未存在でも `Ok(())`（冪等）。
    fn delete(&self, key: &str) -> Result<(), DeviceError>;
}

/// 対応不可プラットフォーム用（Linux、Web）。Windows は `fandhe-egui-proxy` 側
/// `device/windows_secure_credential.rs`（`WindowsPasswordVault`、Issue #923）、
/// iOS/macOS/Android も同クレート側で対応済み。常に
/// `Unsupported("secure_credential")` を返す。
pub struct UnsupportedSecureCredential;

impl SecureCredential for UnsupportedSecureCredential {
    fn store(&self, _key: &str, _secret: &str) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("secure_credential"))
    }

    fn load(&self, _key: &str) -> Result<Option<Secret>, DeviceError> {
        Err(DeviceError::Unsupported("secure_credential"))
    }

    fn delete(&self, _key: &str) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("secure_credential"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn secret_debug_does_not_contain_value() {
        let secret = Secret::new("hunter2-test");
        let debug = format!("{secret:?}");
        assert!(!debug.contains("hunter2"));
        assert!(debug.contains("bytes"));
        assert!(debug.contains("12"));
    }

    #[test]
    fn validate_secret_rejects_empty() {
        assert!(matches!(
            validate_secret("").unwrap_err(),
            DeviceError::InvalidInput("secure_credential secret")
        ));
    }

    #[test]
    fn validate_secret_rejects_embedded_nul() {
        assert!(matches!(
            validate_secret("a\0b").unwrap_err(),
            DeviceError::InvalidInput("secure_credential secret")
        ));
    }

    #[test]
    fn validate_secret_rejects_oversize() {
        let oversize = "a".repeat(SECRET_MAX_BYTES + 1);
        assert!(matches!(
            validate_secret(&oversize).unwrap_err(),
            DeviceError::InvalidInput("secure_credential secret")
        ));
    }

    #[test]
    fn validate_secret_accepts_max_size() {
        let max = "a".repeat(SECRET_MAX_BYTES);
        assert_eq!(validate_secret(&max).unwrap(), max);
    }

    #[test]
    fn validate_key_rejects_traversal_and_separator() {
        assert!(matches!(
            validate_key("../x").unwrap_err(),
            DeviceError::InvalidKey(_)
        ));
        assert!(matches!(
            validate_key("a/b").unwrap_err(),
            DeviceError::InvalidKey(_)
        ));
    }

    #[test]
    fn validate_key_rejects_too_long() {
        let long = "a".repeat(KEY_MAX_LEN + 1);
        assert!(matches!(
            validate_key(&long).unwrap_err(),
            DeviceError::InvalidKey(_)
        ));
    }

    #[test]
    fn validate_key_accepts_max_len() {
        let max = "a".repeat(KEY_MAX_LEN);
        assert_eq!(validate_key(&max).unwrap(), max);
    }

    #[test]
    fn unsupported_returns_unsupported_error() {
        let cred = UnsupportedSecureCredential;
        assert!(matches!(
            cred.store("k", "v").unwrap_err(),
            DeviceError::Unsupported("secure_credential")
        ));
        assert!(matches!(
            cred.load("k").unwrap_err(),
            DeviceError::Unsupported("secure_credential")
        ));
        assert!(matches!(
            cred.delete("k").unwrap_err(),
            DeviceError::Unsupported("secure_credential")
        ));
    }

    /// インメモリモック（テストのみ）。`SecureCredential` 実装の往復契約
    /// （無い=`Ok(None)`・削除は冪等）を固定する。
    #[derive(Default)]
    struct MemCredential(Arc<Mutex<HashMap<String, String>>>);

    impl SecureCredential for MemCredential {
        fn store(&self, key: &str, secret: &str) -> Result<(), DeviceError> {
            self.0
                .lock()
                .expect("lock は成功する")
                .insert(key.to_string(), secret.to_string());
            Ok(())
        }

        fn load(&self, key: &str) -> Result<Option<Secret>, DeviceError> {
            Ok(self
                .0
                .lock()
                .expect("lock は成功する")
                .get(key)
                .cloned()
                .map(Secret::new))
        }

        fn delete(&self, key: &str) -> Result<(), DeviceError> {
            self.0.lock().expect("lock は成功する").remove(key);
            Ok(())
        }
    }

    #[test]
    fn load_missing_key_is_none() {
        let cred = MemCredential::default();
        assert_eq!(cred.load("absent").unwrap(), None);
    }

    #[test]
    fn store_then_load_round_trips() {
        let cred = MemCredential::default();
        cred.store("demo", "secret-value").unwrap();
        let loaded = cred.load("demo").unwrap().expect("値がある");
        assert_eq!(loaded.expose(), "secret-value");
    }

    #[test]
    fn delete_then_load_is_none() {
        let cred = MemCredential::default();
        cred.store("demo", "secret-value").unwrap();
        cred.delete("demo").unwrap();
        assert_eq!(cred.load("demo").unwrap(), None);
    }

    #[test]
    fn store_overwrites() {
        let cred = MemCredential::default();
        cred.store("demo", "first").unwrap();
        cred.store("demo", "second").unwrap();
        assert_eq!(cred.load("demo").unwrap().unwrap().expose(), "second");
    }

    #[test]
    fn delete_missing_is_ok() {
        let cred = MemCredential::default();
        assert!(cred.delete("absent").is_ok());
    }
}
