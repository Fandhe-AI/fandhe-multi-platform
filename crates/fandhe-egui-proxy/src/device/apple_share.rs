//! iOS / macOS 向け共有シート実装（Issue #897 macOS → #899 iOS 横展開）。
//! `device/share.rs` の macOS/iOS 経路から呼ばれ、AppKit（`NSSharingServicePicker`）/
//! UIKit（`UIActivityViewController`）実装（`apple/fandhe_apple_share.m`、`build.rs` が
//! iOS 全体・macOS の feature `device` 有効時のみコンパイル・リンクする）を
//! `unsafe extern "C"` 1 関数（[`present`]）で呼ぶ。`unsafe` はこの呼び出し 1 箇所に閉じ
//! `// SAFETY:` で契約を明記する（`apple_location.rs` の `start` と同型、`lib.rs` の
//! iOS/macOS `deny(unsafe_code)` 緩和・coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の delegate コールバックから [`fandhe_rust_share_outcome`]（本モジュールが
//! 公開する `extern "C"` 関数）が呼ばれ、`device/share::bridge` へ結果を push する
//! （pull 型、`device/file_pick.rs::bridge` と同じ橋渡し方式）。結果コード → 意味の
//! 対応は `device/share.rs::outcome_from_code`（iOS/macOS/Android 共通、Issue #899
//! §2.2 の表）に一本化する。
//!
//! 共有テキスト・URL をログへ出力しない（security.md A09）。

use std::ffi::CString;
use std::os::raw::c_char;

use fandhe_device::share::ShareRequest;

// SAFETY: `apple/fandhe_apple_share.m` が定義する C ABI（`build.rs` が iOS 全体・macOS
// ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` ファイル
// 冒頭コメントおよび下記 `present` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_share_present(request_id: u64, text: *const c_char, url: *const c_char) -> i32;
}

/// `.m` 側の `fandhe_apple_share_present` を呼び、共有シートの提示を依頼する
/// （`device/share.rs` の macOS 経路から呼ばれる契約）。`request_id` は
/// `device/share::bridge::begin` が払い出したプロセス内一意な ID
/// （画面の `generation` ではない、`file_pick::bridge` と同じ設計）。
///
/// `text`/`url` の `CString` 化が失敗する（内部 NUL 混入）ことは通常起きない
/// （[`ShareRequest::new`] が事前に NUL を弾く契約のため）。万一失敗した場合は
/// 提示自体を行わず `false` を返し、呼び出し元が [`DeviceError::Internal`] として
/// `bridge::deliver` する。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, request: &ShareRequest) -> bool {
    let Ok(text) = CString::new(request.text()) else {
        return false;
    };
    let url = match request.url() {
        Some(u) => match CString::new(u) {
            Ok(c) => Some(c),
            Err(_) => return false,
        },
        None => None,
    };
    let url_ptr = url.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());

    // SAFETY: `text`/`url_ptr` は上で構築した `CString` の生存期間内でのみ参照される
    // （呼び出しが返るまで `text`/`url` を drop しない）。`.m` 側は返る前に文字列を
    // `NSString` へコピーする契約（`.m` ファイル冒頭コメント参照）ため、ポインタの
    // 所有権は移らない。`text` は `ShareRequest::new` が内部 NUL 無しを保証しており、
    // `CString::new` が成功した時点で NUL 終端 UTF-8 として読み書きが対称になる。
    let code = unsafe { fandhe_apple_share_present(request_id, text.as_ptr(), url_ptr) };
    code == 0
}

/// `.m` 側 delegate から届く結果コードの解釈（`device/share.rs::outcome_from_code`、
/// iOS/macOS/Android 共通の表）へ委譲する。`.m` 側の `FandheShareDelegate`
/// （macOS: `sharingServicePicker:didChooseSharingService:`、iOS:
/// `completionWithItemsHandler`）から呼ばれる `extern "C"` 関数。結果を
/// [`super::share::bridge`] へ push する（pull 型、
/// `apple_location.rs::fandhe_rust_location_fix` と同じ橋渡し方式）。
///
/// iOS では delegate コールバックが egui の `update` 外（UIKit の completion）から
/// 届くため、[`super::ios::request_repaint`] で明示的に次フレームを起こす
/// （`fandhe_rust_file_picker_outcome` と同じ理由。macOS/Android は各々の event loop が
/// 別途 repaint を要求する経路を持つためここでは呼ばない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_share_outcome(request_id: u64, code: i32) {
    super::share::bridge::deliver(request_id, super::share::outcome_from_code(code));
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
