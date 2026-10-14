//! macOS / iOS 向け連絡先選択実装（Issue #1220・#1221）。`device/contact_pick.rs` の Apple 経路から呼ばれ、
//! `apple/fandhe_apple_contact_picker.m`（macOS は `CNContactPicker`、iOS は
//! `CNContactPickerViewController` で権限確認なし。
//! `build.rs` が feature `device` 有効時のみコンパイル・リンクする）を
//! `unsafe extern "C"` 1 関数（[`present`]）で呼ぶ。
//! `unsafe` はこの呼び出しと受け口に閉じ `// SAFETY:` で契約を明記する
//! （`apple_biometric.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側のデリゲートから [`fandhe_rust_contact_picker_outcome`] が呼ばれ、
//! `device/contact_pick::bridge` へ結果を push する（pull 型）。コード → 結果の写像は
//! `device/contact_pick.rs::outcome_from_code` に一本化する。
//!
//! 名前（個人情報）をログへ出力しない（security.md A09）。

use std::ffi::CStr;
use std::os::raw::c_char;

// SAFETY: `apple/fandhe_apple_contact_picker.m` が定義する C ABI。契約は `.m` ファイル
// 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_contact_picker_present(request_id: u64) -> i32;
}

/// ピッカーの提示を依頼する。`request_id` は `bridge::begin` が払い出した ID。
/// 戻り値は「提示依頼を受理できたか」（`false` は呼び出し元が `Internal` として畳む）。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64) -> bool {
    // SAFETY: 引数は値渡しの `u64` のみでポインタを渡さない。`.m` 側は main queue へ
    // dispatch してから即座に 0 を返す契約。
    let code = unsafe { fandhe_apple_contact_picker_present(request_id) };
    code == 0
}

/// `.m` 側のデリゲートから呼ばれる受け口。`name` は code 0 のときだけ非 NULL で、
/// 呼び出しの間だけ有効なため、ここでコピーして保持しない。NULL・不正な UTF-8 は
/// `Internal` にする（成功に倒さない）。
///
/// iOS では delegate コールバックが egui の `update` 外（UIKit）から届くため、
/// [`crate::ios::request_repaint`] で次フレームを起こす（`fandhe_rust_share_outcome` と同じ理由。
/// macOS は event loop 側が別途 repaint を要求するため呼ばない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_contact_picker_outcome(request_id: u64, code: i32, name: *const c_char) {
    let name = if name.is_null() {
        None
    } else {
        // SAFETY: `.m` 側が NUL 終端の有効な C 文字列を渡し、この呼び出しが返るまで
        // 解放しない契約。ここで `String` へコピーし、ポインタは保持しない。
        unsafe { CStr::from_ptr(name) }
            .to_str()
            .ok()
            .map(str::to_owned)
    };
    super::contact_pick::bridge::deliver(
        request_id,
        super::contact_pick::outcome_from_code(code, name),
    );
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
