//! iOS / macOS 向け生体認証実装（Issue #906 macOS・#908 iOS。`LAContext` は macOS と
//! 同一 API のため、iOS 側は提示 VC を要求せず同じ経路で完了する）。
//! `device/biometric_auth.rs` の macOS/iOS 経路から呼ばれ、`LocalAuthentication`
//! （`LAContext`）実装（`apple/fandhe_apple_biometric.m`、`build.rs` が iOS 全体・
//! macOS の feature `device` 有効時のみコンパイル・リンクする）を `unsafe extern "C"`
//! 1 関数（[`present`]）で呼ぶ。`unsafe` はこの呼び出し 1 箇所に閉じ `// SAFETY:` で
//! 契約を明記する（`apple_share.rs::present` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の reply コールバックから [`fandhe_rust_biometric_outcome`]（本モジュールが
//! 公開する `extern "C"` 関数）が呼ばれ、`device/biometric_auth::bridge` へ結果を push
//! する（pull 型、`apple_share.rs::fandhe_rust_share_outcome` と同じ橋渡し方式）。
//! `LAError` の生コード → 結果の写像は `device/biometric_auth.rs::outcome_from_reply`
//! （iOS/macOS 共通の SSOT）に一本化する。
//!
//! 認証結果以外の情報（生体データ・識別子）をログへ出力しない（security.md A09）。

use std::ffi::CString;
use std::os::raw::c_char;

// SAFETY: `apple/fandhe_apple_biometric.m` が定義する C ABI（`build.rs` が iOS 全体・
// macOS ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m`
// ファイル冒頭コメントおよび下記 `present` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_biometric_authenticate(request_id: u64, reason: *const c_char) -> i32;
}

/// `.m` 側の `fandhe_apple_biometric_authenticate` を呼び、生体認証 UI の提示を依頼する
/// （`device/biometric_auth.rs` の macOS/iOS 経路から呼ばれる契約）。`request_id` は
/// `device/biometric_auth::bridge::begin` が払い出したプロセス内一意な ID（画面の
/// `generation` ではない、`apple_share.rs::present` と同じ設計）。
///
/// `reason` の `CString` 化が失敗する（内部 NUL 混入）ことは通常起きない
/// （`fandhe_device::biometric_auth::validate_reason` が事前に NUL を弾く契約のため）。
/// 万一失敗した場合は提示自体を行わず `false` を返し、呼び出し元が
/// `DeviceError::Internal` として `bridge::deliver` する。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, reason: &str) -> bool {
    let Ok(reason) = CString::new(reason) else {
        return false;
    };

    // SAFETY: `reason` は上で構築した `CString` の生存期間内でのみ参照される
    // （呼び出しが返るまで `reason` を drop しない）。`.m` 側は返る前に文字列を
    // `NSString` へコピーする契約（`.m` ファイル冒頭コメント参照）ため、ポインタの
    // 所有権は移らない。`validate_reason` が内部 NUL 無しを保証しており、
    // `CString::new` が成功した時点で NUL 終端 UTF-8 として読み書きが対称になる。
    let code = unsafe { fandhe_apple_biometric_authenticate(request_id, reason.as_ptr()) };
    code == 0
}

/// `.m` 側 reply コールバックから届く結果（`success`/`la_code`）の解釈
/// （`device/biometric_auth.rs::outcome_from_reply`、iOS/macOS 共通の表）へ委譲する。
/// `.m` 側の `LAContext.evaluatePolicy` reply ブロックから呼ばれる `extern "C"` 関数。
/// 結果を [`super::biometric_auth::bridge`] へ push する（pull 型、
/// `apple_share.rs::fandhe_rust_share_outcome` と同じ橋渡し方式）。
///
/// iOS では reply コールバックが egui の `update` 外（`LAContext` の private queue）
/// から届くため、[`super::ios::request_repaint`] で明示的に次フレームを起こす
/// （`fandhe_rust_share_outcome` と同じ理由。macOS は event loop が別途 repaint を
/// 要求する経路を持つためここでは呼ばない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_biometric_outcome(request_id: u64, success: i32, la_code: i64) {
    super::biometric_auth::bridge::deliver(
        request_id,
        super::biometric_auth::outcome_from_reply(success != 0, la_code),
    );
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
