//! macOS / iOS 向け予定追加実装（Issue #1224・#1225）。`device/calendar.rs` の
//! iOS / macOS 経路から呼ばれ、
//! EventKit 実装（`apple/fandhe_apple_calendar.m`、`build.rs` が
//! feature `device` 有効時のみコンパイル・リンクする。iOS は main queue で
//! `EKEventEditViewController` を提示する）を `unsafe extern "C"` 1 関数
//! （[`present`]）で呼ぶ。`unsafe` はこの呼び出し 1 箇所に閉じ `// SAFETY:` で契約を明記する
//! （`apple_biometric.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の完了から [`fandhe_rust_calendar_outcome`] が呼ばれ、`device/calendar::bridge`
//! へ結果を置く（pull 型）。結果コード → 結果の写像は `device/calendar.rs::outcome_from_code`
//! に一本化する。件名・日時はログへ出力しない（security.md A09）。

use std::ffi::CString;
use std::os::raw::c_char;

use fandhe_device::calendar::EventDraft;

// SAFETY: `apple/fandhe_apple_calendar.m` が定義する C ABI。契約は `.m` 冒頭コメントと
// 下記 `present` のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_calendar_add_event(
        request_id: u64,
        title: *const c_char,
        start: *const i32,
        end: *const i32,
    ) -> i32;
}

/// 検査済みの予定を EventKit へ渡す依頼を出す。`false` は依頼自体が受理されなかった
/// （呼び出し元が `Internal` として `bridge::deliver` する）。`request_id` は
/// `bridge::begin` が払い出したプロセス内一意な ID。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, draft: &EventDraft) -> bool {
    // `EventDraft::parse` が NUL を弾いているため通常は失敗しない。
    let Ok(title) = CString::new(draft.title()) else {
        return false;
    };
    let pack = |t: fandhe_device::calendar::LocalDateTime| {
        [
            t.year,
            i32::from(t.month),
            i32::from(t.day),
            i32::from(t.hour),
            i32::from(t.minute),
        ]
    };
    let (start, end) = (pack(draft.start()), pack(draft.end()));

    // SAFETY: `title` は呼び出しが返るまで生存し、`start` / `end` は長さ 5 の配列
    // （年・月・日・時・分）への有効なポインタ。`.m` 側は返る前に 3 つとも
    // 値コピー（`NSString` / `int`）するため、所有権は移らず返却後は参照されない。
    let code = unsafe {
        fandhe_apple_calendar_add_event(request_id, title.as_ptr(), start.as_ptr(), end.as_ptr())
    };
    code == 0
}

/// `.m` 側の完了から呼ばれる `extern "C"` 関数。コードの解釈は
/// `device/calendar.rs::outcome_from_code`。任意スレッドから呼ばれてよい
/// （`bridge` は `Mutex`）。画面への反映は毎フレームの `drive_callback` が行う。
///
/// iOS では EventKitUI の delegate が egui の `update` 外から呼ぶため、結果を置いた後に
/// `ios::request_repaint` で次フレームを起こす（`apple_biometric.rs` と同じ理由。
/// macOS は event loop が別途 repaint を要求するためここでは呼ばない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_calendar_outcome(request_id: u64, code: i32) {
    super::calendar::bridge::deliver(request_id, super::calendar::outcome_from_code(code));
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
