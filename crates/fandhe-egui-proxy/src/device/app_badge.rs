//! Phase 11（親 #1141）型サンプル「バッジの設定」（Issue #1360）の effect ドライバ。
//! `fandhe_core::screens::app_badge_set`（端末依存ゼロ）の要求を観測し、値を
//! `fandhe_device::app_badge::BadgeValue` で検査してから OS へ渡し、結果を `CompletionSender`
//! 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、`device/photo_save.rs`
//! と同型）。契約と守る範囲は `fandhe_device::app_badge` のモジュール doc を参照。
//!
//! - iOS（#1361）/ Android（#1362）: コールバック型のため pull 型 [`bridge`]（`RequestSlot`
//!   の facade）で毎フレーム `drive_callback` が結果を反映する。後続 issue が埋めるまで
//!   各 OS モジュールのスタブが `Unsupported` を返す。**dispatch はここで最終形にしてあり、
//!   後続 issue は OS 別モジュールだけを埋める**
//! - macOS / Windows / それ以外: [`AppBadge`] を UI スレッドで同期に呼ぶ。Linux は
//!   `UnsupportedAppBadge` で「非対応」を表示する

use fandhe_core::screens::app_badge_set::{
    finish_set, mark_setting, take_request, AppBadgeSetState, CoreOutcome, OUT_OF_PROGRESS_MESSAGE,
    OUT_OF_RANGE_MESSAGE,
};
use fandhe_device::app_badge::{BadgeOutcome, BadgeValue};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。`value` は 0 の成功を「消去」の文言に分けるために使う。
fn to_outcome(
    value: u32,
    result: Result<BadgeOutcome, DeviceError>,
) -> Result<CoreOutcome, String> {
    match result {
        Ok(BadgeOutcome::Submitted) if value == 0 => Ok(CoreOutcome::Cleared),
        Ok(BadgeOutcome::Submitted) => Ok(CoreOutcome::Submitted),
        Ok(BadgeOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(DeviceError::PermissionDenied(_)) => Ok(CoreOutcome::Denied),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::InvalidInput("app_badge progress")) => {
            Err(OUT_OF_PROGRESS_MESSAGE.to_string())
        }
        Err(DeviceError::InvalidInput(_)) => Err(OUT_OF_RANGE_MESSAGE.to_string()),
        Err(_) => Err("OS のバッジ設定に失敗しました".to_string()),
    }
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// notification とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::app_badge_set::CoreOutcome;
    use fandhe_device::app_badge::BadgeOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(
        request_id: u64,
        value: u32,
        result: Result<BadgeOutcome, DeviceError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(value, result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる（`photo_save::bridge::reset` と同じ）。
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

/// OS へ設定を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_set(request_id: u64, value: BadgeValue) -> Result<(), DeviceError> {
    super::ios_app_badge::begin_set(request_id, value)
}

#[cfg(target_os = "android")]
fn begin_set(request_id: u64, value: BadgeValue) -> Result<(), DeviceError> {
    super::android_app_badge::begin_set(request_id, value)
}

/// iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(state: &AppBadgeSetState, sender: &CompletionSender<AppBadgeSetState>) {
    use fandhe_core::screens::app_badge_set::BadgePhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_setting));
        let request_id = bridge::begin();
        // 範囲外の値は OS へ渡さずその場で画面へ返す。
        let accepted = BadgeValue::new(request.value).and_then(|v| begin_set(request_id, v));
        if let Err(e) = accepted {
            bridge::deliver(request_id, request.value, Err(e));
        }
        return;
    }

    if state.phase != BadgePhase::Setting {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut AppBadgeSetState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != BadgePhase::Idle {
                finish_set(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `app_badge_set` 画面用に配線される effect ドライバ（iOS / Android）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn app_badge_set_effect(state: &AppBadgeSetState, sender: &CompletionSender<AppBadgeSetState>) {
    drive_callback(state, sender);
}

/// 同期経路（macOS・Windows・Linux）: 検査してから [`AppBadge`] を呼ぶ。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive(
    state: &AppBadgeSetState,
    sender: &CompletionSender<AppBadgeSetState>,
    badge: &dyn fandhe_device::app_badge::AppBadge,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_setting));
    let value = request.value;
    let result = BadgeValue::new(value).and_then(|v| badge.set(v));
    let outcome = to_outcome(value, result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut AppBadgeSetState| {
        finish_set(s, generation, outcome);
    }));
}

/// OS 別の [`AppBadge`] 実装を返す（macOS / Windows / それ以外）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn platform_app_badge() -> Box<dyn fandhe_device::app_badge::AppBadge> {
    #[cfg(target_os = "macos")]
    return super::macos_app_badge::implementation();
    #[cfg(target_os = "windows")]
    return super::windows_app_badge::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(fandhe_device::app_badge::UnsupportedAppBadge);
}

