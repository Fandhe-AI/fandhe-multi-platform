//! Phase 11（親 #185）型サンプル「MDM の管理アプリ設定」（Issue #944）の effect ドライバ。
//! `fandhe_core::screens::managed_app_config_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::managed_app_config::ManagedAppConfig` で固定 3 キーを読んで `CompletionSender`
//! 経由で結果を返す（`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)`
//! として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::managed_app_config`。
//!
//! # スレッド
//!
//! Android はディスク I/O を伴うため、OS を問わずワーカースレッドで読む（`wifi_info.rs` と同型）。
//! 完了は画面状態の世代照合つきで入れる（古い世代は捨てられる）。`CompletionSender::send` が
//! 再描画を起こすため、iOS で別に `request_repaint` を呼ぶ必要は無い。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを
//! 埋める（macOS: `macos_managed_app_config`（#944）、iOS: `ios_managed_app_config`（#945）、
//! Android: `android_managed_app_config`（#946））。それ以外は `UnsupportedManagedAppConfig` 固定で、
//! ダミーの成功実装へは倒さない。

use std::sync::Arc;

use fandhe_core::screens::managed_app_config_show::{
    finish_read, mark_reading, take_request, KeyView, ManagedAppConfigShowState, ReadFailure,
    ReadingView,
};
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
use fandhe_device::managed_app_config::UnsupportedManagedAppConfig;
use fandhe_device::managed_app_config::{KeyState, ManagedAppConfig, Report};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 失敗時に画面へ出す固定文言（OS の生コードは出さない）。
const FAILED_TEXT: &str = "管理アプリ設定の読み取り";

/// 現在のプラットフォームに応じた `ManagedAppConfig` 実装を返す。
fn implementation() -> Arc<dyn ManagedAppConfig> {
    #[cfg(target_os = "macos")]
    {
        super::macos_managed_app_config::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_managed_app_config::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_managed_app_config::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        Arc::new(UnsupportedManagedAppConfig)
    }
}

/// `DeviceError` を画面の失敗種別へ写す。その他は固定文言へ畳む（security.md A09）。
fn to_failure(e: DeviceError) -> ReadFailure {
    match e {
        DeviceError::Unsupported(_) => ReadFailure::Unsupported,
        _ => ReadFailure::Failed(FAILED_TEXT.to_string()),
    }
}

fn to_key(k: KeyState) -> KeyView {
    match k {
        KeyState::Unset => KeyView::Unset,
        KeyState::Bool(b) => KeyView::Bool(b),
        KeyState::Integer(n) => KeyView::Integer(n),
        KeyState::Text { chars } => KeyView::Text { chars },
        KeyState::WrongType => KeyView::WrongType,
    }
}

fn to_view(r: Report) -> ReadingView {
    match r {
        Report::NoConfig => ReadingView::NoConfig,
        Report::Config { keys, other_keys } => ReadingView::Config {
            boolean: to_key(keys.boolean),
            integer: to_key(keys.integer),
            text: to_key(keys.text),
            other_keys,
        },
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &ManagedAppConfigShowState,
    sender: &CompletionSender<ManagedAppConfigShowState>,
    imp: Arc<dyn ManagedAppConfig>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_reading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("managed-app-config-read".to_string())
        .spawn(move || {
            let result = imp.read().map(to_view).map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut ManagedAppConfigShowState| {
                finish_read(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut ManagedAppConfigShowState| {
            finish_read(
                s,
                generation,
                Err(ReadFailure::Failed(FAILED_TEXT.to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `managed_app_config_show` 画面用に配線される effect ドライバ。
pub fn managed_app_config_show_effect(
    state: &ManagedAppConfigShowState,
    sender: &CompletionSender<ManagedAppConfigShowState>,
) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::managed_app_config_show::{
        read, ReadPhase, FAILED_PREFIX, UNSUPPORTED_STATUS,
    };
    use fandhe_device::managed_app_config::{
        ConfigKeys, UnsupportedManagedAppConfig as Unsupported, FEATURE, READ_FAILED,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<ManagedAppConfigShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<ManagedAppConfigShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<Report, fn() -> DeviceError>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn ok(r: Report) -> Arc<Self> {
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

    impl ManagedAppConfig for Fake {
        fn read(&self) -> Result<Report, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.map_err(|f| f())
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_reading と結果）をすべて適用した状態を返す。
    fn run(imp: Arc<dyn ManagedAppConfig>) -> ManagedAppConfigShowState {
        let (sender, rx) = sender_with_channel();
        let mut state = ManagedAppConfigShowState::default();
        read(&mut state);
        drive(&state, &sender, imp);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("mark_reading と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::ok(Report::NoConfig);
        drive(&ManagedAppConfigShowState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn no_config_is_success() {
        let state = run(Fake::ok(Report::NoConfig));
        assert_eq!(state.reading, Some(ReadingView::NoConfig));
        assert_eq!(state.error, "");
        assert_eq!(state.phase, ReadPhase::Idle);
    }

    #[test]
    fn config_is_passed_through() {
        let state = run(Fake::ok(Report::Config {
            keys: ConfigKeys {
                boolean: KeyState::Bool(true),
                integer: KeyState::WrongType,
                text: KeyState::Text { chars: 3 },
            },
            other_keys: Some(1),
        }));
        assert_eq!(
            state.reading,
            Some(ReadingView::Config {
                boolean: KeyView::Bool(true),
                integer: KeyView::WrongType,
                text: KeyView::Text { chars: 3 },
                other_keys: Some(1),
            })
        );
    }

    #[test]
    fn each_failure_is_error_not_success() {
        let fixed = format!("{FAILED_PREFIX}{FAILED_TEXT}");
        let cases: [(fn() -> DeviceError, String); 4] = [
            (
                || DeviceError::Unsupported(FEATURE),
                UNSUPPORTED_STATUS.into(),
            ),
            (|| DeviceError::Internal(READ_FAILED), fixed.clone()),
            (|| DeviceError::PermissionDenied(FEATURE), fixed.clone()),
            (|| DeviceError::Cancelled(FEATURE), fixed),
        ];
        for (make, expected) in cases {
            let state = run(Fake::err(make));
            assert_eq!(state.reading, None);
            assert_eq!(state.error, expected);
        }
    }

    #[test]
    fn second_drive_while_reading_does_not_call_os_twice() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::ok(Report::NoConfig);
        let mut state = ManagedAppConfigShowState::default();
        read(&mut state);
        drive(&state, &sender, fake.clone());
        // mark_reading を適用してから再度 drive しても要求は無い。
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        drive(&state, &sender, fake.clone());
        // 完了を待つ。
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_completion_does_not_overwrite_new_request() {
        let (sender, rx) = sender_with_channel();
        let mut state = ManagedAppConfigShowState::default();
        read(&mut state);
        drive(&state, &sender, Fake::ok(Report::NoConfig));
        // 完了を適用する前に画面側が次の世代へ進んだ状況を、世代違いの完了で再現する。
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        state.generation += 1;
        rx.recv_timeout(Duration::from_secs(5)).unwrap()(&mut state);
        assert_eq!(state.reading, None);
        assert_eq!(state.phase, ReadPhase::Reading);
    }

    #[test]
    fn unsupported_impl_never_succeeds() {
        let state = run(Arc::new(Unsupported));
        assert_eq!(state.reading, None);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    #[test]
    fn fallback_implementation_is_unsupported() {
        assert!(matches!(
            implementation().read(),
            Err(DeviceError::Unsupported(FEATURE))
        ));
    }
}
