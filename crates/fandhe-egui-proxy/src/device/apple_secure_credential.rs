//! iOS / macOS 向け資格情報保存実装（Issue #922・#924）。`device/secure_credential.rs`
//! の iOS/macOS 経路から呼ばれ、`fandhe_device::secure_credential::SecureCredential` を
//! Keychain Services（`apple/fandhe_apple_secure_credential.m`、`build.rs` が iOS・
//! macOS 双方のターゲット・feature `device` 有効時のみコンパイル・リンクする）で
//! 実装する。Security Services は iOS/macOS 同一 API のため `.m` はそのまま共用する
//! （分岐なし、Issue #924 調査）。
//!
//! `unsafe` はこのファイルの 3 呼び出しに閉じ、各 `// SAFETY:` で契約を明記する
//! （`apple_biometric.rs::present` と同型、coding-rust.md の FFI 例外規定準拠）。

use std::ffi::CString;
use std::os::raw::c_char;

use fandhe_device::secure_credential::{
    validate_key, validate_secret, Secret, SecureCredential, SECRET_MAX_BYTES,
};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_secure_credential.m` が定義する C ABI（`build.rs` が
// macOS ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は
// `.m` ファイル冒頭コメントおよび下記 `AppleKeychain` 各メソッドのコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_secure_credential_store(
        key: *const c_char,
        secret: *const u8,
        secret_len: usize,
    ) -> i32;
    fn fandhe_apple_secure_credential_load(
        key: *const c_char,
        out: *mut u8,
        cap: usize,
        out_len: *mut usize,
    ) -> i32;
    fn fandhe_apple_secure_credential_delete(key: *const c_char) -> i32;
}

/// `.m` 側の戻り値コード → `DeviceError` の写像（1 関数に集約、`.m` ファイル冒頭
/// コメントの表と対）。`load` の「無い」（コード 1）は呼び出し元が `Ok(None)` へ
/// 個別に写すため、ここでは `DeviceError` は返さず `Found` 相当の分岐は呼び出し元に
/// 委ねる形にせず、統一的に「コード → エラー」だけを扱う（成功・「無い」は呼び出し元
/// が個別分岐する）。
fn status_to_error(code: i32) -> DeviceError {
    match code {
        2 => DeviceError::InvalidInput("secure_credential key"),
        3 => DeviceError::Internal("secure_credential: buffer too small"),
        _ => DeviceError::Internal("secure_credential"),
    }
}

/// macOS Keychain（Security.framework）による `SecureCredential` 実装。
pub(crate) struct AppleKeychain;

impl SecureCredential for AppleKeychain {
    #[allow(unsafe_code)]
    fn store(&self, key: &str, secret: &str) -> Result<(), DeviceError> {
        let key = validate_key(key)?;
        let secret = validate_secret(secret)?;
        // `validate_key` は英数字・`.`/`_`/`-` のみを許可しているため内部 NUL は
        // 含み得ないが、契約が変わっても panic させない防御として `CString::new`
        // の失敗を明示的に扱う。
        let Ok(c_key) = CString::new(key) else {
            return Err(DeviceError::InvalidInput("secure_credential key"));
        };

        // SAFETY: `c_key` はこの呼び出しが返るまで生存するスタック上の `CString`。
        // `secret.as_ptr()`/`secret.len()` も同様にこの呼び出し中のみ有効。`.m` 側は
        // 返る前に値を `NSData`/Keychain へコピーする契約（`.m` ファイル冒頭コメント）
        // のため、ポインタの所有権は移らない。
        let code = unsafe {
            fandhe_apple_secure_credential_store(c_key.as_ptr(), secret.as_ptr(), secret.len())
        };
        if code == 0 {
            Ok(())
        } else {
            Err(status_to_error(code))
        }
    }

    #[allow(unsafe_code)]
    fn load(&self, key: &str) -> Result<Option<Secret>, DeviceError> {
        let key = validate_key(key)?;
        let Ok(c_key) = CString::new(key) else {
            return Err(DeviceError::InvalidInput("secure_credential key"));
        };

        let mut buf = vec![0u8; SECRET_MAX_BYTES];
        let mut out_len: usize = 0;

        // SAFETY: `c_key` はこの呼び出し中のみ参照される。`buf` は `SECRET_MAX_BYTES`
        // 長で確保済みの固定バッファで `cap == buf.len()` を渡す。`.m` 側は
        // `out_len` に実際に書いたバイト数（成功時）または必要なバイト数
        // （`cap` 不足時）のみを書く契約で、`out` はその範囲内でのみ書き込まれる
        // （`.m` ファイル冒頭コメント）。
        let code = unsafe {
            fandhe_apple_secure_credential_load(
                c_key.as_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut out_len,
            )
        };

        match code {
            0 => {
                buf.truncate(out_len);
                match String::from_utf8(buf) {
                    Ok(value) => Ok(Some(Secret::new(value))),
                    // 保存時は UTF-8 文字列のみを扱う契約（`validate_secret`）のため
                    // 通常は起きないが、Keychain 側に別経路で非 UTF-8 が書かれた
                    // 場合に備えて明示的に失敗させる（値をエラー文言へ含めない）。
                    Err(_) => Err(DeviceError::Internal("secure_credential: invalid utf-8")),
                }
            }
            1 => Ok(None),
            _ => Err(status_to_error(code)),
        }
    }

    #[allow(unsafe_code)]
    fn delete(&self, key: &str) -> Result<(), DeviceError> {
        let key = validate_key(key)?;
        let Ok(c_key) = CString::new(key) else {
            return Err(DeviceError::InvalidInput("secure_credential key"));
        };

        // SAFETY: `c_key` はこの呼び出しが返るまで生存する。引数はこれのみで
        // 出力ポインタは無い。
        let code = unsafe { fandhe_apple_secure_credential_delete(c_key.as_ptr()) };
        match code {
            // 「無い」は削除の冪等性契約により成功として扱う
            // （`fandhe_device::secure_credential::SecureCredential::delete` の doc）。
            0 | 1 => Ok(()),
            _ => Err(status_to_error(code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_to_error_maps_known_codes() {
        assert!(matches!(
            status_to_error(2),
            DeviceError::InvalidInput("secure_credential key")
        ));
        assert!(matches!(
            status_to_error(3),
            DeviceError::Internal("secure_credential: buffer too small")
        ));
        assert!(matches!(
            status_to_error(4),
            DeviceError::Internal("secure_credential")
        ));
        assert!(matches!(
            status_to_error(99),
            DeviceError::Internal("secure_credential")
        ));
    }

    // 実 Keychain を叩く往復テストは実機依存（許可ダイアログ・ログインキーチェーン
    // 状態に左右される）のため、CI 向けユニットテストには含めない
    // （実装計画 §6「実挙動」は Mac 目視で担保、Step 6 参照）。`status_to_error` の
    // 写像固定と `SecureCredential` トレイト実装であることの型検査のみをここで行う。
    #[test]
    fn apple_keychain_implements_secure_credential() {
        fn assert_impl<T: SecureCredential>() {}
        assert_impl::<AppleKeychain>();
    }
}
