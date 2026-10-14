//! Windows 向け資格情報保存実装（Issue #923）。`device/secure_credential.rs` の
//! Windows 経路から呼ばれ、`fandhe_device::secure_credential::SecureCredential` を
//! WinRT `Windows.Security.Credentials.PasswordVault`（Credential Locker）で
//! 実装する。macOS 実装（`apple_secure_credential.rs`、Keychain FFI）と異なり
//! `windows` クレート側が safe fn としてラップ済みのため `unsafe` は不要
//! （`windows_text_scale.rs` と同型。`lib.rs` の Windows `deny(unsafe_code)` に対する
//! `#[allow(unsafe_code)]` も付けない）。RoInitialize も呼ばない
//! （`windows-core` が `CO_E_NOTINITIALIZED` 時に MTA へフォールバックする既定挙動、
//! #898/#907/#913 と同じ前提）。
//!
//! 識別子は macOS 実装（`apple/fandhe_apple_secure_credential.m`: service
//! `com.fandhe.egui-proxy-desktop`、account `fandhe.` + key）と揃える: resource は
//! アプリ固有の固定文字列、user name は `fandhe.` + key。
//!
//! `load`/`delete` の「無い」判定は `Retrieve` の失敗 HRESULT が
//! `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`（`0x80070490`）かどうかで行う
//! （それ以外の失敗は `Err`、`SecureCredential` トレイトの契約どおり「無い」と
//! 「失敗」を区別する）。
//!
//! HRESULT の生値・秘密値をログ・エラー文言へ出さず、固定ラベルの
//! `DeviceError::Internal` へ畳む（`windows_share.rs`/`windows_biometric.rs`/
//! `windows_text_scale.rs` と同じ方針、security.md A09）。
//!
//! # 既知の天井
//!
//! `PasswordVault` は同一 Windows ユーザーの他アプリからも `RetrieveAll` 等で
//! 列挙・取得できる（アプリ単位の分離は resource 名頼みで、OS レベルの強制分離は
//! 無い）。より強い分離（DPAPI + ファイル方式等）が必要になった場合は別 Issue で
//! 検討する（対応表 `docs/device/capabilities/secure-credential.json` の
//! `windows.note` にも記録）。

use fandhe_device::secure_credential::{validate_key, validate_secret, Secret, SecureCredential};
use fandhe_device::DeviceError;
use windows::core::HSTRING;
use windows::Security::Credentials::{PasswordCredential, PasswordVault};
use windows::Win32::Foundation::ERROR_NOT_FOUND;

/// PasswordVault の resource 識別子。macOS Keychain 実装の service 文字列と揃える。
const RESOURCE: &str = "com.fandhe.egui-proxy-desktop";

/// user name の prefix。macOS Keychain 実装の account 文字列（`fandhe.` + key）と揃える。
const USER_PREFIX: &str = "fandhe.";

/// `Retrieve` の失敗が「無い」（未登録）によるものかを判定する。
/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)` との一致で判定する
/// （`ERROR_NOT_FOUND` = 1168 = `0x80070490`）。
fn is_not_found(error: &windows::core::Error) -> bool {
    error.code() == windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0)
}

/// `PasswordVault` インスタンスを生成する。状態を持たないため呼び出しごとに
/// 生成する（`windows_text_scale.rs::WindowsTextScale` と同じ方針。resource 単位の
/// 操作頻度は低く常駐コストを避ける）。
fn vault() -> Result<PasswordVault, DeviceError> {
    PasswordVault::new().map_err(|_| DeviceError::Internal("secure_credential vault"))
}

/// 検証済み key から user name（`USER_PREFIX` + key）を組み立てる。
fn user_name(key: &str) -> HSTRING {
    HSTRING::from(format!("{USER_PREFIX}{key}"))
}

/// resource（固定文字列）と `user` に対応する `PasswordCredential` を取得する。
/// 「無い」は `Ok(None)`、それ以外の失敗は `Err`。
fn retrieve(
    vault: &PasswordVault,
    user: &HSTRING,
) -> Result<Option<PasswordCredential>, DeviceError> {
    match vault.Retrieve(&HSTRING::from(RESOURCE), user) {
        Ok(cred) => Ok(Some(cred)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(_) => Err(DeviceError::Internal("secure_credential load")),
    }
}

/// WinRT `PasswordVault`（Credential Locker）による `SecureCredential` 実装。
pub(crate) struct WindowsPasswordVault;

impl SecureCredential for WindowsPasswordVault {
    fn store(&self, key: &str, secret: &str) -> Result<(), DeviceError> {
        let key = validate_key(key)?;
        let secret = validate_secret(secret)?;
        let vault = vault()?;
        let user = user_name(key);
        let cred = PasswordCredential::CreatePasswordCredential(
            &HSTRING::from(RESOURCE),
            &user,
            &HSTRING::from(secret),
        )
        .map_err(|_| DeviceError::Internal("secure_credential store"))?;
        // `Add` は同一 resource + user の既存項目があると「置き換え」として成功する
        // （PasswordVault の仕様。Microsoft のドキュメントに準拠）。したがって
        // Remove→再 Add のフォールバックは不要（PR #930 レビュー指摘）だけでなく
        // 危険: 既存値がある状態で最初の Add が失敗 → Retrieve/Remove 成功 → 再 Add と
        // 復元 Add がいずれも失敗、という順序で旧資格情報が失われる窓を作ってしまう。
        // Add の失敗はそのまま返す。
        vault
            .Add(&cred)
            .map_err(|_| DeviceError::Internal("secure_credential store"))
    }

    fn load(&self, key: &str) -> Result<Option<Secret>, DeviceError> {
        let key = validate_key(key)?;
        let vault = vault()?;
        let user = user_name(key);
        let Some(cred) = retrieve(&vault, &user)? else {
            return Ok(None);
        };
        // `Password()` は事前に `RetrievePassword()` を呼ばないと空を返す契約
        // （WinRT `PasswordCredential` の仕様）。
        cred.RetrievePassword()
            .map_err(|_| DeviceError::Internal("secure_credential load"))?;
        let password = cred
            .Password()
            .map_err(|_| DeviceError::Internal("secure_credential load"))?;
        Ok(Some(Secret::new(password.to_string_lossy())))
    }

    fn delete(&self, key: &str) -> Result<(), DeviceError> {
        let key = validate_key(key)?;
        let vault = vault()?;
        let user = user_name(key);
        let Some(cred) = retrieve(&vault, &user)? else {
            // 「無い」は削除の冪等性契約により成功として扱う
            // （`SecureCredential::delete` の doc）。
            return Ok(());
        };
        vault
            .Remove(&cred)
            .map_err(|_| DeviceError::Internal("secure_credential delete"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_password_vault_implements_secure_credential() {
        fn assert_impl<T: SecureCredential>() {}
        assert_impl::<WindowsPasswordVault>();
    }

    #[test]
    fn is_not_found_matches_error_not_found_hresult() {
        let error =
            windows::core::Error::from(windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0));
        assert!(is_not_found(&error));
    }

    #[test]
    fn is_not_found_rejects_other_hresult() {
        let error = windows::core::Error::from(windows::core::HRESULT(-1));
        assert!(!is_not_found(&error));
    }
}
