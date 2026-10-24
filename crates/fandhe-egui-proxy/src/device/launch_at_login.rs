//! Phase 11 の拡充ループ（親 #134）型サンプル「ログイン時の自動起動」（Issue #696）の effect
//! ドライバ。`fandhe_core::screens::launch_at_login_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::launch_at_login::LaunchAtLogin` を別スレッドで呼んで結果を `CompletionSender`
//! 経由で返す（`crate::screens::registry` の `effect` 区画から呼ばれる契約、
//! `device/url_scheme.rs::drive_registration` と同型）。契約と守る範囲は
//! `fandhe_device::launch_at_login` のモジュール doc を参照。
//!
//! **dispatch はここで最終形にしてあり、後続 issue は OS 別モジュールだけを埋める**。
//!
//! - macOS: [`super::macos_launch_at_login`]（`SMAppService.mainApp`）
//! - Windows: [`super::windows_launch_at_login`]（骨格のスタブ。Issue #697 が埋める）
//! - それ以外: `UnsupportedLaunchAtLogin` 固定で、画面には「未対応」と出る
//!
//! # 流れ
//!
//! 1. 画面の OS 区分を一度だけ注入する
//! 2. `take_request` で未発送の要求を観測したら、先に `mark_in_flight` を送って二重発行を防ぐ
//! 3. 別スレッドで OS を呼び、`finish_action` を送る（世代の照合は画面側が行う）
//!
//! 継続ポーリングはしない（結果の `send` が再描画を起こす）。OS 設定側の変更は画面の
//! 「状態を読み直す」で拾う（守らない範囲: 時間差）。

use std::sync::OnceLock;

use fandhe_core::screens::launch_at_login_show::{
    apply_platform, finish_action, mark_in_flight, take_request, Action, ActionFailure,
    ActionOutcome, LaunchAtLoginShowState, LoginItemState, Platform,
};
#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
use fandhe_device::launch_at_login::UnsupportedLaunchAtLogin;
use fandhe_device::launch_at_login::{LaunchAtLogin, LoginItemStatus};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在の OS の実装（macOS は `SMAppService`、Windows は骨格のスタブ、他は `Unsupported` 固定）。
fn platform_launch_at_login() -> &'static dyn LaunchAtLogin {
    static IMPL: OnceLock<Box<dyn LaunchAtLogin>> = OnceLock::new();
    IMPL.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            super::macos_launch_at_login::implementation()
        }
        #[cfg(target_os = "windows")]
        {
            super::windows_launch_at_login::implementation()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Box::new(UnsupportedLaunchAtLogin)
        }
    })
    .as_ref()
}

/// 画面へ注入する OS 区分。
fn platform_kind() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Unknown
    }
}

/// 契約上の状態 → 画面用の写し（1 対 1）。
fn to_state(status: LoginItemStatus) -> LoginItemState {
    match status {
        LoginItemStatus::Enabled => LoginItemState::Enabled,
        LoginItemStatus::NotRegistered => LoginItemState::NotRegistered,
        LoginItemStatus::RequiresApproval => LoginItemState::RequiresApproval,
        LoginItemStatus::DisabledByUser => LoginItemState::DisabledByUser,
        LoginItemStatus::RegisteredElsewhere => LoginItemState::RegisteredElsewhere,
        LoginItemStatus::Unknown => LoginItemState::Unknown,
    }
}

/// `DeviceError` を画面用の失敗へ写像する。内部識別子だけを含み、パスや OS の文言は混ざらない
/// （契約表 8）。
fn to_failure(e: DeviceError) -> ActionFailure {
    match e {
        DeviceError::Unsupported(_) => ActionFailure::Unsupported,
        DeviceError::PermissionDenied(_) => ActionFailure::PermissionDenied,
        other => ActionFailure::Failed(format!("失敗しました: {other}")),
    }
}

