//! Phase 11 の拡充ループ（親 #121、Issue #585、a）: ワンタイムコード（OTP）の自動入力。
//! `fandhe-egui-proxy` の `device/autofill_otp.rs` が、画面を開いた最初の effect で
//! [`AutofillOtp::set_one_time_code_hint`] を UI スレッドから同期で 1 回呼ぶ。iOS（隠し
//! `UITextField` の `textContentType = .oneTimeCode`）が実装する（b-ios、#586）。Web は
//! wasm のメインスレッドで DOM（`autocomplete="one-time-code"` の本物の `<input>`、Chromium では
//! WebOTP）を直接触るため本 trait を通らず、本モジュールの純関数と定数だけを共有する（c、#587。
//! passkey と同じ形）。Android・macOS は保留、Windows は非対応。
//!
//! # 結果（契約表 10）
//!
//! ヒント（trait）:
//! - `Ok(())`: OS へ「この入力欄はワンタイムコード」と伝えた。**候補が出るかは分からない**
//! - `Err(Unsupported("autofill_otp"))`: Android・macOS（保留）、Windows（非対応）、Linux
//! - `Err(Internal(..))`: 反映に失敗した
//! - キャンセルは存在しない。`PermissionDenied` と `InvalidInput` は使わず、届いた場合も
//!   呼び出し側は失敗として扱う
//!
//! WebOTP の待ち受け（[`WaitOutcome`]）は受信・キャンセル（中立）・期限切れ（中立）・非対応・失敗の
//! 5 種。候補を選ばないことや SMS が来ないことは失敗ではない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 画面・1 プロセス内の順序（世代照合）、OS から入った値を加工せず届けること
//! （trim・全角半角変換・桁数の切り詰めをしない）、値を外へ出さないこと（ログ・`Debug`・
//! `console`・`localStorage`）、未対応を成功と表示しないこと、WebOTP の取り消し・タイマー・
//! リスナーの解放。
//!
//! 守らない: OS が候補を出すか（SMS の到着・文面・OS 設定・自動入力サービス次第）、
//! domain-bound code、パスワードとユーザー名、iOS で同じプロセスの別画面にヒントが残ること
//! （デモは 1 プロセス 1 画面）、Web で再構築直後から effect までの一瞬に属性が無いこと、複数タブ。

use crate::DeviceError;

/// WebOTP の待ち受けの上限（ミリ秒）。Web の `setTimeout` で使う。経過時間は単調時計側
/// （`setTimeout` / `performance.now()`）に任せ、壁時計（`Date.now()`）では測らない（契約表 3）。
pub const WEBOTP_TIMEOUT_MS: u32 = 120_000;

/// 入力欄へワンタイムコードのヒントを付ける。UI スレッドから同期で呼ばれる。契約は本モジュール doc。
pub trait AutofillOtp: Send + Sync {
    /// `on = true` で OS の自動入力に「この入力欄はワンタイムコード」と伝え、`false` で既定へ戻す。
    /// 冪等で、同じ値を何度渡しても結果は同じ。
    fn set_one_time_code_hint(&self, on: bool) -> Result<(), DeviceError>;
}

/// 非対応環境（保留の OS・Linux 等）用。常に `Unsupported`。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedAutofillOtp;

impl AutofillOtp for UnsupportedAutofillOtp {
    fn set_one_time_code_hint(&self, _on: bool) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("autofill_otp"))
    }
}

/// OS や WebOTP から届いたワンタイムコード。値はログや `Debug` に出さない（文字数だけ出す）。
#[derive(Clone, PartialEq, Eq)]
pub struct OtpCode(String);

impl OtpCode {
    /// 値を包む。空文字だけを `InvalidInput` で拒む（0 バイトを成功にしない）。
    /// trim・全角半角変換・桁数の切り詰めはしない。
    pub fn new(value: String) -> Result<Self, DeviceError> {
        if value.is_empty() {
            return Err(DeviceError::InvalidInput("autofill_otp code"));
        }
        Ok(Self(value))
    }

    /// 値を借りる。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 値を取り出す。
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for OtpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtpCode")
            .field("chars", &self.0.chars().count())
            .finish()
    }
}

/// WebOTP（`navigator.credentials.get({ otp })`）の待ち受け結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    /// コードを受け取った。
    Received(OtpCode),
    /// 利用者または画面の都合で取り消された（中立。失敗ではない）。
    Cancelled,
    /// 期限（[`WEBOTP_TIMEOUT_MS`]）で取り消した（中立。失敗ではない）。
    TimedOut,
}

/// DOM 例外の名前だけから待ち受け結果へ写像する純関数。`message` は受け取らない（秘匿）。
///
/// `NotAllowedError` / `AbortError` は `Cancelled`。期限による取り消しかどうかは呼び出し側の
/// フラグで見分け、`TimedOut` に置き換える。
pub fn wait_outcome_from_dom_error(name: &str) -> Result<WaitOutcome, DeviceError> {
    match name {
        "NotAllowedError" | "AbortError" => Ok(WaitOutcome::Cancelled),
        "SecurityError" | "NotSupportedError" => Err(DeviceError::Unsupported("autofill_otp")),
        _ => Err(DeviceError::Internal("autofill_otp webotp")),
    }
}

/// 受信時に入力欄が空のときだけ値を入れる規則の単一の定義元（SSOT）。
/// 呼び出し側は適用時点の最新の入力値で判定する。
pub fn should_fill(current: &str) -> bool {
    current.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        for on in [true, false] {
            assert!(matches!(
                UnsupportedAutofillOtp.set_one_time_code_hint(on),
                Err(DeviceError::Unsupported("autofill_otp"))
            ));
        }
    }

    #[test]
    fn dom_error_mapping() {
        for n in ["NotAllowedError", "AbortError"] {
            assert_eq!(
                wait_outcome_from_dom_error(n).unwrap(),
                WaitOutcome::Cancelled
            );
        }
        for n in ["SecurityError", "NotSupportedError"] {
            assert!(matches!(
                wait_outcome_from_dom_error(n),
                Err(DeviceError::Unsupported(_))
            ));
        }
        for n in ["InvalidStateError", "", "unknown"] {
            assert!(matches!(
                wait_outcome_from_dom_error(n),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn code_rejects_empty_and_keeps_raw() {
        assert!(matches!(
            OtpCode::new(String::new()),
            Err(DeviceError::InvalidInput(_))
        ));
        for raw in [" 123456 ", "１２３４５６", "123456789012345678"] {
            assert_eq!(OtpCode::new(raw.to_string()).unwrap().as_str(), raw);
        }
    }

    #[test]
    fn debug_hides_value() {
        let code = OtpCode::new("123456".into()).unwrap();
        let shown = format!("{code:?} {:?}", WaitOutcome::Received(code.clone()));
        assert!(!shown.contains("123456"));
        assert!(shown.contains("chars: 6"));
    }

    #[test]
    fn should_fill_only_when_empty() {
        assert!(should_fill(""));
        assert!(!should_fill(" "));
        assert!(!should_fill("1"));
    }
}