/// `registry.rs` から配線される effect ドライバ（macOS・Windows・Linux）。
///
/// ponytail: バッジ設定は OS への 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。
/// 戻らない OS 呼び出しが見つかったら、作業スレッド + bridge へ移す。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn app_badge_set_effect(state: &AppBadgeSetState, sender: &CompletionSender<AppBadgeSetState>) {
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_app_badge().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_outcome_covers_all_paths() {
        assert_eq!(
            to_outcome(3, Ok(BadgeOutcome::Submitted)),
            Ok(CoreOutcome::Submitted)
        );
        assert_eq!(
            to_outcome(0, Ok(BadgeOutcome::Submitted)),
            Ok(CoreOutcome::Cleared)
        );
        assert_eq!(
            to_outcome(3, Ok(BadgeOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(3, Err(DeviceError::PermissionDenied("app_badge"))),
            Ok(CoreOutcome::Denied)
        );
        assert_eq!(
            to_outcome(0, Err(DeviceError::PermissionDenied("app_badge"))),
            Ok(CoreOutcome::Denied)
        );
        assert_eq!(
            to_outcome(3, Err(DeviceError::Unsupported("app_badge"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(300, Err(DeviceError::InvalidInput("app_badge progress"))),
            Err(OUT_OF_PROGRESS_MESSAGE.to_string())
        );
        assert_eq!(
            to_outcome(30000, Err(DeviceError::InvalidInput("app_badge value"))),
            Err(OUT_OF_RANGE_MESSAGE.to_string())
        );
        assert!(to_outcome(3, Err(DeviceError::Internal("app_badge set"))).is_err());
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use fandhe_core::screens::app_badge_set::{clear, set, BadgePhase};
        use fandhe_device::app_badge::{AppBadge, UnsupportedAppBadge};
        use std::sync::atomic::{AtomicU32, Ordering};

        /// 呼び出し回数を数え、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            result: fn() -> Result<BadgeOutcome, DeviceError>,
        }

        impl AppBadge for Fake {
            fn set(&self, _v: BadgeValue) -> Result<BadgeOutcome, DeviceError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<BadgeOutcome, DeviceError>) -> Fake {
            Fake {
                calls: AtomicU32::new(0),
                result,
            }
        }

        /// 入力を押下して 1 回ドライバを回し、反映後の状態を返す。
        fn run(input: &str, clear_btn: bool, badge: &dyn AppBadge) -> AppBadgeSetState {
            let mut s = AppBadgeSetState {
                input: input.into(),
                ..Default::default()
            };
            if clear_btn {
                clear(&mut s);
            } else {
                set(&mut s);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, badge);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        #[test]
        fn no_request_never_calls_os() {
            let f = fake(|| Ok(BadgeOutcome::Submitted));
            let s = AppBadgeSetState::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, &f);
            assert!(rx.try_recv().is_err());
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn submitted_and_cleared_are_shown() {
            let f = fake(|| Ok(BadgeOutcome::Submitted));
            let s = run("5", false, &f);
            assert_eq!(s.phase, BadgePhase::Idle);
            assert_eq!(
                s.status,
                fandhe_core::screens::app_badge_set::SUBMITTED_STATUS
            );
            let s = run("", true, &f);
            assert_eq!(
                s.status,
                fandhe_core::screens::app_badge_set::CLEARED_STATUS
            );
        }

        #[test]
        fn cancelled_is_not_a_success_text() {
            let f = fake(|| Ok(BadgeOutcome::Cancelled));
            let s = run("5", false, &f);
            assert_eq!(
                s.status,
                fandhe_core::screens::app_badge_set::CANCELLED_STATUS
            );
            assert!(s.error.is_empty());
        }

        #[test]
        fn denied_and_unsupported_go_to_error() {
            let f = fake(|| Err(DeviceError::PermissionDenied("app_badge")));
            let s = run("5", false, &f);
            assert_eq!(s.error, fandhe_core::screens::app_badge_set::DENIED_STATUS);
            assert!(s.status.is_empty());
            let s = run("5", false, &UnsupportedAppBadge);
            assert_eq!(
                s.error,
                fandhe_core::screens::app_badge_set::UNSUPPORTED_STATUS
            );
            assert!(s.status.is_empty());
        }

        #[test]
        fn out_of_range_never_reaches_os() {
            let f = fake(|| Ok(BadgeOutcome::Submitted));
            let s = run("10000", false, &f);
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
            assert!(s.error.contains(OUT_OF_RANGE_MESSAGE));
            assert!(s.status.is_empty());
        }

        #[test]
        fn stale_generation_completion_is_dropped() {
            let mut s = AppBadgeSetState {
                input: "5".into(),
                ..Default::default()
            };
            set(&mut s);
            mark_setting(&mut s);
            let stale = s.generation.wrapping_sub(1);
            finish_set(&mut s, stale, Ok(CoreOutcome::Submitted));
            assert_eq!(s.phase, BadgePhase::Setting);
            assert!(s.status.is_empty());
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
        assert!(!bridge::deliver(id + 1000, 1, Ok(BadgeOutcome::Submitted)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(
            id,
            1,
            Err(DeviceError::PermissionDenied("app_badge"))
        ));
        assert!(!bridge::deliver(id, 1, Ok(BadgeOutcome::Submitted)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(CoreOutcome::Denied)));
        bridge::SLOT.clear(id);
    }
}
