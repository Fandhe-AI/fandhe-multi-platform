//! Phase 11 の拡充ループ（親 #101）型サンプル「アプリショートカット」（Issue #424）の
//! effect ドライバ。`fandhe_core::screens::url_scheme_show`（端末依存ゼロ）へ、
//! `fandhe_device::app_shortcuts::AppShortcuts` の登録結果を一度だけ注入する。
//! `device/url_scheme.rs::url_scheme_show_effect` から呼ばれる（選ばれたショートカットの
//! 受信は URL として既存の `UrlInbox` に入るので、ここは「OS に出す側」だけ）。
//!
//! feature `device` かつ非 wasm32 でのみコンパイル対象になる（`device.rs` 参照）。
//!
//! # dispatch
//!
//! [`platform`] は全 OS 分の最終形。Android・Windows は兄弟 issue（iOS は #425 で実装済み） が各 OS のモジュールの
//! 中身だけを埋める。ここの分岐・[`ON_WORKER`] は変えない。
//!
//! # スレッド
//!
//! macOS・iOS は main thread のまま同期で呼ぶ。Windows・Android は別スレッドで呼ぶ
//! （COM・JNI の呼び出しを UI スレッドで待たない）。どちらも `finish` で状態を確定してから
//! 画面へ送る（`device-bridge-contracts.md`「2. 並行性」）。実行中は次回のポーリングだけ予約する。

use std::time::Duration;

use fandhe_core::screens::url_scheme_show::{
    apply_shortcut_status, ShortcutStatus, UrlSchemeShowState,
};
#[cfg(any(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    test
))]
use fandhe_device::app_shortcuts::UnsupportedAppShortcuts;
use fandhe_device::app_shortcuts::{AppShortcuts, InstallCell, InstallOutcome};

use crate::app::CompletionSender;

/// 別スレッドで実行中のとき結果を取りに戻る間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Windows・Android は別スレッドで呼ぶ。後続の子は変えない。
const ON_WORKER: bool = cfg!(any(target_os = "windows", target_os = "android"));

/// プロセス全体で 1 回だけ登録するための状態。
static CELL: InstallCell = InstallCell::new();

/// 現在の OS の実装。
fn platform() -> &'static dyn AppShortcuts {
    #[cfg(target_os = "macos")]
    {
        super::macos_app_shortcuts::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_app_shortcuts::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_app_shortcuts::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_app_shortcuts::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        &UnsupportedAppShortcuts
    }
}

fn status_of(o: InstallOutcome) -> ShortcutStatus {
    match o {
        InstallOutcome::Declared => ShortcutStatus::Declared,
        InstallOutcome::Installed => ShortcutStatus::Installed,
        InstallOutcome::Unsupported => ShortcutStatus::Unsupported,
        InstallOutcome::Failed => ShortcutStatus::Failed,
    }
}

fn send(sender: &CompletionSender<UrlSchemeShowState>, o: InstallOutcome) {
    let status = status_of(o);
    // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut UrlSchemeShowState| {
        apply_shortcut_status(s, status);
    }));
}

/// 登録を 1 回だけ起動し、結果を画面へ一度だけ届ける。
fn drive(
    state: &UrlSchemeShowState,
    sender: &CompletionSender<UrlSchemeShowState>,
    cell: &'static InstallCell,
    sc: &'static dyn AppShortcuts,
    on_worker: bool,
) {
    if state.shortcut_status() != ShortcutStatus::Unread {
        return;
    }
    // 画面を開き直したときは保持済みの結果を送るだけ（再登録しない）。
    if let Some(o) = cell.get() {
        send(sender, o);
        return;
    }
    if !cell.begin() {
        // 別スレッドで実行中。結果を取りに戻る。
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }
    let run = {
        let sender = sender.clone();
        move || {
            let result = sc.install();
            cell.finish(&result);
            if let Some(o) = cell.get() {
                send(&sender, o);
            }
        }
    };
    if on_worker {
        std::thread::spawn(run);
        sender.request_repaint_after(POLL_INTERVAL);
    } else {
        run();
    }
}

