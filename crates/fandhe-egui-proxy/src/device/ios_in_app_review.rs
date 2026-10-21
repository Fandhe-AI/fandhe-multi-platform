//! iOS 向けアプリ内レビュー依頼のスタブ（Issue #563、a）。`device/in_app_review.rs` の dispatch
//! （ios 経路）から呼ばれる。実体は b-ios が埋めるまで `Unsupported` を返す（嘘の成功を作らない）。
//!
//! b-ios が埋める範囲（dispatch・画面・fallback 行は触らない）:
//! - `ios/fandhe_ios_in_app_review.m`: main thread で ForegroundActive の `UIWindowScene` を取り
//!   `requestReviewInScene:` を呼ぶ。scene が無ければ `Internal`。`keyWindow` へ逃がさない
//! - `build.rs` の iOS 分岐で StoreKit をリンクする

use fandhe_device::in_app_review::{InAppReview, UnsupportedInAppReview};

/// iOS の `InAppReview` 実装を返す（b-ios まではスタブ）。
pub(crate) fn implementation() -> Box<dyn InAppReview> {
    Box::new(UnsupportedInAppReview)
}
