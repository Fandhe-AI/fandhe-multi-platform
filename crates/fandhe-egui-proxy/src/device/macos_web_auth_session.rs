//! macOS 向け Web 認証セッション実装（Issue #530、親 #113）。`device/web_auth_session.rs` の
//! macOS 経路から呼ばれ、`macos/fandhe_macos_web_auth_session.m`（`ASWebAuthenticationSession`、
//! `build.rs` が feature `device` 有効時にコンパイルして AuthenticationServices をリンクする）を
//! `unsafe extern "C"` で呼ぶ。`unsafe` はこの FFI 宣言と受け口に閉じ、各所に `// SAFETY:` を書く
//! （coding-rust.md の FFI 例外規定）。契約の SSOT は `fandhe_device::web_auth_session`。
//!
//! 完了は `.m` の完了ハンドラから [`fandhe_rust_web_auth_session_complete`] が呼ばれ、
//! `web_auth_session::bridge::complete_from_os` へ渡す。URL・エラー文はログに出さない。

use fandhe_device::web_auth_session::WebAuthFailure;

/// `fandhe_macos_web_auth_start` の戻りコードを失敗へ写す（0 は開始成功）。
/// 1=表示先なし・開始不可は `NoBrowser`、2・未知は `Internal`（コードを成功へ倒さない）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn start_result(code: i32) -> Result<(), WebAuthFailure> {
    match code {
        0 => Ok(()),
        1 => Err(WebAuthFailure::NoBrowser),
        _ => Err(WebAuthFailure::Internal),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::{c_char, CStr, CString};
    use std::sync::Arc;

    use fandhe_device::web_auth_session::{
        outcome_from_as_error, AuthRequest, RedirectKind, WebAuthFailure, WebAuthOutcome,
        WebAuthSession,
    };

    use crate::device::web_auth_session::bridge::{self, RawResult};

    // SAFETY: `macos/fandhe_macos_web_auth_session.m` が定義する C ABI。契約は `.m` ファイル
    // 冒頭コメントに集約する。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_macos_web_auth_start(
            request_id: u64,
            url: *const c_char,
            callback_scheme: *const c_char,
        ) -> i32;
        fn fandhe_macos_web_auth_cancel(request_id: u64);
        fn fandhe_macos_web_auth_random(buf: *mut u8, len: usize) -> i32;
        fn fandhe_macos_web_auth_sha256(data: *const u8, len: usize, out: *mut u8);
    }

    struct MacosWebAuthSession;

    impl WebAuthSession for MacosWebAuthSession {
        fn redirect_kind(&self) -> RedirectKind {
            RedirectKind::CustomScheme
        }

        #[allow(unsafe_code)]
        fn fill_random(&self, buf: &mut [u8]) -> Result<(), WebAuthFailure> {
            // SAFETY: `buf` は有効な可変スライスで、`.m` は `len` バイトだけ書く（`arc4random_buf`）。
            let code = unsafe { fandhe_macos_web_auth_random(buf.as_mut_ptr(), buf.len()) };
            if code == 0 {
                Ok(())
            } else {
                Err(WebAuthFailure::Internal)
            }
        }

        #[allow(unsafe_code)]
        fn sha256(&self, data: &[u8]) -> Result<[u8; 32], WebAuthFailure> {
            let mut out = [0u8; 32];
            // SAFETY: `data` は有効なスライス、`out` は 32 バイトの書き込み先。`.m` は
            // 呼び出しの間だけ読み書きし、ポインタを保持しない（`CC_SHA256`）。
            unsafe { fandhe_macos_web_auth_sha256(data.as_ptr(), data.len(), out.as_mut_ptr()) };
            Ok(out)
        }

        #[allow(unsafe_code)]
        fn start(&self, request_id: u64, req: &AuthRequest) -> Result<(), WebAuthFailure> {
            let scheme = req.callback_scheme().ok_or(WebAuthFailure::Internal)?;
            // NUL を含む文字列は `CString` を作れず、ここで弾く。
            let url = CString::new(req.start_url.as_str()).map_err(|_| WebAuthFailure::Internal)?;
            let scheme = CString::new(scheme).map_err(|_| WebAuthFailure::Internal)?;
            // SAFETY: 2 つのポインタは NUL 終端の有効な C 文字列で、呼び出しの間だけ有効。
            // `.m` は main thread 以外から呼ばれたら 2 を返す契約（effect ドライバは update ＝ main で呼ぶ）。
            let code =
                unsafe { fandhe_macos_web_auth_start(request_id, url.as_ptr(), scheme.as_ptr()) };
            super::start_result(code)
        }

        #[allow(unsafe_code)]
        fn cancel(&self, request_id: u64) {
            // SAFETY: 値渡しの `u64` のみ。`.m` は main 以外なら main queue へ寄せる契約。
            unsafe { fandhe_macos_web_auth_cancel(request_id) };
        }
    }

    pub(crate) fn implementation() -> Arc<dyn WebAuthSession> {
        Arc::new(MacosWebAuthSession)
    }

    /// `.m` の完了ハンドラから呼ばれる受け口。`url` は kind 0 のときだけ非 NULL で、呼び出しの
    /// 間だけ有効なためここでコピーする。NULL・不正な UTF-8 は `Internal`（成功に倒さない）。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_web_auth_session_complete(
        request_id: u64,
        kind: i32,
        as_code: i64,
        url: *const c_char,
    ) {
        let result = match kind {
            0 if !url.is_null() => {
                // SAFETY: `.m` が NUL 終端の有効な C 文字列を渡し、この呼び出しが返るまで解放しない。
                // ここで `String` へコピーし、ポインタは保持しない。
                unsafe { CStr::from_ptr(url) }
                    .to_str()
                    .map(|s| RawResult::Url(s.to_owned()))
                    .map_err(|_| WebAuthFailure::Internal)
            }
            1 => match outcome_from_as_error(as_code) {
                Ok(WebAuthOutcome::Cancelled) => Ok(RawResult::Cancelled),
                Ok(_) => Err(WebAuthFailure::Internal),
                Err(f) => Err(f),
            },
            _ => Err(WebAuthFailure::Internal),
        };
        bridge::complete_from_os(request_id, result);
    }
}

#[cfg(target_os = "macos")]
pub(crate) use ffi::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_map_without_false_success() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(WebAuthFailure::NoBrowser));
        for c in [2, 3, -1, 99] {
            assert_eq!(start_result(c), Err(WebAuthFailure::Internal));
        }
    }

    /// RFC 7636 付録 B のベクタを実際の CommonCrypto で検証する。
    #[cfg(target_os = "macos")]
    #[test]
    fn real_sha256_matches_rfc7636_appendix_b() {
        use fandhe_device::web_auth_session::Pkce;
        let bytes: [u8; 32] =
            fandhe_device::passkey::base64url_decode("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
                .unwrap()
                .try_into()
                .unwrap();
        let imp = ffi::implementation();
        let p = Pkce::from_random(&bytes, |d| imp.sha256(d)).unwrap();
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        imp.fill_random(&mut a).unwrap();
        imp.fill_random(&mut b).unwrap();
        assert_ne!(a, b);
    }
}
