//! Phase 11 の拡充ループ（親 #1130、Issue #1282）「ネットワーク接続状態」の effect ドライバ。
//! `fandhe_device::network_status::NetworkStatus` を一定間隔で読み、結果を
//! `fandhe_core::screens::network_status_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。
//!
//! **間隔つき pull**（`gamepad.rs` と同型）: 最終読取時刻から `READ_INTERVAL` 未満なら OS を
//! 読まず残り時間で再描画を予約する。読んだら次回の再描画を必ず予約し（入力が無いと egui が
//! 描画せずポーリングが止まる、契約表 4）、状態が変わらなければ送信を省略する。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS は
//! `ios_network_status.rs`（Apple 共用実装）を返し、Android は `android_network_status.rs`
//! （`ConnectivityManager`、#1284）を返す。Web は `fandhe-web-dom` 側（#1285）。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::network_status_show::{
    apply_read, KindView, NetworkStatusShowState, ReadFailure, Reading,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::network_status::UnsupportedNetworkStatus;
use fandhe_device::network_status::{
    ConnectionKind, NetworkSnapshot, NetworkStatus, READ_INTERVAL,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。macOS の monitor を
/// 二重に作らないため `OnceLock` で保持する）。
pub(crate) fn implementation() -> Arc<dyn NetworkStatus> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn NetworkStatus>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                Arc::new(super::windows_network_status::WindowsNetworkStatus)
            }
            #[cfg(target_os = "macos")]
            {
                Arc::new(super::apple_network_status::AppleNetworkStatus)
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_network_status::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_network_status::implementation()
            }
            #[cfg(not(any(
                target_os = "windows",
                target_os = "macos",
                target_os = "ios",
                target_os = "android"
            )))]
            {
                Arc::new(UnsupportedNetworkStatus)
            }
        })
        .clone()
}

fn to_kind(k: ConnectionKind) -> KindView {
    match k {
        ConnectionKind::Wifi => KindView::Wifi,
        ConnectionKind::Cellular => KindView::Cellular,
        ConnectionKind::Ethernet => KindView::Ethernet,
        ConnectionKind::Other => KindView::Other,
        ConnectionKind::Unknown => KindView::Unknown,
    }
}

/// 契約の結果を画面の入力へ写す。失敗は固定文言へ畳み、OS 固有値を画面へ漏らさない
/// （security.md A09）。
fn to_result(r: Result<NetworkSnapshot, DeviceError>) -> Result<Reading, ReadFailure> {
    match r {
        Ok(NetworkSnapshot::Online { kind, metered }) => Ok(Reading::Online {
            kind: to_kind(kind),
            metered,
        }),
        Ok(NetworkSnapshot::Offline) => Ok(Reading::Offline),
        Ok(NetworkSnapshot::Unknown) => Ok(Reading::Unknown),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(ReadFailure::Failed("接続状態の取得".to_string())),
    }
}

/// 最終読取時刻（プロセス全体で 1 つ。`gamepad.rs::LAST_TICK` と同じ位置づけ）。
static LAST_READ: Mutex<Option<Instant>> = Mutex::new(None);

/// 間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &NetworkStatusShowState,
    sender: &CompletionSender<NetworkStatusShowState>,
    net: &dyn NetworkStatus,
    now: Instant,
    last: &mut Option<Instant>,
) {
    if let Some(prev) = *last {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            sender.request_repaint_after(READ_INTERVAL - elapsed);
            return;
        }
    }
    *last = Some(now);
    // 送信を省略する分岐でも次回の読み取りを予約する。
    sender.request_repaint_after(READ_INTERVAL);

    let result = to_result(net.read());
    let mut next = state.clone();
    apply_read(&mut next, result.clone());
    if next == *state {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut NetworkStatusShowState| {
        apply_read(s, result);
    }));
}

/// `registry.rs` の `effect` 区画から `network_status_show` 画面用に配線される effect ドライバ。
pub fn network_status_show_effect(
    state: &NetworkStatusShowState,
    sender: &CompletionSender<NetworkStatusShowState>,
) {
    let mut last = LAST_READ
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        implementation().as_ref(),
        Instant::now(),
        &mut last,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::network_status_show::{connection_text, NetworkView};
    use fandhe_device::network_status::UnsupportedNetworkStatus as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<NetworkStatusShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<NetworkStatusShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<NetworkSnapshot, DeviceError>,
        reads: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<NetworkSnapshot, DeviceError>) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
            }
        }
    }

    impl NetworkStatus for Fake {
        fn read(&self) -> Result<NetworkSnapshot, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &self.result {
                Ok(s) => Ok(*s),
                Err(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Err(_) => Err(DeviceError::Internal("x")),
            }
        }
    }

    fn wifi() -> NetworkSnapshot {
        NetworkSnapshot::Online {
            kind: ConnectionKind::Wifi,
            metered: Some(false),
        }
    }

    fn apply_all(
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<NetworkStatusShowState>>,
        s: &mut NetworkStatusShowState,
    ) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    #[test]
    fn skips_os_read_within_interval() {
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new(Ok(wifi()));
        let state = NetworkStatusShowState::default();
        let t0 = Instant::now();
        let mut last = None;
        drive(&state, &tx, &fake, t0, &mut last);
        drive(
            &state,
            &tx,
            &fake,
            t0 + Duration::from_millis(500),
            &mut last,
        );
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        drive(&state, &tx, &fake, t0 + READ_INTERVAL, &mut last);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
        drop(rx);
    }

    #[test]
    fn same_value_is_not_sent_and_change_is() {
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new(Ok(wifi()));
        let mut state = NetworkStatusShowState::default();
        let t0 = Instant::now();
        let mut last = None;
        drive(&state, &tx, &fake, t0, &mut last);
        apply_all(&rx, &mut state);
        assert!(connection_text(&state).contains("接続あり"));
        // 同値: 送信されない。
        drive(&state, &tx, &fake, t0 + READ_INTERVAL, &mut last);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failures_never_become_online() {
        for r in [
            Err(DeviceError::Unsupported("network_status")),
            Err(DeviceError::Internal("x")),
        ] {
            let (tx, rx) = sender_with_channel();
            let fake = Fake::new(r);
            let mut state = NetworkStatusShowState::default();
            drive(&state, &tx, &fake, Instant::now(), &mut None);
            apply_all(&rx, &mut state);
            assert!(!connection_text(&state).contains("接続あり"));
            assert!(matches!(
                state.view,
                NetworkView::Unsupported | NetworkView::Failed(_)
            ));
        }
    }

    #[test]
    fn online_is_not_kept_after_failure() {
        let (tx, rx) = sender_with_channel();
        let mut state = NetworkStatusShowState::default();
        let t0 = Instant::now();
        let mut last = None;
        drive(&state, &tx, &Fake::new(Ok(wifi())), t0, &mut last);
        apply_all(&rx, &mut state);
        let failing = Fake::new(Err(DeviceError::Internal("x")));
        drive(&state, &tx, &failing, t0 + READ_INTERVAL, &mut last);
        apply_all(&rx, &mut state);
        assert!(!matches!(state.view, NetworkView::Online { .. }));
    }

    #[test]
    fn unsupported_impl_reports_unsupported() {
        let (tx, rx) = sender_with_channel();
        let mut state = NetworkStatusShowState::default();
        drive(&state, &tx, &Unsupported, Instant::now(), &mut None);
        apply_all(&rx, &mut state);
        assert_eq!(state.view, NetworkView::Unsupported);
    }
}
