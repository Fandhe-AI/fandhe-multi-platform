//! Phase 11（親 #171）型サンプル「OS の設定画面を開く」（Issue #798）の effect ドライバ。
//! `fandhe_core::screens::open_system_settings_show`（端末依存ゼロ）の要求を観測し、頁を
//! `fandhe_device::open_system_settings::SettingsPage` へ写して OS へ渡し、結果を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `device/compose_mail_sms.rs` と同型）。契約と守る範囲は `fandhe_device::open_system_settings` の
//! モジュール doc を参照。
//!
//! - Windows: [`OpenSystemSettings`] を UI スレッドで同期に呼ぶ
//! - iOS / Android: コールバック型のため pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム
//!   `drive_callback` が結果を反映する。OS 別モジュールは #799 / #800 が埋めるまでスタブ
//!   （`Unsupported`）。**dispatch はここで最終形にしてあり、後続 issue は OS 別モジュールだけを埋める**
//! - macOS・Linux・それ以外: `UnsupportedOpenSystemSettings` で「非対応」を表示する（Web は対象外）

use fandhe_core::screens::open_system_settings_show::{
    CoreOutcome, OpenSettingsFailure, OpenSystemSettingsState, SettingsTarget,
};
use fandhe_device::open_system_settings::{OpenSettingsOutcome, SettingsPage};
use fandhe_device::DeviceError;

#[cfg(any(target_os = "ios", target_os = "android"))]
use fandhe_core::screens::open_system_settings_show::take_request;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_core::screens::open_system_settings_show::{finish_open, mark_opening, take_request};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::open_system_settings::OpenSystemSettings;

use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする（security.md A09）。
fn to_outcome(
    result: Result<OpenSettingsOutcome, DeviceError>,
) -> Result<CoreOutcome, OpenSettingsFailure> {
    match result {
        Ok(OpenSettingsOutcome::HandedOff) => Ok(CoreOutcome::HandedOff),
        Ok(OpenSettingsOutcome::NotOpened) => Ok(CoreOutcome::NotOpened),
        Ok(OpenSettingsOutcome::PageUnavailable) => Ok(CoreOutcome::PageUnavailable),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(OpenSettingsFailure::Failed(
            "OS の設定画面を開けませんでした".to_string(),
        )),
    }
}

/// 画面の頁を device の頁へ写す。
#[cfg_attr(
    not(any(target_os = "ios", target_os = "android", test)),
    allow(dead_code)
)]
fn to_page(target: SettingsTarget) -> SettingsPage {
    match target {
        SettingsTarget::App => SettingsPage::AppSettings,
        SettingsTarget::Notifications => SettingsPage::Notifications,
    }
}

/// iOS の結果コード表（SSOT。`.m` の定数と一致させる）。未知のコードは失敗に倒す。
#[allow(dead_code)] // #799 が結果受け口から使う
pub(crate) fn outcome_from_ios_code(code: i32) -> Result<OpenSettingsOutcome, DeviceError> {
    match code {
        0 => Ok(OpenSettingsOutcome::HandedOff),
        1 => Ok(OpenSettingsOutcome::NotOpened),
        2 => Err(DeviceError::Internal("open settings present")),
        _ => Err(DeviceError::Internal("open settings outcome")),
    }
}

/// Android の結果コード表（SSOT。Java 側の定数と一致させる）。未知のコードは失敗に倒す。
#[allow(dead_code)] // #800 が結果受け口から使う
pub(crate) fn outcome_from_android_code(code: i32) -> Result<OpenSettingsOutcome, DeviceError> {
    match code {
        0 => Ok(OpenSettingsOutcome::HandedOff),
        1 => Ok(OpenSettingsOutcome::NotOpened),
        2 => Err(DeviceError::Internal("open settings background")),
        3 => Err(DeviceError::Internal("open settings present")),
        _ => Err(DeviceError::Internal("open settings outcome")),
    }
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::open_system_settings_show::{CoreOutcome, OpenSettingsFailure};
    use fandhe_device::open_system_settings::OpenSettingsOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, OpenSettingsFailure>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    #[allow(dead_code)] // 後続 issue（#799 / #800）の受け口が使う
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<OpenSettingsOutcome, DeviceError>,
    ) -> bool {
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

/// OS へ設定画面の表示を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_open(request_id: u64, page: SettingsPage) -> Result<(), DeviceError> {
    super::ios_open_system_settings::begin_open(request_id, page)
}

