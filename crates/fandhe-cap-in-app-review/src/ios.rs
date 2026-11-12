//! iOS 向けアプリ内レビュー依頼実装（Issue #564、b-ios）。`ios/fandhe_ios_in_app_review.m`
//! （前面の `UIWindowScene` を `SKStoreReviewController requestReviewInScene:` へ渡す、`build.rs` が
//! iOS 分岐の コンパイルし StoreKit をリンクする）を `unsafe extern "C"` 1 関数で
//! 呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`driver` の
//! iOS 経路から呼ばれ、契約は `contract`。結果コードの写像 [`outcome_from_code`] は
//! ホストでもテストできるようモジュールは `cfg(any(ios, test))`。
//!
//! # 既知の天井
//!
//! - 表示・投稿の有無は返らない（成功は `Requested`＝OS へ渡した）。TestFlight では出ず、本番は 365 日に 3 回まで
//! - 前面の scene が無い（背景など）ときは `keyWindow` へ逃がさず `Internal`
//! - この API に completion は無い。結果は UI スレッドの `CompletionSender::send` が返し、そこで
//!   repaint も起こすため、ここから `fandhe_runtime::request_repaint` は呼ばない

use crate::contract::ReviewOutcome;
use crate::error::InAppReviewError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 依頼を渡した、それ以外（main thread 外・前面 scene なし・未知）は `Internal`。
pub(crate) fn outcome_from_code(code: i32) -> Result<ReviewOutcome, InAppReviewError> {
    match code {
        0 => Ok(ReviewOutcome::Requested),
        _ => Err(InAppReviewError::Internal),
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    // SAFETY: `ios/fandhe_ios_in_app_review.m`（`build.rs` がコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_ios_in_app_review_request() -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn request() -> i32 {
        // SAFETY: 引数なしで、`.m` はポインタを受け取らず保持しない。
        unsafe { fandhe_ios_in_app_review_request() }
    }
}

#[cfg(target_os = "ios")]
struct IosInAppReview;

#[cfg(target_os = "ios")]
impl crate::contract::InAppReview for IosInAppReview {
    fn request(&self) -> Result<ReviewOutcome, InAppReviewError> {
        outcome_from_code(ffi::request())
    }
}

/// iOS の `InAppReview` 実装を返す（`driver` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Box<dyn crate::contract::InAppReview> {
    Box::new(IosInAppReview)
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