/// 1 回分のドライブ。OS 区分を一度だけ注入し、未発送の要求があれば別スレッドで処理する。
fn drive(
    state: &LaunchAtLoginShowState,
    sender: &CompletionSender<LaunchAtLoginShowState>,
    imp: &'static dyn LaunchAtLogin,
) {
    let kind = platform_kind();
    if kind != Platform::Unknown && state.platform() == Platform::Unknown {
        // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい。
        let _ = sender.send(Box::new(move |s: &mut LaunchAtLoginShowState| {
            apply_platform(s, kind);
        }));
    }
    let Some(request) = take_request(state) else {
        return;
    };
    // 起動済みマーキング（二重起動防止、`url_scheme::drive_registration` と同型）。
    let _ = sender.send(Box::new(mark_in_flight));
    let sender = sender.clone();
    std::thread::spawn(move || {
        let result = match request.action {
            Action::Refresh => imp.status().map(|s| ActionOutcome::Status(to_state(s))),
            Action::Enable => imp.enable().map(|s| ActionOutcome::Status(to_state(s))),
            Action::Disable => imp.disable().map(|s| ActionOutcome::Status(to_state(s))),
            Action::OpenSettings => imp
                .open_settings()
                .map(|()| ActionOutcome::SettingsRequested),
        }
        .map_err(to_failure);
        let _ = sender.send(Box::new(move |s: &mut LaunchAtLoginShowState| {
            finish_action(s, request.generation, request.action, result);
        }));
    });
}

