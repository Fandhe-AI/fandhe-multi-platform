//! iOS 向けローカル通知実装（Issue #1005）。`device/notification.rs` の iOS 経路
//! （`bridge`/`drive_callback`）から呼ばれ、`UserNotifications`（`UNUserNotificationCenter`）
//! 実装（`ios/fandhe_ios_notification.m`、`build.rs` が feature `device` 有効時のみ
//! コンパイル・リンクする）を `unsafe extern "C"` 1 関数（[`present`]）で呼ぶ。
//! `unsafe` はこの呼び出し 1 箇所に閉じ `// SAFETY:` で契約を明記する
//! （`apple_biometric.rs::present` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の completion から [`fandhe_rust_notification_outcome`]（本モジュールが
//! 公開する `extern "C"` 関数）が呼ばれ、`device/notification::bridge` へ結果を
//! push する（pull 型、`apple_biometric.rs::fandhe_rust_biometric_outcome` と
//! 同じ橋渡し方式）。OS のコード表 → 結果の写像は
//! `device/notification.rs::outcome_from_code`（SSOT）に一本化する。
//!
//! 通知内容（タイトル・本文）・エラーの詳細をログへ出力しない（security.md A09）。

use std::ffi::CString;
use std::os::raw::c_char;

// SAFETY: `ios/fandhe_ios_notification.m` が定義する C ABI（`build.rs` が feature
// `device` 有効時のみコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントおよび
// 下記 `present` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_notification_post(
        request_id: u64,
        title: *const c_char,
        body: *const c_char,
    ) -> i32;
}

/// `.m` 側の `fandhe_ios_notification_post` を呼び、通知の提示を依頼する
/// （`device/notification.rs` の iOS 経路から呼ばれる契約）。`request_id` は
/// `device/notification::bridge::begin` が払い出したプロセス内一意な ID
/// （画面の `generation` ではない、`apple_biometric.rs::present` と同じ設計）。
///
/// `title`/`body` の `CString` 化が失敗する（内部 NUL 混入）ことは通常起きない
/// （`fandhe_device::notification::NotificationRequest::new` が事前に NUL を
/// 弾く契約のため）。万一失敗した場合は提示自体を行わず `false` を返し、
/// 呼び出し元が `DeviceError::Internal` として `bridge::deliver` する。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, title: &str, body: &str) -> bool {
    let (Ok(title), Ok(body)) = (CString::new(title), CString::new(body)) else {
        return false;
    };

    // SAFETY: `title`/`body` は上で構築した `CString` の生存期間内でのみ参照
    // される（呼び出しが返るまで drop しない）。`.m` 側は返る前に両文字列を
    // `NSString` へコピーする契約（`.m` ファイル冒頭コメント参照）ため、
    // ポインタの所有権は移らない。`NotificationRequest::new` が内部 NUL 無しを
    // 保証しており、`CString::new` が成功した時点で NUL 終端 UTF-8 として
    // 読み書きが対称になる。
    let code = unsafe { fandhe_ios_notification_post(request_id, title.as_ptr(), body.as_ptr()) };
    code == 0
}

/// `.m` 側 completion から届く結果コードの解釈（`device/notification.rs::
/// outcome_from_code`、iOS 専用の写像表）へ委譲する。`.m` 側の各 completion
/// ブロックから呼ばれる `extern "C"` 関数。結果を [`super::notification::bridge`]
/// へ push する（pull 型、`fandhe_rust_biometric_outcome` と同じ橋渡し方式）。
///
/// `.m` 側は completion を常に main queue 上で呼ぶ契約（`.m` ファイル冒頭
/// コメント参照）だが、egui の `update` 呼び出し自体とは非同期のため
/// [`super::ios::request_repaint`] で明示的に次フレームを起こす
/// （`fandhe_rust_biometric_outcome` と同じ理由）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_notification_outcome(request_id: u64, code: i32) {
    super::notification::bridge::deliver(request_id, super::notification::outcome_from_code(code));
    crate::ios::request_repaint();
}
