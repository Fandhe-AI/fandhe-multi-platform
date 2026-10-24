//! iOS 向けアプリ内ブラウザ実装（親 #135、Issue #702）。`device/in_app_browser.rs` の iOS 経路
//! （dispatch）から呼ばれ、`ios/fandhe_ios_in_app_browser.m`（`SFSafariViewController`。`build.rs` が
//! feature `device` 有効時にコンパイルして SafariServices をリンクする）を `unsafe extern "C"` で呼ぶ。
//! `unsafe` はこの FFI 宣言と受け口に閉じ、各所に `// SAFETY:` を書く（coding-rust.md の FFI 例外規定）。
//! 契約の SSOT は `fandhe_device::in_app_browser`。
//!
//! 期限（開くまでの 10 秒）・世代照合・二重完了の破棄は effect ドライバの `bridge` が担い、ここは
//! 開く・「閉じた」を受け取る・完了を渡すだけをする。契約表との対応:
//! - 表示前の背景遷移は `Interrupted`、表示中の背景遷移は OS に任せる（1）。
//! - 表示直前に `try_begin_present` で現在の要求かを確かめ、期限切れ後は表示しない（2・3）。
//! - 完了は main queue・`@autoreleasepool` の内側で 1 回だけ。受け口は結果を渡した直後に
//!   `crate::ios::request_repaint` で次フレームを起こす（4・5）。
//! - 「閉じた」は成功、提示できなかった場合は `Internal` で、成功に畳まない（6・10）。
//! - 完了・失敗のどの経路でも delegate の強参照を手放す（9）。
//! - URL・OS のエラー文はログに出さない（8）。新しい権限・依存は足さない。
//!
//! 守らない範囲: 開いた後に Rust から閉じること・閲覧内容と遷移先と読み込み失敗の検知・表示中の
//! 背景遷移・複数プロセス・プロセス死後の結果。

use std::sync::Arc;

use fandhe_device::in_app_browser::{InAppBrowserFailure, InAppBrowserOutcome};

#[cfg(target_os = "ios")]
use std::ffi::{c_char, CString};

#[cfg(target_os = "ios")]
use fandhe_device::in_app_browser::{BrowserUrl, InAppBrowser};
#[cfg(not(target_os = "ios"))]
use fandhe_device::in_app_browser::{InAppBrowser, UnsupportedInAppBrowser};

// SAFETY: `ios/fandhe_ios_in_app_browser.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_in_app_browser_open(request_id: u64, url: *const c_char) -> i32;
}

type Outcome = Result<InAppBrowserOutcome, InAppBrowserFailure>;

/// 完了コードを結果へ写す（`.m` 冒頭のコード表と一致させる）。0 だけが「閉じた」で、未知は `Internal`。
fn outcome_from_ios_code(code: i32) -> Outcome {
    match code {
        0 => Ok(InAppBrowserOutcome::Closed),
        1 => Err(InAppBrowserFailure::NoPresenter),
        2 => Err(InAppBrowserFailure::Interrupted),
        3 => Err(InAppBrowserFailure::InvalidUrl),
        _ => Err(InAppBrowserFailure::Internal),
    }
}

/// 同期の受理コードを写す。0 = 受理、1 = URL 不正、それ以外は `Internal`。
fn open_result(code: i32) -> Result<(), InAppBrowserFailure> {
    match code {
        0 => Ok(()),
        1 => Err(InAppBrowserFailure::InvalidUrl),
        _ => Err(InAppBrowserFailure::Internal),
    }
}

#[cfg(target_os = "ios")]
struct IosInAppBrowser;

#[cfg(target_os = "ios")]
impl InAppBrowser for IosInAppBrowser {
    #[allow(unsafe_code)]
    fn open(&self, request_id: u64, url: &BrowserUrl) -> Result<(), InAppBrowserFailure> {
        // NUL を含めば作れない。`validate_url` が通さないため通常は起きない。
        let c = CString::new(url.as_str()).map_err(|_| InAppBrowserFailure::InvalidUrl)?;
        // SAFETY: `c` は NUL 終端の有効な C 文字列で、この呼び出しの間だけ生きる。
        // `.m` は return 前に NSURL へコピーし、ポインタを保持しない。
        let code = unsafe { fandhe_ios_in_app_browser_open(request_id, c.as_ptr()) };
        open_result(code)
    }
}

/// iOS 用の実装を返す。ホストのテストビルドでは dispatch が呼ばれないため `Unsupported` を返す。
pub(crate) fn implementation() -> Arc<dyn InAppBrowser> {
    #[cfg(target_os = "ios")]
    {
        Arc::new(IosInAppBrowser)
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(UnsupportedInAppBrowser)
    }
}

/// `.m` が表示の直前に呼ぶ。1 = 表示してよい、0 = 表示しない（期限切れ・古い要求）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_in_app_browser_try_begin_present(request_id: u64) -> i32 {
    i32::from(super::in_app_browser::try_begin_present(request_id))
}

/// `.m` が結果を 1 回だけ渡す受け口。渡した直後に再描画を要求する（入力がないと egui が止まる）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_in_app_browser_complete(request_id: u64, code: i32) {
    super::in_app_browser::complete_from_os(request_id, outcome_from_ios_code(code));
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_maps_to_outcome() {
        assert_eq!(outcome_from_ios_code(0), Ok(InAppBrowserOutcome::Closed));
        assert_eq!(
            outcome_from_ios_code(1),
            Err(InAppBrowserFailure::NoPresenter)
        );
        assert_eq!(
            outcome_from_ios_code(2),
            Err(InAppBrowserFailure::Interrupted)
        );
        assert_eq!(
            outcome_from_ios_code(3),
            Err(InAppBrowserFailure::InvalidUrl)
        );
        assert_eq!(outcome_from_ios_code(4), Err(InAppBrowserFailure::Internal));
    }

    #[test]
    fn unknown_code_is_internal_never_closed() {
        for code in [-1, 5, 99, i32::MIN, i32::MAX] {
            assert_eq!(
                outcome_from_ios_code(code),
                Err(InAppBrowserFailure::Internal)
            );
        }
    }

    #[test]
    fn open_code_maps() {
        assert_eq!(open_result(0), Ok(()));
        assert_eq!(open_result(1), Err(InAppBrowserFailure::InvalidUrl));
        assert_eq!(open_result(2), Err(InAppBrowserFailure::Internal));
        assert_eq!(open_result(-7), Err(InAppBrowserFailure::Internal));
    }
}
