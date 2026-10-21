//! Windows 向け Web 認証セッション実装（Issue #531、親 #113）。`device/web_auth_session.rs` の
//! Windows 経路から呼ばれる。`ShellExecuteW` で既定ブラウザを開き、コールバックは
//! `127.0.0.1` の一時ポート（`DemoAuthServer`）で受ける（RFC 8252 §7.3）。
//! `WebAuthenticationBroker` はデスクトップアプリで使えないため使わない。契約の SSOT は
//! `fandhe_device::web_auth_session`。
//!
//! 乱数は `BCryptGenRandom`、SHA-256（PKCE の S256）は `BCryptHash`。`unsafe` はこの 2 呼び出しに
//! 閉じ、各所に `// SAFETY:` を書く（`ShellExecuteW` は `windows_compose_mail_sms::open_url` を共用）。
//!
//! # 取り消し
//!
//! `cancel` は何もしない（ブラウザのタブは閉じられない）。取り消し・期限・終了は
//! `bridge::conclude_with` がサーバーを止めるだけで済む。タブは残るが、そこからの遷移は
//! listener が閉じているので届かない（守らない範囲）。
//!
//! # 既知の天井
//!
//! - `ShellExecuteW` の `> 32` は「OS が受理した」だけで、ブラウザが開いたかは分からない
//! - 関連付けが無くても「アプリを選ぶ」が出て `> 32` になることがある
//! - `127.0.0.1` への bind がファイアウォールの確認を出さないかは実機で確認する（#528）

use std::sync::Arc;

use fandhe_device::url_scheme::MAX_URL_BYTES;
use fandhe_device::web_auth_session::{AuthRequest, RedirectKind, WebAuthFailure, WebAuthSession};

use super::windows_compose_mail_sms::{open_url, SE_ERR_NOASSOC};

/// `ShellExecuteW` の戻り値 → 結果。`> 32` は成功、関連付けなしは `NoBrowser`、他は `Internal`。
pub(crate) fn start_result(code: isize) -> Result<(), WebAuthFailure> {
    match code {
        c if c > 32 => Ok(()),
        SE_ERR_NOASSOC => Err(WebAuthFailure::NoBrowser),
        _ => Err(WebAuthFailure::Internal),
    }
}

/// `ShellExecuteW` に渡す前の URL 検査。実行ファイル・ローカルパス・任意 scheme を開かせない（A03）。
pub(crate) fn check_start_url(url: &str) -> Result<(), WebAuthFailure> {
    let ok = !url.is_empty()
        && url.len() <= MAX_URL_BYTES
        && !url.chars().any(char::is_control)
        && (url.starts_with("http://127.0.0.1:") || url.starts_with("https://"));
    if ok {
        Ok(())
    } else {
        Err(WebAuthFailure::Internal)
    }
}

struct WindowsWebAuthSession;

impl WebAuthSession for WindowsWebAuthSession {
    fn redirect_kind(&self) -> RedirectKind {
        RedirectKind::Loopback
    }

    #[allow(unsafe_code)]
    fn fill_random(&self, buf: &mut [u8]) -> Result<(), WebAuthFailure> {
        use windows::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        // SAFETY: `buf` は有効な可変スライスで、OS は呼び出しの間だけ書き、保持しない。
        let status = unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        if status.is_ok() {
            Ok(())
        } else {
            Err(WebAuthFailure::Internal)
        }
    }

    #[allow(unsafe_code)]
    fn sha256(&self, data: &[u8]) -> Result<[u8; 32], WebAuthFailure> {
        use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
        let mut out = [0u8; 32];
        // SAFETY: スライスは有効で、OS は呼び出しの間だけ読み書きし保持しない。
        // `BCRYPT_SHA256_ALG_HANDLE` は疑似ハンドルで解放不要。
        let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
        if status.is_ok() {
            Ok(out)
        } else {
            Err(WebAuthFailure::Internal)
        }
    }

    fn start(&self, _request_id: u64, req: &AuthRequest) -> Result<(), WebAuthFailure> {
        if req.callback_scheme().is_some() {
            return Err(WebAuthFailure::Internal);
        }
        check_start_url(&req.start_url)?;
        // UI スレッド（effect）から同期に呼ぶ。
        start_result(open_url(&req.start_url))
    }

    fn cancel(&self, _request_id: u64) {}
}

/// Windows の `WebAuthSession` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn WebAuthSession> {
    Arc::new(WindowsWebAuthSession)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_result_table() {
        assert!(start_result(33).is_ok());
        assert_eq!(start_result(31), Err(WebAuthFailure::NoBrowser));
        for c in [0, 2, 5, 32] {
            assert_eq!(start_result(c), Err(WebAuthFailure::Internal));
        }
    }

    #[test]
    fn check_start_url_rejects_unsafe() {
        let long = format!("https://{}", "a".repeat(MAX_URL_BYTES));
        for u in [
            "",
            "https://a\0b",
            "https://a\nb",
            long.as_str(),
            "file:///c:/x.exe",
            "C:\\x.exe",
            "http://evil/",
        ] {
            assert!(check_start_url(u).is_err(), "{u:?}");
        }
        assert!(check_start_url("http://127.0.0.1:5000/authorize?a=b").is_ok());
        assert!(check_start_url("https://example.com/").is_ok());
    }

    /// RFC 7636 付録 B のベクタを実際の BCrypt で検証する。
    #[test]
    fn real_sha256_matches_rfc7636_appendix_b() {
        use fandhe_device::web_auth_session::Pkce;
        let verifier = [
            116u8, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212,
            37, 77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
        ];
        let pkce = Pkce::from_random(&verifier, |d| WindowsWebAuthSession.sha256(d)).unwrap();
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_differs() {
        let (mut a, mut b) = ([0u8; 16], [0u8; 16]);
        WindowsWebAuthSession.fill_random(&mut a).unwrap();
        WindowsWebAuthSession.fill_random(&mut b).unwrap();
        assert_ne!(a, b);
    }
}
