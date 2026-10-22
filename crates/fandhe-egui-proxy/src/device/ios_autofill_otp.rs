//! iOS 向けワンタイムコード自動入力実装（Issue #586、b-ios）。
//! `device/autofill_otp.rs` の iOS 経路（`platform_autofill_otp`）から呼ばれ、契約は
//! `fandhe_device::autofill_otp`。ソフトキーボード入力の隠し `UITextField`
//! （`ios/fandhe_ios_text_input.m` の `FandheTextField`）へ `textContentType = oneTimeCode` と
//! 数字キーボードを設定し、QuickType バーに SMS コードの候補が出る状態にする。
//!
//! # 日本語入力への影響
//!
//! `on = false` では `nil` と `UIKeyboardTypeDefault`（生成直後の既定値）へ戻す。OTP 以外の画面
//! （日本語入力）では setter が呼ばれず、生成時も既定値を入れ直すだけなので挙動は変わらない。
//!
//! # 結果の写像
//!
//! `.m` の戻りコード 0 は `Ok`、2（main 以外）・3（入力欄の橋渡し未完）・未知値は `Internal`。
//! 未完を成功と偽らない（契約表 6）。計測 feature のビルドは入力欄の橋渡し自体が無いので
//! `Unsupported`（`build.rs`・`lib.rs` の `ios_text_input` と同じ条件）。
//!
//! # 再描画（契約表 4）
//!
//! OS コールバックが共有状態へ結果を入れる経路は無く、結果は effect ドライバが同期で受け取り
//! `CompletionSender::send`（内部で再描画を要求）が起こすため、`crate::ios::request_repaint` は
//! 呼ばない（該当なし）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 嘘の成功の禁止、main 以外で UIKit に触れないこと、コードの値をログに出さないこと。
//! 守らない: 候補が出るか（OS と SMS 次第）、domain-bound code・パスワード保存、同じプロセスの
//! 別画面へヒントが残ること。確認は #576。

use fandhe_device::autofill_otp::AutofillOtp;
#[cfg(not(all(
    target_os = "ios",
    not(any(feature = "cold-start-probe", feature = "frame-metrics"))
)))]
use fandhe_device::autofill_otp::UnsupportedAutofillOtp;
#[cfg(any(
    test,
    all(
        target_os = "ios",
        not(any(feature = "cold-start-probe", feature = "frame-metrics"))
    )
))]
use fandhe_device::DeviceError;

// `ios/fandhe_ios_text_input.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
// 計測 feature では `.m` がコンパイルされないため、宣言も同じ条件で囲む（リンク切れ防止）。
#[cfg(all(
    target_os = "ios",
    not(any(feature = "cold-start-probe", feature = "frame-metrics"))
))]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_text_input_set_one_time_code_hint(on: i32) -> i32;
}

/// `.m` の戻りコード（0 = 成功、それ以外 = 失敗）を契約のエラー種別へ写す。
#[cfg(any(
    test,
    all(
        target_os = "ios",
        not(any(feature = "cold-start-probe", feature = "frame-metrics"))
    )
))]
fn decode(code: i32) -> Result<(), DeviceError> {
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("autofill_otp hint"))
    }
}

/// iOS の [`AutofillOtp`] 実装（隠し入力欄の `textContentType`）。
#[cfg(all(
    target_os = "ios",
    not(any(feature = "cold-start-probe", feature = "frame-metrics"))
))]
struct IosAutofillOtp;

#[cfg(all(
    target_os = "ios",
    not(any(feature = "cold-start-probe", feature = "frame-metrics"))
))]
impl AutofillOtp for IosAutofillOtp {
    #[allow(unsafe_code)]
    fn set_one_time_code_hint(&self, on: bool) -> Result<(), DeviceError> {
        // SAFETY: 引数は値渡しの i32 のみでポインタを経由しない。main スレッドの検査は
        // `.m` 側で行い、main 以外では UIKit に触れず 2 を返す。
        let code = unsafe { fandhe_ios_text_input_set_one_time_code_hint(i32::from(on)) };
        decode(code)
    }
}

/// iOS 用の [`AutofillOtp`] 実装を返す。
#[cfg(all(
    target_os = "ios",
    not(any(feature = "cold-start-probe", feature = "frame-metrics"))
))]
pub(crate) fn implementation() -> Box<dyn AutofillOtp> {
    Box::new(IosAutofillOtp)
}

/// ホスト・計測ビルドでは非対応を返す（計測ビルドは入力欄の橋渡しが無い）。
#[cfg(not(all(
    target_os = "ios",
    not(any(feature = "cold-start-probe", feature = "frame-metrics"))
)))]
pub(crate) fn implementation() -> Box<dyn AutofillOtp> {
    Box::new(UnsupportedAutofillOtp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_zero_is_ok() {
        assert!(decode(0).is_ok());
    }

    #[test]
    fn nonzero_codes_are_internal_errors() {
        for code in [2, 3, -1, 99] {
            assert!(matches!(
                decode(code),
                Err(DeviceError::Internal("autofill_otp hint"))
            ));
        }
    }
}
