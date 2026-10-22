//! iOS 向けワンタイムコード自動入力の骨格（Issue #585、a）。実体は b-ios（#586）が本ファイルの中だけで埋める。
//! `device/autofill_otp.rs` の iOS 経路（`platform_autofill_otp`）から呼ばれ、契約は
//! `fandhe_device::autofill_otp`。それまでは `Unsupported` を返し、成功を偽らない。
//!
//! b-ios が埋める範囲:
//! - `ios/fandhe_ios_text_input.m` に content hint の setter を足す（`textContentType = .oneTimeCode`・
//!   数字キーボード。`on = false` では `nil`・`Default` へ戻す）
//! - 生成時と setter 呼び出し時に反映し、first responder なら `reloadInputViews` する
//! - main queue と `@autoreleasepool` で包む
//! - OTP 以外の画面（日本語入力）の挙動を変えない
//! - cfg を `any(ios, test)` へ広げる必要が出ても、閉じるのは本ファイルの中。dispatch・画面・
//!   fallback 行・`device.rs` の `pub mod` 列は触らない

use fandhe_device::autofill_otp::{AutofillOtp, UnsupportedAutofillOtp};

/// iOS 用の [`AutofillOtp`] 実装を返す。b-ios が実体へ差し替えるまで非対応。
pub(crate) fn implementation() -> Box<dyn AutofillOtp> {
    Box::new(UnsupportedAutofillOtp)
}
