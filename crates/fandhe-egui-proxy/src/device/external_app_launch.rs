//! Phase 11（親 #176）型サンプル「電話発信・地図など他アプリの起動」（Issue #861）の effect ドライバ。
//! `fandhe_core::screens::external_app_launch_show`（端末依存ゼロ）の要求を観測し、操作を
//! `fandhe_device::external_app_launch::LaunchTarget` へ写して OS へ渡し、結果を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `device/open_system_settings.rs` と同型）。契約と守る範囲は `fandhe_device::external_app_launch` の
//! モジュール doc を参照。
//!
//! - macOS・Windows: [`ExternalAppLaunch`] を UI スレッドで同期に呼ぶ
//! - iOS / Android: コールバック型のため pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム
//!   `drive_callback` が結果を反映する。iOS は実装済み（#862）、Android は (b-android) が埋めるまでスタブ
//!   （`Unsupported`）。**dispatch はここで最終形にしてあり、後続 issue は OS 別モジュールだけを埋める**
//! - Linux・それ以外: `UnsupportedExternalAppLaunch` で「非対応」を表示する（Web は `fandhe-web-dom`）

use fandhe_core::screens::external_app_launch_show::{
    CoreOutcome, ExternalAppLaunchState, ExternalLaunchFailure, LaunchKind,
};
use fandhe_device::external_app_launch::{LaunchOutcome, LaunchTarget};
use fandhe_device::DeviceError;

#[cfg(any(target_os = "ios", target_os = "android"))]
use fandhe_core::screens::external_app_launch_show::take_request;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_core::screens::external_app_launch_show::{finish_launch, mark_launching, take_request};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::external_app_launch::ExternalAppLaunch;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする（security.md A09）。
fn to_outcome(
    result: Result<LaunchOutcome, DeviceError>,
) -> Result<CoreOutcome, ExternalLaunchFailure> {
    match result {
        Ok(LaunchOutcome::HandedOff) => Ok(CoreOutcome::HandedOff),
        Ok(LaunchOutcome::NoHandler) => Ok(CoreOutcome::NoHandler),
        Ok(LaunchOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(LaunchOutcome::TargetUnavailable) => Ok(CoreOutcome::TargetUnavailable),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(ExternalLaunchFailure::Failed(
            "他のアプリを開けませんでした".to_string(),
        )),
    }
}

/// 画面の操作を device の操作へ写す。
fn to_target(kind: LaunchKind) -> LaunchTarget {
    match kind {
        LaunchKind::Phone => LaunchTarget::Phone,
        LaunchKind::Map => LaunchTarget::Map,
    }
}

/// iOS の結果コード表（SSOT。`.m` の定数と一致させる）。未知のコードは失敗に倒す。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))] // iOS の結果受け口が使う
pub(crate) fn outcome_from_ios_code(code: i32) -> Result<LaunchOutcome, DeviceError> {
    match code {
        0 => Ok(LaunchOutcome::HandedOff),
        1 => Ok(LaunchOutcome::NoHandler),
        2 => Err(DeviceError::Internal("external app present")),
        _ => Err(DeviceError::Internal("external app outcome")),
    }
}

/// Android の結果コード表（SSOT。Java 側の定数と一致させる）。未知のコードは失敗に倒す。
#[allow(dead_code)] // (b-android) が結果受け口から使う
pub(crate) fn outcome_from_android_code(code: i32) -> Result<LaunchOutcome, DeviceError> {
    match code {
        0 => Ok(LaunchOutcome::HandedOff),
        1 => Ok(LaunchOutcome::NoHandler),
        2 => Err(DeviceError::Internal("external app background")),
        3 => Err(DeviceError::Internal("external app present")),
        _ => Err(DeviceError::Internal("external app outcome")),
    }
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::external_app_launch_show::{CoreOutcome, ExternalLaunchFailure};
    use fandhe_device::external_app_launch::LaunchOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, ExternalLaunchFailure>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    #[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))] // iOS（#862）/ Android（b-android）の受け口が使う
    pub(crate) fn deliver(request_id: u64, result: Result<LaunchOutcome, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

/// OS へ起動を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_launch(request_id: u64, target: LaunchTarget) -> Result<(), DeviceError> {
    super::ios_external_app_launch::begin_launch(request_id, target)
}

#[cfg(target_os = "android")]
fn begin_launch(request_id: u64, target: LaunchTarget) -> Result<(), DeviceError> {
    super::android_external_app_launch::begin_launch(request_id, target)
}

/// iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(
    state: &ExternalAppLaunchState,
    sender: &CompletionSender<ExternalAppLaunchState>,
) {
    use fandhe_core::screens::external_app_launch_show::{
        finish_launch, mark_launching, ExternalLaunchPhase,
    };

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_launching));
        let request_id = bridge::begin();
        if let Err(e) = begin_launch(request_id, to_target(view.kind)) {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != ExternalLaunchPhase::Launching {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut ExternalAppLaunchState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != ExternalLaunchPhase::Idle {
                finish_launch(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `external_app_launch_show` 画面用に配線される effect ドライバ（iOS / Android）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn external_app_launch_show_effect(
    state: &ExternalAppLaunchState,
    sender: &CompletionSender<ExternalAppLaunchState>,
) {
    drive_callback(state, sender);
}

/// 同期経路（macOS・Windows・Linux）: [`ExternalAppLaunch`] を呼ぶ。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive(
    state: &ExternalAppLaunchState,
    sender: &CompletionSender<ExternalAppLaunchState>,
    launcher: &dyn ExternalAppLaunch,
) {
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_launching));
    let outcome = to_outcome(launcher.launch(to_target(view.kind)));
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut ExternalAppLaunchState| {
        finish_launch(s, generation, outcome);
    }));
}

