//! Phase 11（親 #111）型サンプル「Wi-Fi 情報」（Issue #514）の effect ドライバ。
//! `fandhe_core::screens::wifi_info_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::wifi_info::WifiInfo` で現在の接続と周辺を読んで `CompletionSender` 経由で結果を
//! 返す（`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::wifi_info`。
//!
//! # スレッド
//!
//! macOS は位置情報の許可ダイアログの応答を待つブロッキング呼び出しになるため、UI を止めないよう
//! OS を問わずワーカースレッドで呼ぶ。完了は画面状態の世代照合つきで入れる（古い世代は捨てられる）。
//! 要求の追跡は世代照合で足りるため `RequestSlot` は使わない（Android 実装が Java の許可ランチャーを
//! 待つ場合は `android_wifi_info.rs` の内側で使う）。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを
//! 埋める（macOS: `macos_wifi_info`（#514）、Windows: `windows_wifi_info`、Android:
//! `android_wifi_info`）。それ以外（iOS は対応表で `deferred`・Linux 等）は `UnsupportedWifiInfo` 固定で、
//! ダミーの成功実装へは倒さない。

use std::sync::Arc;

use fandhe_core::screens::wifi_info_show::{
    finish_read, mark_loading, take_request, PermissionHint, WifiFailure, WifiInfoShowState,
    WifiRow, WifiView,
};
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
use fandhe_device::wifi_info::UnsupportedWifiInfo;
use fandhe_device::wifi_info::{WifiInfo, WifiNetwork, WifiReport, LOCATION_OFF};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `WifiInfo` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn WifiInfo> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn WifiInfo>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_wifi_info::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_wifi_info::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_wifi_info::implementation()
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
            {
                Arc::new(UnsupportedWifiInfo)
            }
        })
        .clone()
}

/// 許可がないときの案内の出し分け（OS ごとに設定の場所が違う）。
const PERMISSION_HINT: PermissionHint = if cfg!(target_os = "windows") {
    PermissionHint::WindowsSettings
} else if cfg!(target_os = "android") {
    PermissionHint::AndroidPrecise
} else {
    PermissionHint::SystemSettings
};

/// `DeviceError` を画面の失敗種別へ写す。その他は固定文言へ畳み、OS 固有の値を画面へ漏らさない
/// （security.md A09）。
fn to_failure(e: DeviceError) -> WifiFailure {
    match e {
        DeviceError::Unsupported(_) => WifiFailure::Unsupported,
        DeviceError::PermissionDenied(_) => WifiFailure::PermissionDenied(PERMISSION_HINT),
        DeviceError::Disabled(LOCATION_OFF) => WifiFailure::LocationOff,
        DeviceError::Disabled(_) => WifiFailure::RadioOff,
        _ => WifiFailure::Failed("Wi-Fi 情報の読み取り".to_string()),
    }
}

fn to_row(n: WifiNetwork) -> WifiRow {
    WifiRow {
        // `Ssid::expose` を使うのは画面への受け渡しのここだけ（`fandhe-core` は `fandhe-device` を
        // 参照できないため）。
        ssid: n.ssid.map(|s| s.expose().to_string()),
        rssi_dbm: n.rssi_dbm,
    }
}

