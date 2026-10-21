//! macOS 向けアプリ内レビュー依頼実装（Issue #563）。`macos/fandhe_macos_in_app_review.m`
//! （`SKStoreReviewController requestReview`、`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルし StoreKit をリンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに
//! 閉じ `// SAFETY:` で契約を明記する。`device/in_app_review.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::in_app_review`。結果コードの写像 [`outcome_from_code`] はホストでもテストできるよう
//! モジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 未バンドル実行（`cargo run`）で表示されるかは未確認（Mac 目視は #556）
//! - 表示は確かめられないため成功は `Requested`（OS へ渡した）に留める

use fandhe_device::in_app_review::ReviewOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 依頼を渡した、それ以外（main thread 外・未知）は `Internal`。
pub(crate) fn outcome_from_code(code: i32) -> Result<ReviewOutcome, DeviceError> {
    match code {
        0 => Ok(ReviewOutcome::Requested),
        _ => Err(DeviceError::Internal("in_app_review request")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_in_app_review.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_in_app_review_request() -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn request() -> i32 {
        // SAFETY: 引数なしで、`.m` はポインタを受け取らず保持しない。
        unsafe { fandhe_macos_in_app_review_request() }
    }
}

#[cfg(target_os = "macos")]
struct MacInAppReview;

#[cfg(target_os = "macos")]
impl fandhe_device::in_app_review::InAppReview for MacInAppReview {
    fn request(&self) -> Result<ReviewOutcome, DeviceError> {
        outcome_from_code(ffi::request())
    }
}

/// macOS の `InAppReview` 実装を返す（`device/in_app_review.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::in_app_review::InAppReview> {
    Box::new(MacInAppReview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert!(matches!(outcome_from_code(0), Ok(ReviewOutcome::Requested)));
        for code in [1, 2, 3, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(code),
                Err(DeviceError::Internal("in_app_review request"))
            ));
        }
    }
}
