//! Phase 11 の拡充ループ（親 #177）型サンプル「画面のリフレッシュレート・HDR・広色域」
//! （Issue #870）の effect ドライバ。`fandhe_core::screens::display_refresh_hdr_show`（端末依存
//! ゼロ）の要求を `take_request` で観測し、`crate::contract::DisplayRefreshHdr`
//! を UI スレッドから同期で 1 回だけ読んで、結果を `CompletionSender` 経由で注入する
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `crate::contract`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_display_refresh_hdr`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()`
//! を持つモジュールだけを埋める（macOS: `macos`、Windows: `windows`、iOS: `ios`（#871）、
//! Android: `android`（#872））。それ以外は `UnsupportedDisplayRefreshHdr` 固定で、
//! ダミーの成功実装へは倒さない。
//!
//! # 同期読み取り
//!
//! 読み取りは押下時の 1 回だけで、ワーカーも再描画予約も持たない。完了は `finish_read` が
//! 世代を照合して反映する。

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use crate::contract::UnsupportedDisplayRefreshHdr;
use crate::contract::{DisplayCapabilities, DisplayRefreshHdr};
use crate::error::DisplayRefreshHdrError;
use fandhe_core::screens::display_refresh_hdr_show::{
    DisplayReading, DisplayRefreshHdrShowState, ReadFailure, finish_read, take_request,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた `DisplayRefreshHdr` 実装を返す（状態を持たないため呼び出しごとに構築）。
///
/// 画面の状態型に依存しない入口で、フレームワークの外のアプリも契約を直接呼べる。
pub fn platform_display_refresh_hdr() -> Box<dyn DisplayRefreshHdr> {
    #[cfg(target_os = "windows")]
    {
        crate::windows::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        crate::ios::implementation()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::implementation()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Box::new(UnsupportedDisplayRefreshHdr)
    }
}

/// device の値を画面用の値へ写す。
fn to_reading(c: DisplayCapabilities) -> DisplayReading {
    DisplayReading {
        current_hz: c.current_hz,
        max_hz: c.max_hz,
        hdr: c.hdr,
        wide_gamut: c.wide_gamut,
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &DisplayRefreshHdrShowState,
    sender: &CompletionSender<DisplayRefreshHdrShowState>,
    display: &dyn DisplayRefreshHdr,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;
    let result = match display.read() {
        Ok(c) => Ok(to_reading(c)),
        Err(DisplayRefreshHdrError::Unsupported) => Err(ReadFailure::Unsupported),
        // 画面へは固定文言のみ（OS の生コードは出さない、security.md A09）。
        Err(_) => Err(ReadFailure::Failed("画面の表示能力の読み取り".to_string())),
    };
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut DisplayRefreshHdrShowState| {
        finish_read(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `display_refresh_hdr_show` 画面用に配線される effect ドライバ。
pub fn display_refresh_hdr_show_effect(
    state: &DisplayRefreshHdrShowState,
    sender: &CompletionSender<DisplayRefreshHdrShowState>,
) {
    drive(state, sender, platform_display_refresh_hdr().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedDisplayRefreshHdr as Unsupported;
    use fandhe_core::screens::display_refresh_hdr_show::{ReadPhase, UNSUPPORTED_STATUS, read};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 起床しないだけの口（受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    struct Fake {
        caps: Option<DisplayCapabilities>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(caps: Option<DisplayCapabilities>) -> Self {
            Self {
                caps,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl DisplayRefreshHdr for Fake {
        fn read(&self) -> Result<DisplayCapabilities, DisplayRefreshHdrError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.caps
                .ok_or(DisplayRefreshHdrError::Internal("display_refresh_hdr read"))
        }
    }

    fn channel() -> (
        CompletionSender<DisplayRefreshHdrShowState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<DisplayRefreshHdrShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    /// 要求を立てて `drive` し、届いた完了をすべて適用した状態を返す。
    fn run(display: &dyn DisplayRefreshHdr) -> DisplayRefreshHdrShowState {
        let (sender, rx) = channel();
        let mut state = DisplayRefreshHdrShowState::default();
        read(&mut state);
        drive(&state, &sender, display);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        state
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Fake::new(Some(DisplayCapabilities::default()));
        drive(&DisplayRefreshHdrShowState::default(), &sender, &fake);
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_is_mapped() {
        let state = run(&Fake::new(Some(DisplayCapabilities {
            current_hz: Some(60),
            hdr: Some(false),
            ..Default::default()
        })));
        let r = state.reading.expect("成功");
        assert_eq!(r.current_hz, Some(60));
        assert_eq!(r.hdr, Some(false));
        assert_eq!(r.max_hz, None);
        assert_eq!(state.error, "");
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
        let mut state = DisplayRefreshHdrShowState::default();
        read(&mut state);
        drive(
            &state,
            &sender,
            &Fake::new(Some(DisplayCapabilities::default())),
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
