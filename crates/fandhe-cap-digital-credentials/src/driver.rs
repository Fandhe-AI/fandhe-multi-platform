//! Phase 11（親 #186）型サンプル「デジタル身分証の提示要求」（Issue #958）の effect ドライバ。
//! `fandhe_core::screens::digital_credentials`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約）。
//! 契約と守る範囲は `crate::contract` のモジュール doc を参照。
//!
//! 全ネイティブ OS（macOS・Linux・Windows・iOS・Android）は同期で `Unsupported` を返す。
//! **dispatch はこれが最終形**で、後続の子は触らない。Web は `fandhe-web-dom`。
//! ワーカーを持たないため `RequestSlot` は使わない。

use crate::contract::{CredentialOutcome, FEATURE};
use crate::error::DigitalCredentialsError;
use fandhe_core::screens::digital_credentials::{
    CoreOutcome, DigitalCredentialsState, finish, mark_running, take_request,
};

use fandhe_runtime::CompletionSender;

/// 内部名を出さない固定文言（security.md A09）。
const FAILED_TEXT: &str = "身分証の提示要求を完了できませんでした";

fn core_outcome(
    result: Result<CredentialOutcome, DigitalCredentialsError>,
) -> Result<CoreOutcome, String> {
    match result {
        Ok(CredentialOutcome::Responded) => Ok(CoreOutcome::Responded),
        Ok(CredentialOutcome::NotShared) => Ok(CoreOutcome::NotShared),
        Ok(CredentialOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(DigitalCredentialsError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

fn drive_sync(
    state: &DigitalCredentialsState,
    sender: &CompletionSender<DigitalCredentialsState>,
    result: fn() -> Result<CredentialOutcome, DigitalCredentialsError>,
) {
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_running));
    let outcome = core_outcome(result());
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut DigitalCredentialsState| {
        finish(s, generation, outcome);
    }));
}

/// ネイティブ OS での身分証の提示要求の結果。全ネイティブ OS は同期で `Unsupported`（Android は対応表で `possible`
/// のまま実装が無く、成功へは倒さない）。Web の実体は `fandhe-web-dom`。画面の状態型に依存しない入口で、
/// フレームワークの外のアプリも同じ結果を直接読める。
pub fn platform_digital_credentials() -> Result<CredentialOutcome, DigitalCredentialsError> {
    Err(DigitalCredentialsError::Unsupported(FEATURE))
}

/// `fandhe-gallery` の `screens/registry.rs` から配線される effect ドライバ。全ネイティブで常に「使えない」を表示する。
pub fn digital_credentials_effect(
    state: &DigitalCredentialsState,
    sender: &CompletionSender<DigitalCredentialsState>,
) {
    drive_sync(state, sender, platform_digital_credentials);
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 起床しないだけの口（受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    use fandhe_core::screens::digital_credentials as screen;

    #[test]
    fn outcomes_cover_all_arms() {
        assert_eq!(
            core_outcome(Ok(CredentialOutcome::Responded)),
            Ok(CoreOutcome::Responded)
        );
        assert_eq!(
            core_outcome(Ok(CredentialOutcome::NotShared)),
            Ok(CoreOutcome::NotShared)
        );
        assert_eq!(
            core_outcome(Ok(CredentialOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            core_outcome(Err(DigitalCredentialsError::Unsupported(FEATURE))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            core_outcome(Err(DigitalCredentialsError::Internal(
                "digital_credentials failed"
            ))),
            Err(FAILED_TEXT.to_string())
        );
        assert!(!FAILED_TEXT.contains("digital") && !FAILED_TEXT.contains("credential"));
    }

    fn run(
        result: fn() -> Result<CredentialOutcome, DigitalCredentialsError>,
    ) -> DigitalCredentialsState {
        let mut s = DigitalCredentialsState::default();
        screen::request(&mut s);
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive_sync(&s, &sender, result);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        s
    }

    #[test]
    fn unsupported_host_never_succeeds() {
        let s = run(|| Err(DigitalCredentialsError::Unsupported(FEATURE)));
        assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn failure_goes_to_error_and_not_shared_does_not() {
        let s = run(|| {
            Err(DigitalCredentialsError::Internal(
                "digital_credentials failed",
            ))
        });
        assert!(s.status.is_empty() && s.error.starts_with(screen::FAILED_PREFIX));
        let s = run(|| Ok(CredentialOutcome::NotShared));
        assert_eq!(s.status, screen::NOT_SHARED_STATUS);
        assert!(s.error.is_empty());
    }

    #[test]
    fn no_request_sends_nothing() {
        let s = DigitalCredentialsState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
        drive_sync(&s, &sender, || Ok(CredentialOutcome::Responded));
        assert!(rx.try_recv().is_err());
    }
}
