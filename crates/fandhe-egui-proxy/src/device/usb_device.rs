//! Phase 11 の拡充ループ（親 #112、Issue #505）型サンプル「USB 機器」の effect ドライバ。
//! `fandhe_core::screens::usb_device_show`（端末依存ゼロ）の要求を `take_request` で観測し、
//! `fandhe_device::usb_device::UsbDevices` を呼んで、結果を `CompletionSender` 経由で注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる
//! 契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::usb_device`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_usb_devices`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `macos_usb_device`、Windows: `windows_usb_device`（#506）、
//! Android: `android_usb_device`（#507））。それ以外（iOS を含む）は `UnsupportedUsbDevices` 固定で、
//! ダミーの成功実装へは倒さない。
//!
//! # 同期のみ・期限なし
//!
//! 全 OS とも UI スレッドから同期で 1 回呼ぶ（[`drive_sync`]）。IOKit も SetupAPI も COM を使わず、
//! ワーカーを置かない。OS 呼び出しが戻らない場合の期限は持たない（既知の天井、
//! `fandhe_device::usb_device` 参照）。Web 由来の読み方（WebUSB / WebHID）がネイティブへ来た場合は
//! OS を呼ばず `Unsupported` で完了する（嘘の成功にしない）。

use fandhe_core::screens::usb_device_show::{
    finish_read, take_request, ReadFailure, ReadSource, UsbDeviceRow, UsbDeviceShowState,
};
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
use fandhe_device::usb_device::UnsupportedUsbDevices;
use fandhe_device::usb_device::{UsbDeviceEntry, UsbDevices};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `UsbDevices` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_usb_devices() -> Box<dyn UsbDevices> {
    #[cfg(target_os = "macos")]
    {
        super::macos_usb_device::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_usb_device::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_usb_device::implementation()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    {
        Box::new(UnsupportedUsbDevices)
    }
}

fn to_rows(entries: Vec<UsbDeviceEntry>) -> Vec<UsbDeviceRow> {
    entries
        .into_iter()
        .map(|e| UsbDeviceRow {
            vendor_id: e.vendor_id,
            product_id: e.product_id,
            name: e.name,
        })
        .collect()
}

/// エラーを画面用の失敗へ写す。画面へは固定文言のみ（OS の生コードは出さない、security.md A09）。
/// ネイティブ経路に `Cancelled` は無いので作らない。
fn to_failure(e: DeviceError) -> ReadFailure {
    match e {
        DeviceError::Unsupported(_) => ReadFailure::Unsupported,
        _ => ReadFailure::Failed("USB 機器の読み取り".to_string()),
    }
}

/// 要求 1 件を UI スレッドで同期処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive_sync(
    state: &UsbDeviceShowState,
    sender: &CompletionSender<UsbDeviceShowState>,
    devices: &dyn UsbDevices,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let result = match request.source {
        ReadSource::System => devices.list().map(to_rows).map_err(to_failure),
        // Web 専用の読み方。OS は呼ばない。
        ReadSource::WebUsb | ReadSource::WebHid => Err(ReadFailure::Unsupported),
    };
    let generation = request.generation;
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut UsbDeviceShowState| {
        finish_read(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `usb_device_show` 画面用に配線される effect ドライバ。
pub fn usb_device_show_effect(
    state: &UsbDeviceShowState,
    sender: &CompletionSender<UsbDeviceShowState>,
) {
    drive_sync(state, sender, platform_usb_devices().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::usb_device_show::{pick_usb, read, ReadPhase, UNSUPPORTED_STATUS};
    use fandhe_device::usb_device::UnsupportedUsbDevices as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    struct Fake {
        result: Option<Vec<UsbDeviceEntry>>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Option<Vec<UsbDeviceEntry>>) -> Self {
            Self {
                result,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl UsbDevices for Fake {
        fn list(&self) -> Result<Vec<UsbDeviceEntry>, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
                .clone()
                .ok_or(DeviceError::Internal("usb_device list"))
        }
    }

    fn sample() -> Vec<UsbDeviceEntry> {
        vec![UsbDeviceEntry {
            vendor_id: 0x05ac,
            product_id: 0x024f,
            name: Some("Keyboard".into()),
        }]
    }

    type Rx = mpsc::Receiver<crate::app::Completion<UsbDeviceShowState>>;

    fn channel() -> (CompletionSender<UsbDeviceShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn run(state: &mut UsbDeviceShowState, devices: &dyn UsbDevices) {
        let (sender, rx) = channel();
        drive_sync(state, &sender, devices);
        while let Ok(c) = rx.try_recv() {
            c(state);
        }
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Fake::new(Some(sample()));
        let idle = UsbDeviceShowState {
            phase: ReadPhase::Idle,
            ..Default::default()
        };
        drive_sync(&idle, &sender, &fake);
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_is_mapped() {
        let mut state = UsbDeviceShowState::default();
        run(&mut state, &Fake::new(Some(sample())));
        let rows = state.rows.expect("成功");
        assert_eq!((rows[0].vendor_id, rows[0].product_id), (0x05ac, 0x024f));
        assert_eq!(state.error, "");
    }

    #[test]
    fn unsupported_is_error_not_success() {
        let mut state = UsbDeviceShowState::default();
        run(&mut state, &Unsupported);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!((state.rows, state.status.as_str()), (None, ""));
    }

    #[test]
    fn internal_failure_is_not_success() {
        let mut state = UsbDeviceShowState::default();
        run(&mut state, &Fake::new(None));
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.rows, None);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let (sender, rx) = channel();
        let mut state = UsbDeviceShowState::default();
        drive_sync(&state, &sender, &Fake::new(Some(sample())));
        // 完了が届く前に状態が別の世代へ進んだ想定。
        state.phase = ReadPhase::Idle;
        read(&mut state);
        let before = state.clone();
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state, before);
    }

    #[test]
    fn web_sources_are_unsupported_without_calling_os() {
        let fake = Fake::new(Some(sample()));
        let mut state = UsbDeviceShowState::browser_picker();
        pick_usb(&mut state);
        run(&mut state, &fake);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            platform_usb_devices().list(),
            Err(DeviceError::Unsupported("usb_device"))
        ));
    }
}
