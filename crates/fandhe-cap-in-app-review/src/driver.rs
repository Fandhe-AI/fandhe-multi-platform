//! Phase 11 の拡充ループ型サンプル「アプリ内レビュー依頼」（Issue #563）の effect ドライバ。
//! `fandhe_core::screens::in_app_review`（端末依存ゼロ）の要求を観測し、OS のレビュー依頼を 1 回
//! 呼んで結果を `CompletionSender` 経由で返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から呼ばれる同期経路）。契約と守る範囲は `contract` の
//! モジュール doc を参照。
//!
//! review API は completion を持たない同期呼び出しなので全 OS とも UI スレッドで同期に呼び、
//! `RequestSlot` と bridge は持たない。**dispatch はここで最終形**（macOS・iOS 実装、
//! それ以外は `UnsupportedInAppReview`）。Android・Windows は保留で fallback に落ち、
//! 他の OS へ判断を広げない。

use crate::contract::{InAppReview, ReviewOutcome};
use crate::error::InAppReviewError;
use fandhe_core::screens::in_app_review::{
    CoreOutcome, FAILED_STATUS, InAppReviewState, finish_request, mark_requesting, take_request,
};

use fandhe_runtime::CompletionSender;

/// `InAppReviewError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。想定外の種別（`PermissionDenied` 等）も失敗に倒す。
fn to_outcome(result: Result<ReviewOutcome, InAppReviewError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(ReviewOutcome::Requested) => Ok(CoreOutcome::Requested),
        Err(InAppReviewError::Unsupported) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(FAILED_STATUS.to_string()),
    }
}

/// 要求があれば [`InAppReview`] を 1 回呼んで世代付きで結果を返す。
fn drive(
    state: &InAppReviewState,
    sender: &CompletionSender<InAppReviewState>,
    review: &dyn InAppReview,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_requesting));
    let outcome = to_outcome(review.request());
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut InAppReviewState| {
        finish_request(s, generation, outcome);
    }));
}

/// OS 別の [`InAppReview`] 実装を返す。画面の状態型に依存しないため、フレームワークの外のアプリも使える。
/// macOS・iOS 以外は `UnsupportedInAppReview`。
pub fn platform_in_app_review() -> Box<dyn InAppReview> {
    #[cfg(target_os = "macos")]
    return crate::macos::implementation();
    #[cfg(target_os = "ios")]
    return crate::ios::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    return Box::new(crate::contract::UnsupportedInAppReview);
}

/// `registry.rs` から配線される effect ドライバ。
///
/// ponytail: 依頼は OS への 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。戻らない呼び出しが
/// 見つかったら作業スレッド + bridge へ移す。
pub fn in_app_review_effect(state: &InAppReviewState, sender: &CompletionSender<InAppReviewState>) {
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_in_app_review().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedInAppReview;
    use fandhe_core::screens::in_app_review::{
        REQUESTED_STATUS, ReviewPhase, UNSUPPORTED_STATUS, request,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    /// 呼び出し回数を数え、決めた結果を返す偽実装。
    struct Fake {
        calls: AtomicU32,
        result: fn() -> Result<ReviewOutcome, InAppReviewError>,
    }

    impl InAppReview for Fake {
        fn request(&self) -> Result<ReviewOutcome, InAppReviewError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<ReviewOutcome, InAppReviewError>) -> Fake {
        Fake {
            calls: AtomicU32::new(0),
            result,
        }
    }

    /// 1 回押下して 1 回ドライバを回し、反映後の状態を返す。
    fn run(review: &dyn InAppReview) -> InAppReviewState {
        let mut s = InAppReviewState::default();
        request(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive(&s, &sender, review);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        s
    }

    #[test]
    fn no_request_never_calls_os() {
        let f = fake(|| Ok(ReviewOutcome::Requested));
        let s = InAppReviewState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive(&s, &sender, &f);
        assert!(rx.try_recv().is_err());
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn one_press_calls_os_once_and_shows_requested() {
        let f = fake(|| Ok(ReviewOutcome::Requested));
        let s = run(&f);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert_eq!(s.phase, ReviewPhase::Idle);
        assert_eq!(s.status, REQUESTED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn failures_never_become_success() {
        for r in [
            (|| Err(InAppReviewError::Internal)) as fn() -> Result<ReviewOutcome, InAppReviewError>,
            || Err(InAppReviewError::PermissionDenied),
            || Err(InAppReviewError::InvalidInput),
        ] {
            let s = run(&fake(r));
            assert!(s.status.is_empty());
            assert_eq!(s.error, FAILED_STATUS);
        }
    }

    #[test]
    fn unsupported_is_shown_as_unsupported() {
        let s = run(&UnsupportedInAppReview);
        assert!(s.status.is_empty());
        assert_eq!(s.error, UNSUPPORTED_STATUS);
        let s = run(&fake(|| Err(InAppReviewError::Unsupported)));
        assert_eq!(s.error, UNSUPPORTED_STATUS);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let mut s = InAppReviewState::default();
        request(&mut s);
        mark_requesting(&mut s);
        let stale = s.generation.wrapping_sub(1);
        finish_request(&mut s, stale, Ok(CoreOutcome::Requested));
        assert_eq!(s.phase, ReviewPhase::Requesting);
        assert!(s.status.is_empty());
    }
}
