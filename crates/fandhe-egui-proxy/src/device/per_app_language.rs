//! Phase 11 の拡充ループ（親 #137）型サンプル「アプリの言語」（Issue #1073）の effect ドライバ。
//! `fandhe_core::screens::per_app_language_show`（端末依存ゼロ）の要求を `take_request` で観測し、
//! `fandhe_device::per_app_language::PerAppLanguage` を UI スレッドから同期で 1 回だけ読んで、
//! 結果を `CompletionSender` 経由で注入する（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::per_app_language`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_per_app_language`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を
//! 持つモジュールだけを埋める（macOS: `apple_per_app_language`、iOS: `ios_per_app_language`（#1074）、
//! Android: `android_per_app_language`（#1075））。それ以外は `UnsupportedPerAppLanguage` 固定で、
//! ダミーの成功実装へは倒さない。
//!
//! # 同期読み取り
//!
//! 読み取りは押下時の 1 回だけで、ワーカーも再描画予約も持たない。完了は `finish_read` が
//! 世代を照合して反映する。

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
use fandhe_device::per_app_language::UnsupportedPerAppLanguage;
use fandhe_device::per_app_language::{AppLanguage, LanguageSource, PerAppLanguage};
use fandhe_device::DeviceError;

use fandhe_core::screens::per_app_language_show::{
    finish_read, take_request, LanguageOrigin, LanguageReading, PerAppLanguageShowState,
    ReadFailure,
};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `PerAppLanguage` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_per_app_language() -> Box<dyn PerAppLanguage> {
    #[cfg(target_os = "macos")]
    {
        super::apple_per_app_language::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_per_app_language::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_per_app_language::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        Box::new(UnsupportedPerAppLanguage)
    }
}

/// device の値を画面用の値へ写す。
fn to_reading(l: AppLanguage) -> LanguageReading {
    LanguageReading {
        tag: l.tag,
        origin: match l.source {
            LanguageSource::PerApp => LanguageOrigin::PerApp,
            LanguageSource::System => LanguageOrigin::System,
            LanguageSource::Unknown => LanguageOrigin::Unknown,
        },
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &PerAppLanguageShowState,
    sender: &CompletionSender<PerAppLanguageShowState>,
    lang: &dyn PerAppLanguage,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;
    let result = match lang.read() {
        Ok(l) => Ok(to_reading(l)),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        // 画面へは固定文言のみ（OS の生の値は出さない、security.md A09）。
        Err(_) => Err(ReadFailure::Failed("アプリの言語の読み取り".to_string())),
    };
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut PerAppLanguageShowState| {
        finish_read(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `per_app_language_show` 画面用に配線される effect ドライバ。
pub fn per_app_language_show_effect(
    state: &PerAppLanguageShowState,
    sender: &CompletionSender<PerAppLanguageShowState>,
) {
    drive(state, sender, platform_per_app_language().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::per_app_language_show::{read, ReadPhase, UNSUPPORTED_STATUS};
    use fandhe_device::per_app_language::UnsupportedPerAppLanguage as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        result: Option<AppLanguage>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Option<AppLanguage>) -> Self {
            Self {
                result,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl PerAppLanguage for Fake {
        fn read(&self) -> Result<AppLanguage, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
                .clone()
                .ok_or(DeviceError::Internal("per_app_language read"))
        }
    }

    fn lang(source: LanguageSource) -> AppLanguage {
        AppLanguage {
            tag: "ja".into(),
            source,
        }
    }

    fn channel() -> (
        CompletionSender<PerAppLanguageShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<PerAppLanguageShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 要求を立てて `drive` し、届いた完了をすべて適用した状態を返す。
    fn run(l: &dyn PerAppLanguage) -> PerAppLanguageShowState {
        let (sender, rx) = channel();
        let mut state = PerAppLanguageShowState::default();
        read(&mut state);
        drive(&state, &sender, l);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        state
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Fake::new(Some(lang(LanguageSource::Unknown)));
        drive(&PerAppLanguageShowState::default(), &sender, &fake);
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_sources_are_mapped() {
        for (s, o) in [
            (LanguageSource::PerApp, LanguageOrigin::PerApp),
            (LanguageSource::System, LanguageOrigin::System),
            (LanguageSource::Unknown, LanguageOrigin::Unknown),
        ] {
            let state = run(&Fake::new(Some(lang(s))));
            let r = state.reading.expect("成功");
            assert_eq!(r.tag, "ja");
            assert_eq!(r.origin, o);
            assert_eq!(state.error, "");
        }
    }

    #[test]
    fn unsupported_is_error_not_success() {
        let state = run(&Unsupported);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.reading, None);
        assert_eq!(state.status, "");
    }

    #[test]
    fn internal_failure_is_not_success() {
        let state = run(&Fake::new(None));
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.reading, None);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let (sender, rx) = channel();
        let mut state = PerAppLanguageShowState::default();
        read(&mut state);
        drive(
            &state,
            &sender,
            &Fake::new(Some(lang(LanguageSource::Unknown))),
        );
        // 完了が届く前に状態が別の世代へ進んだ想定。
        state.phase = ReadPhase::Idle;
        read(&mut state);
        let before = state.clone();
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state, before);
    }
}
