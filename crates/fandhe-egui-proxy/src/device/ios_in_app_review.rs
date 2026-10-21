//! iOS 向けアプリ内レビュー依頼実装（Issue #564、b-ios）。`ios/fandhe_ios_in_app_review.m`
//! （前面の `UIWindowScene` を `SKStoreReviewController requestReviewInScene:` へ渡す、`build.rs` が
//! iOS 分岐の feature `device` 有効時にコンパイルし StoreKit をリンクする）を `unsafe extern "C"` 1 関数で
//! 呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/in_app_review.rs` の
//! iOS 経路から呼ばれ、契約は `fandhe_device::in_app_review`。結果コードの写像 [`outcome_from_code`] は
//! ホストでもテストできるようモジュールは `cfg(any(ios, test))`。
//!
//! # 既知の天井
//!
//! - 表示・投稿の有無は返らない（成功は `Requested`＝OS へ渡した）。TestFlight では出ず、本番は 365 日に 3 回まで
//! - 前面の scene が無い（背景など）ときは `keyWindow` へ逃がさず `Internal`
//! - この API に completion は無い。結果は UI スレッドの `CompletionSender::send` が返し、そこで
//!   repaint も起こすため、ここから `crate::ios::request_repaint` は呼ばない

use fandhe_device::in_app_review::ReviewOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 依頼を渡した、それ以外（main thread 外・前面 scene なし・未知）は `Internal`。
pub(crate) fn outcome_from_code(code: i32) -> Result<ReviewOutcome, DeviceError> {
    match code {
        0 => Ok(ReviewOutcome::Requested),
        _ => Err(DeviceError::Internal("in_app_review request")),
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    // SAFETY: `ios/fandhe_ios_in_app_review.m`（`build.rs` が iOS 分岐の feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
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
impl fandhe_device::in_app_review::InAppReview for IosInAppReview {
    fn request(&self) -> Result<ReviewOutcome, DeviceError> {
        outcome_from_code(ffi::request())
    }
}

/// iOS の `InAppReview` 実装を返す（`device/in_app_review.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::in_app_review::InAppReview> {
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
                Err(DeviceError::Internal("in_app_review request"))
            ));
        }
    }
}