#[cfg(target_os = "android")]
fn begin_open(request_id: u64, page: SettingsPage) -> Result<(), DeviceError> {
    super::android_open_system_settings::begin_open(request_id, page)
}

/// iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(
    state: &OpenSystemSettingsState,
    sender: &CompletionSender<OpenSystemSettingsState>,
) {
    use fandhe_core::screens::open_system_settings_show::{
        finish_open, mark_opening, OpenSettingsPhase,
    };

    if let Some(view) = take_request(state) {
        let _ = sender.send(Box::new(mark_opening));
        let request_id = bridge::begin();
        if let Err(e) = begin_open(request_id, to_page(view.target)) {
            bridge::deliver(request_id, Err(e));
        }
        return;
    }

    if state.phase != OpenSettingsPhase::Opening {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut OpenSystemSettingsState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != OpenSettingsPhase::Idle {
                finish_open(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `open_system_settings_show` 画面用に配線される effect ドライバ（iOS / Android）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn open_system_settings_show_effect(
    state: &OpenSystemSettingsState,
    sender: &CompletionSender<OpenSystemSettingsState>,
) {
    drive_callback(state, sender);
}

/// 同期経路（Windows・macOS・Linux）: [`OpenSystemSettings`] を呼ぶ。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive(
    state: &OpenSystemSettingsState,
    sender: &CompletionSender<OpenSystemSettingsState>,
    opener: &dyn OpenSystemSettings,
) {
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_opening));
    let outcome = to_outcome(opener.open(to_page(view.target)));
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut OpenSystemSettingsState| {
        finish_open(s, generation, outcome);
    }));
}

/// OS 別の [`OpenSystemSettings`] 実装を返す。macOS は対応表 `no` のため fallback に入る。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn platform_open() -> Box<dyn OpenSystemSettings> {
    #[cfg(target_os = "windows")]
    return super::windows_open_system_settings::implementation();
    #[cfg(not(target_os = "windows"))]
    return Box::new(fandhe_device::open_system_settings::UnsupportedOpenSystemSettings);
}

