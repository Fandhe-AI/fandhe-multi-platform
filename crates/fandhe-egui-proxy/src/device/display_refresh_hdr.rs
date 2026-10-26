//! Phase 11 の拡充ループ（親 #177）型サンプル「画面のリフレッシュレート・HDR・広色域」
//! （Issue #870）の effect ドライバ。`fandhe_core::screens::display_refresh_hdr_show`（端末依存
//! ゼロ）の要求を `take_request` で観測し、`fandhe_device::display_refresh_hdr::DisplayRefreshHdr`
//! を UI スレッドから同期で 1 回だけ読んで、結果を `CompletionSender` 経由で注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::display_refresh_hdr`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_display_refresh_hdr`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()`
//! を持つモジュールだけを埋める（macOS: `macos_display_refresh_hdr`、Windows:
//! `windows_display_refresh_hdr`、iOS: `ios_display_refresh_hdr`（#871）、Android:
//! `android_display_refresh_hdr`（#872））。それ以外は `UnsupportedDisplayRefreshHdr` 固定で、
//! ダミーの成功実装へは倒さない。
//!
//! # 同期読み取り
//!
//! 読み取りは押下時の 1 回だけで、ワーカーも再描画予約も持たない。完了は `finish_read` が
//! 世代を照合して反映する。

use fandhe_core::screens::display_refresh_hdr_show::{
    finish_read, take_request, DisplayReading, DisplayRefreshHdrShowState, ReadFailure,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::display_refresh_hdr::UnsupportedDisplayRefreshHdr;
use fandhe_device::display_refresh_hdr::{DisplayCapabilities, DisplayRefreshHdr};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `DisplayRefreshHdr` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_display_refresh_hdr() -> Box<dyn DisplayRefreshHdr> {
    #[cfg(target_os = "windows")]
    {
        super::windows_display_refresh_hdr::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_display_refresh_hdr::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_display_refresh_hdr::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_display_refresh_hdr::implementation()
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
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
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
    use fandhe_core::screens::display_refresh_hdr_show::{read, ReadPhase, UNSUPPORTED_STATUS};
    use fandhe_device::display_refresh_hdr::UnsupportedDisplayRefreshHdr as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        fn read(&self) -> Result<DisplayCapabilities, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.caps
                .ok_or(DeviceError::Internal("display_refresh_hdr read"))
        }
    }

    fn channel() -> (
        CompletionSender<DisplayRefreshHdrShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<DisplayRefreshHdrShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
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
