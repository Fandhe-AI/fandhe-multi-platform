//! Phase 11 の拡充ループ（親 #1131）型サンプル「端末・OS 情報」（Issue #1304）の effect ドライバ。
//! `fandhe_core::screens::device_info_show`（端末依存ゼロ）の要求を `take_request` で観測し、
//! `fandhe_device::device_info::DeviceInfo` を UI スレッドから同期で 1 回だけ読んで、結果を
//! `CompletionSender` 経由で注入する（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::device_info`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_device_info`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を
//! 持つモジュールだけを埋める（macOS: `macos_device_info`、Windows: `windows_device_info`、
//! iOS: `ios_device_info`（#1305）、Android: `android_device_info`（#1306））。それ以外は
//! `UnsupportedDeviceInfo` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 同期読み取り
//!
//! 読み取りは押下時の 1 回だけで、ワーカーも再描画予約も持たない。完了は `finish_read` が
//! 世代を照合して反映する。

use fandhe_core::screens::device_info_show::{
    finish_read, take_request, DeviceInfoReading, DeviceInfoShowState, ReadFailure,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::device_info::UnsupportedDeviceInfo;
use fandhe_device::device_info::{DeviceInfo, DeviceInfoSnapshot};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `DeviceInfo` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_device_info() -> Box<dyn DeviceInfo> {
    #[cfg(target_os = "windows")]
    {
        super::windows_device_info::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_device_info::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_device_info::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_device_info::implementation()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Box::new(UnsupportedDeviceInfo)
    }
}

/// device の値を画面用の値へ写す。
fn to_reading(s: DeviceInfoSnapshot) -> DeviceInfoReading {
    DeviceInfoReading {
        os_name: s.os_name,
        os_version: s.os_version,
        model: s.model,
        memory_bytes: s.memory_bytes,
        free_storage_bytes: s.free_storage_bytes,
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &DeviceInfoShowState,
    sender: &CompletionSender<DeviceInfoShowState>,
    info: &dyn DeviceInfo,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;
    let result = match info.read() {
        Ok(snapshot) => Ok(to_reading(snapshot)),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        // 画面へは固定文言のみ（OS の生コードは出さない、security.md A09）。
        Err(_) => Err(ReadFailure::Failed("端末情報の読み取り".to_string())),
    };
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut DeviceInfoShowState| {
        finish_read(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `device_info_show` 画面用に配線される effect ドライバ。
pub fn device_info_show_effect(
    state: &DeviceInfoShowState,
    sender: &CompletionSender<DeviceInfoShowState>,
) {
    drive(state, sender, platform_device_info().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::device_info_show::{read, ReadPhase, UNSUPPORTED_STATUS};
    use fandhe_device::device_info::UnsupportedDeviceInfo as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        snapshot: Option<DeviceInfoSnapshot>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(snapshot: Option<DeviceInfoSnapshot>) -> Self {
            Self {
                snapshot,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl DeviceInfo for Fake {
        fn read(&self) -> Result<DeviceInfoSnapshot, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.snapshot
                .clone()
                .ok_or(DeviceError::Internal("device_info read"))
        }
    }

    fn channel() -> (
        CompletionSender<DeviceInfoShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<DeviceInfoShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 要求を立てて `drive` し、届いた完了をすべて適用した状態を返す。
    fn run(info: &dyn DeviceInfo) -> DeviceInfoShowState {
        let (sender, rx) = channel();
        let mut state = DeviceInfoShowState::default();
        read(&mut state);
        drive(&state, &sender, info);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        state
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Fake::new(Some(DeviceInfoSnapshot::default()));
        drive(&DeviceInfoShowState::default(), &sender, &fake);
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_is_mapped() {
        let state = run(&Fake::new(Some(DeviceInfoSnapshot {
            os_name: Some("macOS".into()),
            free_storage_bytes: Some(0),
            ..Default::default()
        })));
        let r = state.reading.expect("成功");
        assert_eq!(r.os_name.as_deref(), Some("macOS"));
        assert_eq!(r.free_storage_bytes, Some(0));
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
        let mut state = DeviceInfoShowState::default();
        read(&mut state);
        drive(
            &state,
            &sender,
            &Fake::new(Some(DeviceInfoSnapshot::default())),
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