/// OS 別の [`ExternalAppLaunch`] 実装を返す。macOS・Windows 以外は fallback に入る。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn platform_launch() -> Box<dyn ExternalAppLaunch> {
    #[cfg(target_os = "macos")]
    return super::macos_external_app_launch::implementation();
    #[cfg(target_os = "windows")]
    return super::windows_external_app_launch::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(fandhe_device::external_app_launch::UnsupportedExternalAppLaunch);
}

/// `registry.rs` から配線される effect ドライバ（macOS・Windows・Linux）。
///
/// ponytail: 他アプリの起動は OS への URI 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ
/// （macOS は main thread 必須）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn external_app_launch_show_effect(
    state: &ExternalAppLaunchState,
    sender: &CompletionSender<ExternalAppLaunchState>,
) {
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_launch().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_outcome_covers_all_arms() {
        assert_eq!(
            to_outcome(Ok(LaunchOutcome::HandedOff)),
            Ok(CoreOutcome::HandedOff)
        );
        assert_eq!(
            to_outcome(Ok(LaunchOutcome::NoHandler)),
            Ok(CoreOutcome::NoHandler)
        );
        assert_eq!(
            to_outcome(Ok(LaunchOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Ok(LaunchOutcome::TargetUnavailable)),
            Ok(CoreOutcome::TargetUnavailable)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("external app launch"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("external app os"))),
            Err(ExternalLaunchFailure::Failed(m)) if !m.contains("os")
        ));
    }

    #[test]
    fn code_tables_never_turn_unknown_codes_into_success() {
        for c in [-1, 3, 99, i32::MAX] {
            assert!(outcome_from_ios_code(c).is_err());
        }
        for c in [-1, 4, 99, i32::MAX] {
            assert!(outcome_from_android_code(c).is_err());
        }
        assert_eq!(outcome_from_ios_code(0).unwrap(), LaunchOutcome::HandedOff);
        assert_eq!(outcome_from_ios_code(1).unwrap(), LaunchOutcome::NoHandler);
        assert!(outcome_from_ios_code(2).is_err());
        assert_eq!(
            outcome_from_android_code(0).unwrap(),
            LaunchOutcome::HandedOff
        );
        assert_eq!(
            outcome_from_android_code(1).unwrap(),
            LaunchOutcome::NoHandler
        );
        assert!(outcome_from_android_code(2).is_err() && outcome_from_android_code(3).is_err());
    }

    #[test]
    fn to_target_maps_each_kind() {
        assert_eq!(to_target(LaunchKind::Phone), LaunchTarget::Phone);
        assert_eq!(to_target(LaunchKind::Map), LaunchTarget::Map);
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use fandhe_core::screens::external_app_launch_show as screen;
        use fandhe_device::external_app_launch::UnsupportedExternalAppLaunch;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Mutex;

        type LaunchFn = fn() -> Result<LaunchOutcome, DeviceError>;

        /// 呼び出し回数と受けた操作を記録し、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            last: Mutex<Option<LaunchTarget>>,
            result: fn() -> Result<LaunchOutcome, DeviceError>,
        }

        impl ExternalAppLaunch for Fake {
            fn launch(&self, target: LaunchTarget) -> Result<LaunchOutcome, DeviceError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                *self.last.lock().unwrap() = Some(target);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<LaunchOutcome, DeviceError>) -> Fake {
            Fake {
                calls: AtomicU32::new(0),
                last: Mutex::new(None),
                result,
            }
        }

        /// 押下して 1 回ドライバを回し、反映後の状態を返す。
        fn run(phone: bool, launcher: &dyn ExternalAppLaunch) -> ExternalAppLaunchState {
            let mut s = ExternalAppLaunchState::default();
            if phone {
                screen::open_phone(&mut s);
            } else {
                screen::open_map(&mut s);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, launcher);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        #[test]
        fn no_request_never_calls_os() {
            let f = fake(|| Ok(LaunchOutcome::HandedOff));
            let s = ExternalAppLaunchState::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, &f);
            assert!(rx.try_recv().is_err());
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn each_outcome_shows_its_own_status() {
            let cases: [(LaunchFn, &str); 4] = [
                (|| Ok(LaunchOutcome::HandedOff), screen::HANDED_OFF_STATUS),
                (|| Ok(LaunchOutcome::NoHandler), screen::NO_HANDLER_STATUS),
                (|| Ok(LaunchOutcome::Cancelled), screen::CANCELLED_STATUS),
                (
                    || Ok(LaunchOutcome::TargetUnavailable),
                    screen::TARGET_UNAVAILABLE_STATUS,
                ),
            ];
            for (result, expected) in cases {
                let s = run(true, &fake(result));
                assert_eq!(s.status, expected);
                assert!(s.error.is_empty());
            }
        }

        #[test]
        fn target_passed_to_os_matches_pressed_button() {
            let f = fake(|| Ok(LaunchOutcome::HandedOff));
            run(true, &f);
            assert_eq!(*f.last.lock().unwrap(), Some(LaunchTarget::Phone));
            run(false, &f);
            assert_eq!(*f.last.lock().unwrap(), Some(LaunchTarget::Map));
            assert_eq!(f.calls.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn unsupported_shows_unsupported_not_success() {
            let s = run(false, &UnsupportedExternalAppLaunch);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
            assert_ne!(s.status, screen::HANDED_OFF_STATUS);
        }

        #[test]
        fn failure_goes_to_error_with_fixed_label() {
            let f = fake(|| Err(DeviceError::Internal("external app shell")));
            let s = run(false, &f);
            assert!(s.status.is_empty());
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
            assert!(!s.error.contains("shell"));
        }

        #[test]
        fn stale_generation_completion_is_dropped() {
            let mut s = ExternalAppLaunchState::default();
            screen::open_phone(&mut s);
            screen::mark_launching(&mut s);
            let stale = s.generation.wrapping_sub(1);
            screen::finish_launch(&mut s, stale, Ok(CoreOutcome::HandedOff));
            assert!(s.status.is_empty());
            assert_eq!(s.phase, screen::ExternalLaunchPhase::Launching);
        }
    }
}

#[cfg(all(test, any(target_os = "ios", target_os = "android")))]
mod bridge_tests {
    use super::bridge;
    use super::*;

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id + 1000, Ok(LaunchOutcome::HandedOff)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, Ok(LaunchOutcome::NoHandler)));
        assert!(!bridge::deliver(id, Ok(LaunchOutcome::HandedOff)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(CoreOutcome::NoHandler)));
        bridge::SLOT.clear(id);
    }
}
