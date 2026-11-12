//! macOS 向けアプリ内レビュー依頼実装（Issue #563）。`macos/fandhe_macos_in_app_review.m`
//! （`SKStoreReviewController requestReview`、`build.rs` が macOS ターゲットで
//! コンパイルし StoreKit をリンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに
//! 閉じ `// SAFETY:` で契約を明記する。`driver` の macOS 経路から呼ばれ、契約は
//! `contract`。結果コードの写像 [`outcome_from_code`] はホストでもテストできるよう
//! モジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 未バンドル実行（`cargo run`）で表示されるかは未確認（Mac 目視は #556）
//! - 表示は確かめられないため成功は `Requested`（OS へ渡した）に留める

use crate::contract::ReviewOutcome;
use crate::error::InAppReviewError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 依頼を渡した、それ以外（main thread 外・未知）は `Internal`。
pub(crate) fn outcome_from_code(code: i32) -> Result<ReviewOutcome, InAppReviewError> {
    match code {
        0 => Ok(ReviewOutcome::Requested),
        _ => Err(InAppReviewError::Internal),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_in_app_review.m`（`build.rs` が macOS ターゲットで
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    unsafe extern "C" {
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
impl crate::contract::InAppReview for MacInAppReview {
    fn request(&self) -> Result<ReviewOutcome, InAppReviewError> {
        outcome_from_code(ffi::request())
    }
}

/// macOS の `InAppReview` 実装を返す（`driver` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn crate::contract::InAppReview> {
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
                Err(InAppReviewError::Internal)
            ));
        }
    }
}
