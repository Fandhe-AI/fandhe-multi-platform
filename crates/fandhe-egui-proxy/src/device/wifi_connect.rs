//! Phase 11（親 #165）型サンプル「指定した Wi-Fi への接続」（Issue #791）の effect ドライバ。
//! `fandhe_core::screens::wifi_connect_show`（端末依存ゼロ）の要求を観測し、入力を
//! `fandhe_device::wifi_connect::validate` で検査してから `WifiConnect` で接続を 1 回要求し、
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は
//! `fandhe_device::wifi_connect`。
//!
//! # スレッド
//!
//! macOS は位置情報の許可ダイアログの応答や管理者パスワードのダイアログを待つブロッキング呼び出しに
//! なるため、UI を止めないよう OS を問わずワーカースレッドで呼ぶ。完了は画面状態の世代照合つきで入れる
//! （古い世代は捨てられる）。要求の追跡は世代照合で足りるため `RequestSlot` は使わない
//! （`wifi_info.rs` と同じ判断）。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを埋める
//! （macOS: `macos_wifi_connect`（#791）、Windows: `windows_wifi_connect`（#792）、
//! Android: `android_wifi_connect`（#1064 のスタブ、#1065 が埋める））。Android の JNI 照合は OS 別モジュールの
//! 内部で行う。それ以外（iOS は対応表で `deferred`・Linux 等）は `UnsupportedWifiConnect` 固定で、
//! ダミーの成功実装へは倒さない。

use std::sync::Arc;

use fandhe_core::screens::wifi_connect_show::{
    finish_connect, mark_connecting, take_request, ConnectFailure, PermissionHint,
    WifiConnectShowState,
};
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
use fandhe_device::wifi_connect::UnsupportedWifiConnect;
use fandhe_device::wifi_connect::{validate, WifiConnect, NOT_FOUND, UNSUPPORTED_SECURITY};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `WifiConnect` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn WifiConnect> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn WifiConnect>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_wifi_connect::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_wifi_connect::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_wifi_connect::implementation()
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
            {
                Arc::new(UnsupportedWifiConnect)
            }
        })
        .clone()
}

/// 許可がないときの案内の出し分け（OS ごとに設定の場所が違う）。
const PERMISSION_HINT: PermissionHint = if cfg!(target_os = "windows") {
    PermissionHint::WindowsSettings
} else {
    PermissionHint::SystemSettings
};

