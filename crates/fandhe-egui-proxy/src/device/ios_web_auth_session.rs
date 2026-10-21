//! iOS 向け Web 認証セッション実装（Issue #532、親 #113）。`device/web_auth_session.rs` の
//! iOS 経路から呼ばれ、`ios/fandhe_ios_web_auth_session.m`（`ASWebAuthenticationSession`、
//! `build.rs` が feature `device` 有効時にコンパイルして AuthenticationServices をリンクする）を
//! `unsafe extern "C"` で呼ぶ。`unsafe` はこの FFI 宣言と受け口に閉じ、各所に `// SAFETY:` を書く
//! （coding-rust.md の FFI 例外規定）。契約の SSOT は `fandhe_device::web_auth_session`。
//!
//! 期限（300 秒）・取り消し・state 検査は effect ドライバの `bridge` が担い、ここは開く・閉じる・
//! 乱数・SHA-256・完了の受け渡しだけをする。契約表との対応:
//! - 背景遷移では取り消さず、期限に任せる（1）。完了は `request_id` を照合して古いものを捨てる（2）。
//! - 完了は main queue・`@autoreleasepool` の内側で 1 回だけ。受け口は結果を置いた直後に
//!   `crate::ios::request_repaint` で次フレームを起こす（4・5）。
//! - 成功・キャンセル・失敗の三値は `outcome_from_as_error` で写す。コード 1 だけが取り消し（6・10）。
//! - 完了・取り消し・開始失敗のどの経路でもセッションと表示先を手放す（9）。
//!
//! 守らない範囲: 背景遷移中にデモサーバーが止まり認可ページが読めないこと・システムブラウザの
//! UI の差・複数プロセス・プロセス死後のコールバック・トークン交換。URL・エラー文はログに出さない。

use std::ffi::{c_char, CStr, CString};
use std::sync::Arc;

use fandhe_device::web_auth_session::{
    outcome_from_as_error, AuthRequest, RedirectKind, WebAuthFailure, WebAuthOutcome,
    WebAuthSession,
};

use crate::device::web_auth_session::bridge::{self, RawResult};

// SAFETY: `ios/fandhe_ios_web_auth_session.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_web_auth_start(
        request_id: u64,
        url: *const c_char,
        callback_scheme: *const c_char,
    ) -> i32;
    fn fandhe_ios_web_auth_cancel(request_id: u64);
    fn fandhe_ios_web_auth_random(buf: *mut u8, len: usize) -> i32;
    fn fandhe_ios_web_auth_sha256(data: *const u8, len: usize, out: *mut u8);
}

/// 開始コードを失敗へ写す（macOS の `start_result` と同じ写像。そちらは iOS ではコンパイルされない）。
fn start_result(code: i32) -> Result<(), WebAuthFailure> {
    match code {
        0 => Ok(()),
        1 => Err(WebAuthFailure::NoBrowser),
        _ => Err(WebAuthFailure::Internal),
    }
}

struct IosWebAuthSession;

impl WebAuthSession for IosWebAuthSession {
    fn redirect_kind(&self) -> RedirectKind {
        RedirectKind::CustomScheme
    }

    #[allow(unsafe_code)]
    fn fill_random(&self, buf: &mut [u8]) -> Result<(), WebAuthFailure> {
        // SAFETY: `buf` は有効な可変スライスで、`.m` は `len` バイトだけ書く（`arc4random_buf`）。
        let code = unsafe { fandhe_ios_web_auth_random(buf.as_mut_ptr(), buf.len()) };
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
        unsafe { fandhe_ios_web_auth_sha256(data.as_ptr(), data.len(), out.as_mut_ptr()) };
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
        let code = unsafe { fandhe_ios_web_auth_start(request_id, url.as_ptr(), scheme.as_ptr()) };
        start_result(code)
    }

    #[allow(unsafe_code)]
    fn cancel(&self, request_id: u64) {
        // SAFETY: 値渡しの `u64` のみ。`.m` は main 以外なら main queue へ寄せる契約。
        unsafe { fandhe_ios_web_auth_cancel(request_id) };
    }
}

/// iOS の `WebAuthSession` 実装を返す。`device/web_auth_session.rs::implementation` の iOS 経路から呼ばれる。
pub(crate) fn implementation() -> Arc<dyn WebAuthSession> {
    Arc::new(IosWebAuthSession)
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
    // 入力がないと egui が描画せず画面が処理中のまま止まるため、次フレームを起こす。
    crate::ios::request_repaint();
}