/// `registry.rs` から配線される effect ドライバ（Windows・macOS・Linux）。
///
/// ponytail: 設定画面を開くのは OS への URI 1 回の軽い呼び出しなので UI スレッドで同期に呼ぶ。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn open_system_settings_show_effect(
    state: &OpenSystemSettingsState,
    sender: &CompletionSender<OpenSystemSettingsState>,
) {
    if take_request(state).is_none() {
        return;
    }
    drive(state, sender, platform_open().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_outcome_covers_all_arms() {
        assert_eq!(
            to_outcome(Ok(OpenSettingsOutcome::HandedOff)),
            Ok(CoreOutcome::HandedOff)
        );
        assert_eq!(
            to_outcome(Ok(OpenSettingsOutcome::NotOpened)),
            Ok(CoreOutcome::NotOpened)
        );
        assert_eq!(
            to_outcome(Ok(OpenSettingsOutcome::PageUnavailable)),
            Ok(CoreOutcome::PageUnavailable)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("open settings"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("open settings shell"))),
            Err(OpenSettingsFailure::Failed(m)) if !m.contains("shell")
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
        assert_eq!(
            outcome_from_ios_code(0).unwrap(),
            OpenSettingsOutcome::HandedOff
        );
        assert_eq!(
            outcome_from_ios_code(1).unwrap(),
            OpenSettingsOutcome::NotOpened
        );
        assert!(outcome_from_ios_code(2).is_err());
        assert_eq!(
            outcome_from_android_code(0).unwrap(),
            OpenSettingsOutcome::HandedOff
        );
        assert_eq!(
            outcome_from_android_code(1).unwrap(),
            OpenSettingsOutcome::NotOpened
        );
        assert!(outcome_from_android_code(2).is_err() && outcome_from_android_code(3).is_err());
    }

    #[test]
    fn to_page_maps_each_target() {
        assert_eq!(to_page(SettingsTarget::App), SettingsPage::AppSettings);
        assert_eq!(
            to_page(SettingsTarget::Notifications),
            SettingsPage::Notifications
        );
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use fandhe_core::screens::open_system_settings_show as screen;
        use fandhe_device::open_system_settings::UnsupportedOpenSystemSettings;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Mutex;

        /// 呼び出し回数と受けた頁を記録し、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            last: Mutex<Option<SettingsPage>>,
            result: fn() -> Result<OpenSettingsOutcome, DeviceError>,
        }

        impl OpenSystemSettings for Fake {
            fn open(&self, page: SettingsPage) -> Result<OpenSettingsOutcome, DeviceError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                *self.last.lock().unwrap() = Some(page);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<OpenSettingsOutcome, DeviceError>) -> Fake {
            Fake {
                calls: AtomicU32::new(0),
                last: Mutex::new(None),
                result,
            }
        }

        /// 押下して 1 回ドライバを回し、反映後の状態を返す。
        fn run(app: bool, opener: &dyn OpenSystemSettings) -> OpenSystemSettingsState {
            let mut s = OpenSystemSettingsState::default();
            if app {
                screen::open_app_settings(&mut s);
            } else {
                screen::open_notification_settings(&mut s);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, opener);
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            s
        }

        #[test]
        fn no_request_never_calls_os() {
            let f = fake(|| Ok(OpenSettingsOutcome::HandedOff));
            let s = OpenSystemSettingsState::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(&s, &sender, &f);
            assert!(rx.try_recv().is_err());
            assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn each_outcome_shows_its_own_status() {
            let f = fake(|| Ok(OpenSettingsOutcome::HandedOff));
            assert_eq!(run(true, &f).status, screen::HANDED_OFF_STATUS);
            let f = fake(|| Ok(OpenSettingsOutcome::NotOpened));
            let s = run(false, &f);
            assert_eq!(s.status, screen::NOT_OPENED_STATUS);
            assert!(s.error.is_empty());
            let f = fake(|| Ok(OpenSettingsOutcome::PageUnavailable));
            let s = run(true, &f);
            assert_eq!(s.status, screen::PAGE_UNAVAILABLE_STATUS);
            assert!(s.error.is_empty());
        }

        #[test]
        fn page_passed_to_os_matches_pressed_button() {
            let f = fake(|| Ok(OpenSettingsOutcome::HandedOff));
            run(true, &f);
            assert_eq!(*f.last.lock().unwrap(), Some(SettingsPage::AppSettings));
            run(false, &f);
            assert_eq!(*f.last.lock().unwrap(), Some(SettingsPage::Notifications));
            assert_eq!(f.calls.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn unsupported_shows_unsupported_not_success() {
            let s = run(false, &UnsupportedOpenSystemSettings);
            assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
            assert!(s.error.is_empty());
            assert_ne!(s.status, screen::HANDED_OFF_STATUS);
        }

        #[test]
        fn failure_goes_to_error_with_fixed_label() {
            let f = fake(|| Err(DeviceError::Internal("open settings shell")));
            let s = run(false, &f);
            assert!(s.status.is_empty());
            assert!(s.error.starts_with(screen::FAILED_PREFIX));
            assert!(!s.error.contains("shell"));
        }

        #[test]
        fn stale_generation_completion_is_dropped() {
            let mut s = OpenSystemSettingsState::default();
            screen::open_app_settings(&mut s);
            screen::mark_opening(&mut s);
            let stale = s.generation.wrapping_sub(1);
            screen::finish_open(&mut s, stale, Ok(CoreOutcome::HandedOff));
            assert!(s.status.is_empty());
            assert_eq!(s.phase, screen::OpenSettingsPhase::Opening);
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
        assert!(!bridge::deliver(
            id + 1000,
            Ok(OpenSettingsOutcome::HandedOff)
        ));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, Ok(OpenSettingsOutcome::NotOpened)));
        assert!(!bridge::deliver(id, Ok(OpenSettingsOutcome::HandedOff)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some(Ok(CoreOutcome::NotOpened)));
        bridge::SLOT.clear(id);
    }
}
