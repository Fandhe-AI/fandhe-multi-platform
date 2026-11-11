//! Phase 11 の拡充ループ（親 #181、Issue #903）型サンプル「アプリ・端末の識別子」の effect ドライバ。
//! `fandhe_core::screens::device_identifier_show`（端末依存ゼロ）の要求を `take_request` で観測し、
//! `DeviceIdentifier`（`crate::contract`） を UI スレッドから同期で 1 回だけ読んで、
//! 伏せた結果（末尾 4 文字と文字数）だけを `CompletionSender` 経由で注入する
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から呼ばれる）。契約の SSOT は
//! `crate::contract`。機能 crate へ移行済み（Issue #1183）で、`fandhe_core::screens` を参照してよいのは本ファイルだけ。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_device_identifier`] の cfg 分岐が全対象 OS の最終形で、iOS（`crate::ios`、
//! `UIDevice.identifierForVendor`）と Android（`crate::android`、`Settings.Secure.ANDROID_ID`）
//! は #904 で実体になっている。
//! それ以外（macOS・Windows・Linux・Web）は対応表で `no` のため `Unsupported` 固定で、成功には届かない。
//! 識別子の全体（`DeviceId`）はここで捨てて画面へ渡さない。ログ出力はしない。

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use crate::contract::UnsupportedDeviceIdentifier;
use crate::contract::{DeviceIdentifier, IdentifierRead};
use crate::error::DeviceIdentifierError;
use fandhe_core::screens::device_identifier_show::{
    DeviceIdentifierShowState, IdScope, IdentifierReading, ReadFailure, ReadOutcome, finish_read,
    take_request,
};

use fandhe_runtime::CompletionSender;

/// 画面に出す範囲。iOS / Android 以外は `Unsupported` 固定で表示に届かない。
#[cfg(target_os = "android")]
const SCOPE: IdScope = IdScope::SigningKeyUserDevice;
#[cfg(not(target_os = "android"))]
const SCOPE: IdScope = IdScope::Vendor;

/// 現在のプラットフォームに応じた実装を返す（状態を持たないため呼び出しごとに構築）。画面の状態型に
/// 依存しない入口で、フレームワークの外のアプリも自分の状態型の effect から使える。
pub fn platform_device_identifier() -> Box<dyn DeviceIdentifier> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::implementation()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::implementation()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Box::new(UnsupportedDeviceIdentifier)
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &DeviceIdentifierShowState,
    sender: &CompletionSender<DeviceIdentifierShowState>,
    ident: &dyn DeviceIdentifier,
    scope: IdScope,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;
    let result = match ident.read() {
        Ok(IdentifierRead::Identifier(id)) => Ok(ReadOutcome::Identifier(IdentifierReading {
            tail: id.masked_tail(),
            char_count: id.char_count(),
            scope,
        })),
        Ok(IdentifierRead::NotYetAvailable) => Ok(ReadOutcome::NotYetAvailable),
        Err(DeviceIdentifierError::Unsupported) => Err(ReadFailure::Unsupported),
        // 画面へは固定文言のみ（OS の生コードは出さない）。
        Err(_) => Err(ReadFailure::Failed("識別子の読み取り".to_string())),
    };
    let _ = sender.send(Box::new(move |s: &mut DeviceIdentifierShowState| {
        finish_read(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `device_identifier_show` 画面用に配線される effect ドライバ。
pub fn device_identifier_show_effect(
    state: &DeviceIdentifierShowState,
    sender: &CompletionSender<DeviceIdentifierShowState>,
) {
    drive(state, sender, platform_device_identifier().as_ref(), SCOPE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{DeviceId, UnsupportedDeviceIdentifier as Unsupported};
    use fandhe_core::screens::device_identifier_show::{ReadPhase, UNSUPPORTED_STATUS, read};
    use fandhe_runtime::RepaintWaker;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    const FAKE: &str = "00000000-0000-0000-0000-00000000ABCD";

    type ReadFn = fn() -> Result<IdentifierRead, DeviceIdentifierError>;

    struct Fake {
        result: ReadFn,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: ReadFn) -> Self {
            Self {
                result,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl DeviceIdentifier for Fake {
        fn read(&self) -> Result<IdentifierRead, DeviceIdentifierError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn channel() -> (
        CompletionSender<DeviceIdentifierShowState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<DeviceIdentifierShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    fn run(ident: &dyn DeviceIdentifier) -> DeviceIdentifierShowState {
        let (sender, rx) = channel();
        let mut state = DeviceIdentifierShowState::default();
        read(&mut state);
        drive(&state, &sender, ident, IdScope::Vendor);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        state
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Fake::new(|| Ok(IdentifierRead::NotYetAvailable));
        drive(
            &DeviceIdentifierShowState::default(),
            &sender,
            &fake,
            IdScope::Vendor,
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_is_masked() {
        let state = run(&Fake::new(|| {
            Ok(IdentifierRead::Identifier(
                DeviceId::new(FAKE.into()).unwrap(),
            ))
        }));
        let r = state.reading.as_ref().expect("成功");
        assert_eq!(
            (r.tail.as_str(), r.char_count, r.scope),
            ("ABCD", 36, IdScope::Vendor)
        );
        assert!(!format!("{state:?}").contains("0000-"));
    }

    #[test]
    fn not_yet_is_mapped() {
        let state = run(&Fake::new(|| Ok(IdentifierRead::NotYetAvailable)));
        assert_eq!(state.reading, None);
        assert_eq!(state.error, "");
        assert!(!state.status.is_empty());
    }

    #[test]
    fn unsupported_is_error_not_success() {
        let state = run(&Unsupported);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.reading, None);
        assert_eq!(state.status, "");
    }

    #[test]
    fn failures_never_succeed() {
        let cases: [ReadFn; 3] = [
            || Err(DeviceIdentifierError::Internal),
            || Err(DeviceIdentifierError::PermissionDenied),
            || Err(DeviceIdentifierError::Cancelled),
        ];
        for f in cases {
            let state = run(&Fake::new(f));
            assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
            assert_eq!(state.reading, None);
        }
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let (sender, rx) = channel();
        let mut state = DeviceIdentifierShowState::default();
        read(&mut state);
        drive(
            &state,
            &sender,
            &Fake::new(|| Ok(IdentifierRead::NotYetAvailable)),
            IdScope::Vendor,
        );
        state.phase = ReadPhase::Idle;
        read(&mut state);
        let before = state.clone();
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state, before);
    }
}
