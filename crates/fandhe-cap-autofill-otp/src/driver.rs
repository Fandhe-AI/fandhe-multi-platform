//! Phase 11 の拡充ループ型サンプル「ワンタイムコードの入力」（親 #121、Issue #585）の effect ドライバ。
//! `fandhe_core::screens::otp_input`（端末依存ゼロ）の要求を観測し、OS へ「この入力欄はワンタイム
//! コード」とヒントを 1 回伝えて結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect`
//! 区画から呼ばれる。画面の要求を同期で処理する経路）。契約と守る範囲は
//! `contract` のモジュール doc を参照。
//!
//! ヒント API は completion を持たない同期呼び出しで、iOS は main thread で `UITextField` を触る必要が
//! あり effect は UI スレッドで動くため、UI スレッドで同期に呼ぶ。`RequestSlot` と bridge は持たない。
//! **dispatch はここで最終形**（iOS 実装、それ以外は `UnsupportedAutofillOtp`）。Android・macOS は
//! 保留、Windows は非対応、Linux は対象外で fallback に落ちる。Web は `fandhe-web-dom`（c、#587）が
//! DOM を直接触って担い、本ドライバは wasm でビルドしない。
//!
//! 「SMS を待つ」（WebOTP）はネイティブに無い。`sms_wait_available` は立たずボタンも出ないが、
//! 防御として要求が来たら OS を呼ばず非対応で確定する（`Waiting` に留めない）。

use crate::contract::AutofillOtp;
use crate::error::AutofillOtpError;
use fandhe_core::screens::otp_input::{
    CoreWaitFailure, HINT_FAILED_STATUS, HintOutcome, OtpInputState, finish_hint, finish_wait,
    mark_hint_applying, mark_waiting, take_hint_request, take_wait_request,
};
use fandhe_runtime::CompletionSender;
use fandhe_runtime::{TextInputHint, TextInputHintError};

/// `AutofillOtpError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。想定外の種別（`PermissionDenied` 等）も失敗に倒す。
fn to_hint_outcome(result: Result<(), AutofillOtpError>) -> Result<HintOutcome, String> {
    match result {
        Ok(()) => Ok(HintOutcome::Applied),
        Err(AutofillOtpError::Unsupported) => Ok(HintOutcome::Unsupported),
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

/// 描画層の入力欄のヒント（`HostView::set_text_input_hint`）へつなぐ [`AutofillOtp`] 実装。
/// 橋渡しを登録するのは iOS だけで、それ以外は描画層が `Unsupported` を返す。`AutofillOtp` は
/// `Send + Sync` を要求するため、`&dyn HostView` ではなく `&CompletionSender`（`Send + Sync`）を持ち、
/// 呼ぶたびに `host_view()` を引く。
struct HostViewAutofillOtp<'a, S>(&'a CompletionSender<S>);

/// 描画層の入力欄のヒントへつなぐ [`AutofillOtp`] を返す。OS 固有の実装はこの crate に無く
/// （iOS の UIKit 側の橋渡しは描画層が `HostView::set_text_input_hint` の裏で持つ。置き場の移動は #1194）、
/// 橋渡しが無い描画層では `Unsupported` を返す。画面の状態型に依存しないため、フレームワークの外のアプリも使える。
pub fn platform_autofill_otp<S>(sender: &CompletionSender<S>) -> impl AutofillOtp + '_ {
    HostViewAutofillOtp(sender)
}

impl<S> AutofillOtp for HostViewAutofillOtp<'_, S> {
    fn set_one_time_code_hint(&self, on: bool) -> Result<(), AutofillOtpError> {
        let hint = if on {
            TextInputHint::OneTimeCode
        } else {
            TextInputHint::Default
        };
        self.0
            .host_view()
            .set_text_input_hint(hint)
            .map_err(|e| match e {
                TextInputHintError::Unsupported => AutofillOtpError::Unsupported,
                TextInputHintError::Failed => AutofillOtpError::Internal("autofill_otp hint"),
            })
    }
}

/// `registry.rs` から配線される effect ドライバ。
///
/// ponytail: ヒントは OS への 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。戻らない呼び出しが
/// 見つかったら作業スレッド + bridge へ移す。
pub fn autofill_otp_effect(state: &OtpInputState, sender: &CompletionSender<OtpInputState>) {
    if take_hint_request(state).is_none() && take_wait_request(state).is_none() {
        return;
    }
    drive_hint(state, sender, &platform_autofill_otp(sender));
    drive_wait(state, sender);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedAutofillOtp;
    use fandhe_core::screens::otp_input::{
        HINT_APPLIED_STATUS, HINT_UNSUPPORTED_STATUS, HintPhase, WAIT_UNSUPPORTED_STATUS,
        WaitPhase, retry_hint, set_sms_wait_available, start_wait,
    };
    use fandhe_runtime::RepaintWaker;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _: std::time::Duration) {}
    }

    /// 呼び出し回数と引数 `on` を記録し、決めた結果を返す偽実装。
    struct Fake {
        calls: AtomicU32,
        args: Mutex<Vec<bool>>,
        result: fn() -> Result<(), AutofillOtpError>,
    }

    impl AutofillOtp for Fake {
        fn set_one_time_code_hint(&self, on: bool) -> Result<(), AutofillOtpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.args.lock().unwrap().push(on);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<(), AutofillOtpError>) -> Fake {
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
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
            (|| Err(AutofillOtpError::Internal("autofill_otp hint")))
                as fn() -> Result<(), AutofillOtpError>,
            || Err(AutofillOtpError::PermissionDenied),
            || Err(AutofillOtpError::InvalidInput("autofill_otp")),
            || Err(AutofillOtpError::Cancelled),
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
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive_wait(&s, &sender);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        assert_eq!(s.wait, WaitPhase::Idle);
        assert_eq!(s.error, WAIT_UNSUPPORTED_STATUS);
        assert!(s.status.is_empty());
    }

    /// 橋渡しを持たない描画層（ホスト・テスト）では非対応、失敗は固定の内部エラーへ写る。
    #[test]
    fn host_view_errors_map_to_device_errors() {
        use fandhe_runtime::{HostView, RepaintWaker};
        use std::time::Duration;

        struct View(Result<(), TextInputHintError>);
        impl HostView for View {
            fn set_text_input_hint(&self, _: TextInputHint) -> Result<(), TextInputHintError> {
                self.0
            }
        }
        struct Waker(View);
        impl RepaintWaker for Waker {
            fn wake(&self) {}
            fn wake_after(&self, _: Duration) {}
            fn host_view(&self) -> &dyn HostView {
                &self.0
            }
        }
        let call = |r| {
            let (tx, _rx) = std::sync::mpsc::channel::<fandhe_runtime::Completion<OtpInputState>>();
            let sender = CompletionSender::new(tx, std::sync::Arc::new(Waker(View(r))));
            HostViewAutofillOtp(&sender).set_one_time_code_hint(true)
        };
        assert!(matches!(
            call(Err(TextInputHintError::Unsupported)),
            Err(AutofillOtpError::Unsupported)
        ));
        assert!(matches!(
            call(Err(TextInputHintError::Failed)),
            Err(AutofillOtpError::Internal("autofill_otp hint"))
        ));
        assert!(call(Ok(())).is_ok());
    }
}
