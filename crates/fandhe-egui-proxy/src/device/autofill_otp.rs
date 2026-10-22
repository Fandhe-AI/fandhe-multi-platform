//! Phase 11 の拡充ループ型サンプル「ワンタイムコードの入力」（親 #121、Issue #585）の effect ドライバ。
//! `fandhe_core::screens::otp_input`（端末依存ゼロ）の要求を観測し、OS へ「この入力欄はワンタイム
//! コード」とヒントを 1 回伝えて結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect`
//! 区画から呼ばれる、`device/in_app_review.rs` と同型の同期経路）。契約と守る範囲は
//! `fandhe_device::autofill_otp` のモジュール doc を参照。
//!
//! ヒント API は completion を持たない同期呼び出しで、iOS は main thread で `UITextField` を触る必要が
//! あり effect は UI スレッドで動くため、UI スレッドで同期に呼ぶ。`RequestSlot` と bridge は持たない。
//! **dispatch はここで最終形**（iOS 実装、それ以外は `UnsupportedAutofillOtp`）。Android・macOS は
//! 保留、Windows は非対応、Linux は対象外で fallback に落ちる。Web は `fandhe-web-dom`（c、#587）が
//! DOM を直接触って担い、本ドライバは wasm でビルドしない。
//!
//! 「SMS を待つ」（WebOTP）はネイティブに無い。`sms_wait_available` は立たずボタンも出ないが、
//! 防御として要求が来たら OS を呼ばず非対応で確定する（`Waiting` に留めない）。

use fandhe_core::screens::otp_input::{
    finish_hint, finish_wait, mark_hint_applying, mark_waiting, take_hint_request,
    take_wait_request, CoreWaitFailure, HintOutcome, OtpInputState, HINT_FAILED_STATUS,
};
use fandhe_device::autofill_otp::AutofillOtp;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。想定外の種別（`PermissionDenied` 等）も失敗に倒す。
fn to_hint_outcome(result: Result<(), DeviceError>) -> Result<HintOutcome, String> {
    match result {
        Ok(()) => Ok(HintOutcome::Applied),
        Err(DeviceError::Unsupported(_)) => Ok(HintOutcome::Unsupported),
        Err(_) => Err(HINT_FAILED_STATUS.to_string()),
    }
}

/// ヒント要求があれば [`AutofillOtp`] を `on = true` で 1 回呼び、世代付きで結果を返す。
fn drive_hint(
    state: &OtpInputState,
    sender: &CompletionSender<OtpInputState>,
    autofill: &dyn AutofillOtp,
) {
    let Some(request) = take_hint_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_hint_applying));
    let outcome = to_hint_outcome(autofill.set_one_time_code_hint(true));
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut OtpInputState| {
        finish_hint(s, generation, outcome);
    }));
}

/// 待ち受け要求があれば、OS を呼ばず非対応で確定する（ネイティブに WebOTP は無い）。
fn drive_wait(state: &OtpInputState, sender: &CompletionSender<OtpInputState>) {
    let Some(request) = take_wait_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_waiting));
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut OtpInputState| {
        finish_wait(s, generation, Err(CoreWaitFailure::Unsupported));
    }));
}

/// OS 別の [`AutofillOtp`] 実装を返す（後続 issue は各 OS モジュールだけを埋める）。
fn platform_autofill_otp() -> Box<dyn AutofillOtp> {
    #[cfg(target_os = "ios")]
    return super::ios_autofill_otp::implementation();
    #[cfg(not(target_os = "ios"))]
    return Box::new(fandhe_device::autofill_otp::UnsupportedAutofillOtp);
}