/// `registry.rs` の `effect` 区画から `launch_at_login_show` 画面用に配線される effect ドライバ。
pub fn launch_at_login_show_effect(
    state: &LaunchAtLoginShowState,
    sender: &CompletionSender<LaunchAtLoginShowState>,
) {
    drive(state, sender, platform_launch_at_login());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::launch_at_login_show as show;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 呼び出し回数を数え、`enable` の結果を差し替えられる偽実装。
    struct Fake {
        calls: AtomicUsize,
        enable_result: fn() -> Result<LoginItemStatus, DeviceError>,
    }
    impl LaunchAtLogin for Fake {
        fn status(&self) -> Result<LoginItemStatus, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(LoginItemStatus::NotRegistered)
        }
        fn enable(&self) -> Result<LoginItemStatus, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.enable_result)()
        }
        fn disable(&self) -> Result<LoginItemStatus, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(LoginItemStatus::NotRegistered)
        }
        fn open_settings(&self) -> Result<(), DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn leak(f: Fake) -> &'static Fake {
        Box::leak(Box::new(f))
    }

    fn sender_with_channel() -> (
        CompletionSender<LaunchAtLoginShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<LaunchAtLoginShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// `drive` を 1 回呼び、届いた完了（OS 区分の注入・起動済みマーク・結果）をすべて適用する。
    fn run_drive(
        state: &mut LaunchAtLoginShowState,
        sender: &CompletionSender<LaunchAtLoginShowState>,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<LaunchAtLoginShowState>>,
        imp: &'static dyn LaunchAtLogin,
    ) {
        let inject = platform_kind() != Platform::Unknown && state.platform() == Platform::Unknown;
        let request = show::take_request(state).is_some();
        drive(state, sender, imp);
        let expected = usize::from(inject) + if request { 2 } else { 0 };
        for _ in 0..expected {
            let c = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("完了が届く");
            c(state);
        }
    }

    #[test]
    fn one_os_call_per_request() {
        let fake = leak(Fake {
            calls: AtomicUsize::new(0),
            enable_result: || Ok(LoginItemStatus::Enabled),
        });
        let (sender, rx) = sender_with_channel();
        let mut state = LaunchAtLoginShowState::default();

        // 初回の読み取り要求。
        run_drive(&mut state, &sender, &rx, fake);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);

        // 要求が無ければ OS は呼ばれない。
        drive(&state, &sender, fake);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);

        show::enable(&mut state);
        run_drive(&mut state, &sender, &rx, fake);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
        assert_eq!(show::result_text(&state), "自動起動を有効にしました");
    }

    #[test]
    fn permission_denied_is_error_without_success_text() {
        let fake = leak(Fake {
            calls: AtomicUsize::new(0),
            enable_result: || Err(DeviceError::PermissionDenied("launch_at_login")),
        });
        let (sender, rx) = sender_with_channel();
        let mut state = LaunchAtLoginShowState::default();
        run_drive(&mut state, &sender, &rx, fake);
        show::enable(&mut state);
        run_drive(&mut state, &sender, &rx, fake);
        assert_eq!(show::error_text(&state), "OS に拒否されました");
        assert_eq!(show::result_text(&state), "");
    }

    #[test]
    fn requires_approval_is_not_reported_as_enabled() {
        let fake = leak(Fake {
            calls: AtomicUsize::new(0),
            enable_result: || Ok(LoginItemStatus::RequiresApproval),
        });
        let (sender, rx) = sender_with_channel();
        let mut state = LaunchAtLoginShowState::default();
        run_drive(&mut state, &sender, &rx, fake);
        show::enable(&mut state);
        run_drive(&mut state, &sender, &rx, fake);
        assert!(!show::result_text(&state).contains("有効にしました"));
        assert!(!show::state_text(&state).contains("有効です"));
    }

    #[test]
    fn unsupported_impl_marks_screen_unsupported() {
        let (sender, rx) = sender_with_channel();
        let mut state = LaunchAtLoginShowState::default();
        static U: UnsupportedLaunchAtLogin = UnsupportedLaunchAtLogin;
        run_drive(&mut state, &sender, &rx, &U);
        assert!(state.is_unsupported());
        assert_eq!(show::state_text(&state), "この OS では未対応です");
    }

    #[test]
    fn stale_completion_is_dropped() {
        let (sender, rx) = sender_with_channel();
        let fake = leak(Fake {
            calls: AtomicUsize::new(0),
            enable_result: || Ok(LoginItemStatus::Enabled),
        });
        // 世代 1 の読み取りを発送し、完了はまだ適用しない。
        let state = LaunchAtLoginShowState::default();
        drive(&state, &sender, fake);
        let mut pending = Vec::new();
        let expected = 2 + usize::from(platform_kind() != Platform::Unknown);
        for _ in 0..expected {
            pending.push(
                rx.recv_timeout(std::time::Duration::from_secs(5))
                    .expect("届く"),
            );
        }
        // 別の状態: 世代 1 を終えて世代 2 を要求した直後。
        let mut other = LaunchAtLoginShowState::default();
        show::mark_in_flight(&mut other);
        show::finish_action(
            &mut other,
            1,
            Action::Refresh,
            Ok(ActionOutcome::Status(LoginItemState::NotRegistered)),
        );
        show::refresh(&mut other);
        for c in pending {
            c(&mut other);
        }
        // 古い完了（世代 1）は世代 2 の要求を完了させず、処理中のまま（ボタンが出ない）。
        let screen = fandhe_core::render_headless(&show::current_screen(&other), &other).join("\n");
        assert!(!screen.contains("状態を読み直す"));
    }

    #[test]
    fn fallback_never_succeeds() {
        let u = UnsupportedLaunchAtLogin;
        assert!(u.status().is_err());
        assert!(u.enable().is_err());
        assert!(u.disable().is_err());
        assert!(u.open_settings().is_err());
    }

    #[test]
    fn state_mapping_is_one_to_one() {
        assert_eq!(to_state(LoginItemStatus::Enabled), LoginItemState::Enabled);
        assert_eq!(
            to_state(LoginItemStatus::DisabledByUser),
            LoginItemState::DisabledByUser
        );
        assert!(matches!(
            to_failure(DeviceError::Internal("x")),
            ActionFailure::Failed(_)
        ));
    }
}