fn to_view(r: WifiReport) -> WifiView {
    WifiView {
        current: r.current.map(to_row),
        nearby: r.nearby.into_iter().map(to_row).collect(),
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &WifiInfoShowState,
    sender: &CompletionSender<WifiInfoShowState>,
    wifi: Arc<dyn WifiInfo>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_loading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("wifi-info-read".to_string())
        .spawn(move || {
            let result = wifi.read().map(to_view).map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut WifiInfoShowState| {
                finish_read(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut WifiInfoShowState| {
            finish_read(
                s,
                generation,
                Err(WifiFailure::Failed("Wi-Fi 情報の読み取り".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `wifi_info_show` 画面用に配線される effect ドライバ。
pub fn wifi_info_show_effect(
    state: &WifiInfoShowState,
    sender: &CompletionSender<WifiInfoShowState>,
) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::wifi_info_show::{
        fetch, Phase, FAILED_PREFIX, LOCATION_OFF_STATUS, PERMISSION_ANDROID_PRECISE,
        PERMISSION_SYSTEM_SETTINGS, PERMISSION_WINDOWS_SETTINGS, RADIO_OFF_STATUS,
        UNSUPPORTED_STATUS,
    };
    use fandhe_device::wifi_info::{
        Ssid, UnsupportedWifiInfo as Unsupported, FEATURE, RADIO_OFF, READ_FAILED,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<WifiInfoShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<WifiInfoShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<WifiReport, fn() -> DeviceError>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn ok(r: WifiReport) -> Arc<Self> {
            Arc::new(Self {
                result: Ok(r),
                calls: AtomicUsize::new(0),
            })
        }
        fn err(f: fn() -> DeviceError) -> Arc<Self> {
            Arc::new(Self {
                result: Err(f),
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl WifiInfo for Fake {
        fn read(&self) -> Result<WifiReport, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone().map_err(|f| f())
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_loading と結果）をすべて適用した状態を返す。
    fn run(wifi: Arc<dyn WifiInfo>) -> WifiInfoShowState {
        let (sender, rx) = sender_with_channel();
        let mut state = WifiInfoShowState::default();
        fetch(&mut state);
        drive(&state, &sender, wifi);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("mark_loading と完了が届く");
            c(&mut state);
        }
        state
    }

    fn net(name: &str, rssi: i16) -> WifiNetwork {
        WifiNetwork {
            ssid: Ssid::from_bytes(name.as_bytes()),
            rssi_dbm: Some(rssi),
        }
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::ok(WifiReport::default());
        drive(&WifiInfoShowState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn report_is_passed_through() {
        let state = run(Fake::ok(WifiReport {
            current: Some(net("home", -40)),
            nearby: vec![net("n1", -60)],
        }));
        let v = state.view.expect("成功");
        assert_eq!(v.current.unwrap().ssid.as_deref(), Some("home"));
        assert_eq!(v.nearby.len(), 1);
        assert_eq!(state.error, "");
    }

    #[test]
    fn disconnected_with_no_nearby_is_success() {
        let state = run(Fake::ok(WifiReport::default()));
        assert_eq!(state.view, Some(WifiView::default()));
        assert_eq!(state.error, "");
    }

    #[test]
    fn each_failure_is_distinct_error_not_success() {
        let hint = if cfg!(target_os = "windows") {
            PERMISSION_WINDOWS_SETTINGS
        } else if cfg!(target_os = "android") {
            PERMISSION_ANDROID_PRECISE
        } else {
            PERMISSION_SYSTEM_SETTINGS
        };
        let cases: [(fn() -> DeviceError, String); 5] = [
            (
                || DeviceError::Unsupported(FEATURE),
                UNSUPPORTED_STATUS.into(),
            ),
            (|| DeviceError::PermissionDenied(FEATURE), hint.into()),
            (|| DeviceError::Disabled(RADIO_OFF), RADIO_OFF_STATUS.into()),
            (
                || DeviceError::Disabled(LOCATION_OFF),
                LOCATION_OFF_STATUS.into(),
            ),
            (
                || DeviceError::Internal(READ_FAILED),
                format!("{FAILED_PREFIX}Wi-Fi 情報の読み取り"),
            ),
        ];
        for (f, expected) in cases {
            let state = run(Fake::err(f));
            assert_eq!(state.error, expected);
            assert_eq!(state.view, None);
        }
    }

    #[test]
    fn second_drive_while_loading_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::ok(WifiReport::default());
        let mut state = WifiInfoShowState::default();
        fetch(&mut state);
        drive(&state, &sender, fake.clone());
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_loading")(&mut state);
        drive(&state, &sender, fake.clone());
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_completion_does_not_overwrite_newer_request() {
        let (sender, rx) = sender_with_channel();
        let mut state = WifiInfoShowState::default();
        fetch(&mut state);
        drive(&state, &sender, Fake::ok(WifiReport::default()));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_loading")(&mut state);
        // 完了が届く前に世代が進んだ状況を再現する。
        state.phase = Phase::Idle;
        fetch(&mut state);
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert_eq!(state.generation, 2);
        assert_eq!(state.view, None);
        assert_eq!(state.phase, Phase::Requested);
    }

    #[test]
    fn unsupported_implementation_never_yields_success() {
        let state = run(Arc::new(Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.view, None);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            implementation().read(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