/// `DeviceError` を画面の失敗種別へ写す。その他は固定文言へ畳み、OS 固有の値を画面へ漏らさない
/// （security.md A09）。
fn to_failure(e: DeviceError) -> ConnectFailure {
    match e {
        DeviceError::Unsupported(_) => ConnectFailure::Unsupported,
        DeviceError::PermissionDenied(_) => ConnectFailure::PermissionDenied(PERMISSION_HINT),
        DeviceError::Disabled(_) => ConnectFailure::RadioOff,
        DeviceError::InvalidInput(_) => ConnectFailure::InvalidInput,
        DeviceError::Internal(NOT_FOUND) => ConnectFailure::NotFound,
        DeviceError::Internal(UNSUPPORTED_SECURITY) => {
            ConnectFailure::Failed("対応していない認証方式".to_string())
        }
        _ => ConnectFailure::Failed("Wi-Fi への接続".to_string()),
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &WifiConnectShowState,
    sender: &CompletionSender<WifiConnectShowState>,
    wifi: Arc<dyn WifiConnect>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_connecting));

    // OS を呼ぶ前に入力を検査する。不正なら OS には何も渡さない。
    let checked = match validate(&request.ssid, &request.password) {
        Ok(r) => r,
        Err(e) => {
            let failure = to_failure(e);
            let _ = sender.send(Box::new(move |s: &mut WifiConnectShowState| {
                finish_connect(s, generation, Err(failure));
            }));
            return;
        }
    };

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("wifi-connect".to_string())
        .spawn(move || {
            let result = wifi.connect(&checked).map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut WifiConnectShowState| {
                finish_connect(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut WifiConnectShowState| {
            finish_connect(
                s,
                generation,
                Err(ConnectFailure::Failed("Wi-Fi への接続".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `wifi_connect_show` 画面用に配線される effect ドライバ。
pub fn wifi_connect_show_effect(
    state: &WifiConnectShowState,
    sender: &CompletionSender<WifiConnectShowState>,
) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::wifi_connect_show::{
        connect, set_password, set_ssid, Outcome, Phase, CONNECTED_STATUS, FAILED_PREFIX,
        INVALID_INPUT_STATUS, NOT_FOUND_STATUS, PERMISSION_SYSTEM_SETTINGS,
        PERMISSION_WINDOWS_SETTINGS, RADIO_OFF_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::wifi_connect::{
        ConnectRequest, UnsupportedWifiConnect as Unsupported, CONNECT_FAILED, FEATURE, RADIO_OFF,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<WifiConnectShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<WifiConnectShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<(), fn() -> DeviceError>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                result: Ok(()),
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

    impl WifiConnect for Fake {
        fn connect(&self, _request: &ConnectRequest) -> Result<(), DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.map_err(|f| f())
        }
    }

    fn requested(ssid: &str, password: &str) -> WifiConnectShowState {
        let mut s = WifiConnectShowState::default();
        set_ssid(&mut s, ssid.to_string());
        set_password(&mut s, password.to_string());
        connect(&mut s);
        s
    }

    /// 要求を立てて `drive` し、届いた完了（mark_connecting と結果）をすべて適用した状態を返す。
    fn run(wifi: Arc<dyn WifiConnect>, ssid: &str, password: &str) -> WifiConnectShowState {
        let (sender, rx) = sender_with_channel();
        let mut state = requested(ssid, password);
        drive(&state, &sender, wifi);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("mark_connecting と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::ok();
        drive(&WifiConnectShowState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_is_reported_once() {
        let fake = Fake::ok();
        let state = run(fake.clone(), "home", "password1");
        assert_eq!(state.outcome, Some(Outcome::Connected));
        assert_eq!(state.status, CONNECTED_STATUS);
        assert_eq!(state.error, "");
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalid_input_never_reaches_os() {
        for (ssid, pw) in [("", ""), ("home", "short")] {
            let fake = Fake::ok();
            let state = run(fake.clone(), ssid, pw);
            assert_eq!(state.error, INVALID_INPUT_STATUS);
            assert_eq!(state.outcome, None);
            assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn each_failure_is_distinct_error_not_success() {
        let hint = if cfg!(target_os = "windows") {
            PERMISSION_WINDOWS_SETTINGS
        } else {
            PERMISSION_SYSTEM_SETTINGS
        };
        let cases: [(fn() -> DeviceError, String); 8] = [
            (
                || DeviceError::Unsupported(FEATURE),
                UNSUPPORTED_STATUS.into(),
            ),
            (|| DeviceError::PermissionDenied(FEATURE), hint.into()),
            (|| DeviceError::Disabled(RADIO_OFF), RADIO_OFF_STATUS.into()),
            (
                || DeviceError::InvalidInput(FEATURE),
                INVALID_INPUT_STATUS.into(),
            ),
            (|| DeviceError::Internal(NOT_FOUND), NOT_FOUND_STATUS.into()),
            (
                || DeviceError::Internal(UNSUPPORTED_SECURITY),
                format!("{FAILED_PREFIX}対応していない認証方式"),
            ),
            (
                || DeviceError::Internal(CONNECT_FAILED),
                format!("{FAILED_PREFIX}Wi-Fi への接続"),
            ),
            (
                || DeviceError::Io(std::io::Error::other("x")),
                format!("{FAILED_PREFIX}Wi-Fi への接続"),
            ),
        ];
        for (f, expected) in cases {
            let state = run(Fake::err(f), "home", "password1");
            assert_eq!(state.error, expected);
            assert_eq!(state.outcome, None);
        }
    }

    #[test]
    fn second_drive_while_connecting_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::ok();
        let mut state = requested("home", "");
        drive(&state, &sender, fake.clone());
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_connecting")(&mut state);
        drive(&state, &sender, fake.clone());
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_completion_does_not_overwrite_newer_request() {
        let (sender, rx) = sender_with_channel();
        let mut state = requested("home", "");
        drive(&state, &sender, Fake::ok());
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_connecting")(&mut state);
        // 完了が届く前に世代が進んだ状況を再現する。
        state.phase = Phase::Idle;
        connect(&mut state);
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert_eq!(state.generation, 2);
        assert_eq!(state.outcome, None);
        assert_eq!(state.phase, Phase::Requested);
    }

    #[test]
    fn unsupported_implementation_never_yields_success() {
        let state = run(Arc::new(Unsupported), "home", "");
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.outcome, None);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        let req = validate("home", "").expect("検査を通る");
        assert!(matches!(
            implementation().connect(&req),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