/// `url_scheme_show_effect` から呼ばれる入口。
pub fn drive_app_shortcuts(
    state: &UrlSchemeShowState,
    sender: &CompletionSender<UrlSchemeShowState>,
) {
    drive(state, sender, &CELL, platform(), ON_WORKER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::app_shortcuts::Registration;
    use fandhe_device::DeviceError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        calls: AtomicUsize,
        result: fn() -> Result<Registration, DeviceError>,
    }
    impl AppShortcuts for Fake {
        fn install(&self) -> Result<Registration, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<Registration, DeviceError>) -> &'static Fake {
        Box::leak(Box::new(Fake {
            calls: AtomicUsize::new(0),
            result,
        }))
    }
    fn cell() -> &'static InstallCell {
        Box::leak(Box::new(InstallCell::new()))
    }
    fn sender_with_channel() -> (
        CompletionSender<UrlSchemeShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<UrlSchemeShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn run_once(f: &'static Fake) -> ShortcutStatus {
        let (sender, rx) = sender_with_channel();
        let mut state = UrlSchemeShowState::default();
        drive(&state, &sender, cell(), f, false);
        rx.recv().expect("結果を 1 件送る")(&mut state);
        state.shortcut_status()
    }

    #[test]
    fn installs_once_and_does_not_reinstall_on_reopen() {
        let f = fake(|| Ok(Registration::Installed));
        let c = cell();
        let (sender, rx) = sender_with_channel();
        let mut state = UrlSchemeShowState::default();
        drive(&state, &sender, c, f, false);
        rx.recv().unwrap()(&mut state);
        assert_eq!(state.shortcut_status(), ShortcutStatus::Installed);
        // 送信済みなら何もしない。
        drive(&state, &sender, c, f, false);
        assert!(rx.try_recv().is_err());
        // 画面を開き直した（Unread の新しい状態）。保持済みを送り、再登録しない。
        let mut reopened = UrlSchemeShowState::default();
        drive(&reopened, &sender, c, f, false);
        rx.recv().unwrap()(&mut reopened);
        assert_eq!(reopened.shortcut_status(), ShortcutStatus::Installed);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn running_cell_is_not_started_twice() {
        let f = fake(|| Ok(Registration::Installed));
        let c = cell();
        assert!(c.begin());
        let (sender, rx) = sender_with_channel();
        let state = UrlSchemeShowState::default();
        drive(&state, &sender, c, f, false);
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn errors_never_become_installed() {
        assert_eq!(
            run_once(fake(|| Err(DeviceError::PermissionDenied("x")))),
            ShortcutStatus::Failed
        );
        assert_eq!(
            run_once(fake(|| Err(DeviceError::InvalidInput("x")))),
            ShortcutStatus::Failed
        );
        assert_eq!(
            run_once(fake(|| Err(DeviceError::Internal("x")))),
            ShortcutStatus::Failed
        );
        assert_eq!(
            run_once(fake(|| Err(DeviceError::Unsupported("x")))),
            ShortcutStatus::Unsupported
        );
        assert_eq!(
            run_once(fake(|| Ok(Registration::Declared))),
            ShortcutStatus::Declared
        );
    }

    #[test]
    fn nothing_is_sent_once_status_is_set() {
        let f = fake(|| Ok(Registration::Installed));
        let (sender, rx) = sender_with_channel();
        let mut state = UrlSchemeShowState::default();
        apply_shortcut_status(&mut state, ShortcutStatus::Failed);
        drive(&state, &sender, cell(), f, false);
        assert!(rx.try_recv().is_err());
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn worker_path_delivers_one_result() {
        let f = fake(|| Ok(Registration::Installed));
        let (sender, rx) = sender_with_channel();
        let mut state = UrlSchemeShowState::default();
        drive(&state, &sender, cell(), f, true);
        rx.recv_timeout(Duration::from_secs(5)).expect("1 件届く")(&mut state);
        assert_eq!(state.shortcut_status(), ShortcutStatus::Installed);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn unsupported_stub_is_not_success() {
        let f: &'static Fake = Box::leak(Box::new(Fake {
            calls: AtomicUsize::new(0),
            result: || UnsupportedAppShortcuts.install(),
        }));
        assert_eq!(run_once(f), ShortcutStatus::Unsupported);
    }
}