/// `registry.rs` から配線される effect ドライバ。
///
/// ponytail: ヒントは OS への 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。戻らない呼び出しが
/// 見つかったら作業スレッド + bridge へ移す。
pub fn autofill_otp_effect(state: &OtpInputState, sender: &CompletionSender<OtpInputState>) {
    if take_hint_request(state).is_none() && take_wait_request(state).is_none() {
        return;
    }
    drive_hint(state, sender, platform_autofill_otp().as_ref());
    drive_wait(state, sender);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::otp_input::{
        retry_hint, set_sms_wait_available, start_wait, HintPhase, WaitPhase, HINT_APPLIED_STATUS,
        HINT_UNSUPPORTED_STATUS, WAIT_UNSUPPORTED_STATUS,
    };
    use fandhe_device::autofill_otp::UnsupportedAutofillOtp;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    /// 呼び出し回数と引数 `on` を記録し、決めた結果を返す偽実装。
    struct Fake {
        calls: AtomicU32,
        args: Mutex<Vec<bool>>,
        result: fn() -> Result<(), DeviceError>,
    }

    impl AutofillOtp for Fake {
        fn set_one_time_code_hint(&self, on: bool) -> Result<(), DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.args.lock().unwrap().push(on);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<(), DeviceError>) -> Fake {
        Fake {
            calls: AtomicU32::new(0),
            args: Mutex::new(Vec::new()),
            result,
        }
    }

    /// ヒント要求を 1 回ドライバで処理し、反映後の状態を返す。
    fn run_hint(autofill: &dyn AutofillOtp) -> OtpInputState {
        let mut s = OtpInputState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive_hint(&s, &sender, autofill);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        s
    }

    #[test]
    fn no_request_never_calls_os() {
        let f = fake(|| Ok(()));
        let mut s = OtpInputState::default();
        mark_hint_applying(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive_hint(&s, &sender, &f);
        assert!(rx.try_recv().is_err());
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn one_request_calls_os_once_with_on_true() {
        let f = fake(|| Ok(()));
        let s = run_hint(&f);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert_eq!(*f.args.lock().unwrap(), vec![true]);
        assert_eq!(s.hint, HintPhase::Applied);
        assert_eq!(s.status, HINT_APPLIED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn failures_never_become_success() {
        for r in [
            (|| Err(DeviceError::Internal("autofill_otp hint"))) as fn() -> Result<(), DeviceError>,
            || Err(DeviceError::PermissionDenied("autofill_otp")),
            || Err(DeviceError::InvalidInput("autofill_otp")),
            || Err(DeviceError::Cancelled("autofill_otp")),
        ] {
            let s = run_hint(&fake(r));
            assert_eq!(s.hint, HintPhase::Failed);
            assert!(s.status.is_empty());
            assert_eq!(s.error, HINT_FAILED_STATUS);
        }
    }

    #[test]
    fn unsupported_is_shown_as_unsupported() {
        let s = run_hint(&UnsupportedAutofillOtp);
        assert_eq!(s.hint, HintPhase::Unsupported);
        assert!(s.status.is_empty());
        assert_eq!(s.error, HINT_UNSUPPORTED_STATUS);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let mut s = OtpInputState::default();
        mark_hint_applying(&mut s);
        s.hint = HintPhase::Applying;
        let stale = s.hint_generation.wrapping_add(1);
        finish_hint(&mut s, stale, Ok(HintOutcome::Applied));
        assert_eq!(s.hint, HintPhase::Applying);
        retry_hint(&mut s);
        assert_eq!(s.hint, HintPhase::Applying);
    }

    #[test]
    fn native_wait_request_is_settled_as_unsupported() {
        let mut s = OtpInputState::default();
        set_sms_wait_available(&mut s, true);
        start_wait(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive_wait(&s, &sender);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        assert_eq!(s.wait, WaitPhase::Idle);
        assert_eq!(s.error, WAIT_UNSUPPORTED_STATUS);
        assert!(s.status.is_empty());
    }

    #[test]
    fn host_platform_is_unsupported() {
        #[cfg(not(target_os = "ios"))]
        assert!(matches!(
            platform_autofill_otp().set_one_time_code_hint(true),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
