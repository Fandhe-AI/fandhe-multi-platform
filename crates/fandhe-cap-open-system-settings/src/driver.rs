//! Phase 11（親 #171）型サンプル「OS の設定画面を開く」（Issue #798）の effect ドライバ。
//! `fandhe_core::screens::open_system_settings_show`（端末依存ゼロ）の要求を観測し、頁を
//! `crate::contract::SettingsPage` へ写して OS へ渡し、結果を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、
//! `device/compose_mail_sms.rs` と同型）。契約と守る範囲は `crate::contract` の
//! モジュール doc を参照。
//!
//! - Windows: [`OpenSystemSettings`] を UI スレッドで同期に呼ぶ
//! - iOS / Android: コールバック型のため pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム
//!   `drive_callback` が結果を反映する。OS 別モジュールは #799 / #800 が埋めるまでスタブ
//!   （`Unsupported`）。**dispatch はここで最終形にしてあり、後続 issue は OS 別モジュールだけを埋める**
//! - macOS・Linux・それ以外: `UnsupportedOpenSystemSettings` で「非対応」を表示する（Web は対象外）

use crate::contract::{OpenSettingsOutcome, SettingsPage};
use crate::error::OpenSystemSettingsError;
use fandhe_core::screens::open_system_settings_show::{
    CoreOutcome, OpenSettingsFailure, OpenSystemSettingsState, SettingsTarget,
};

use crate::contract::OpenSystemSettings;
#[cfg(any(target_os = "ios", target_os = "android"))]
use fandhe_core::screens::open_system_settings_show::take_request;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_core::screens::open_system_settings_show::{finish_open, mark_opening, take_request};

use fandhe_runtime::CompletionSender;

/// `OpenSystemSettingsError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする（security.md A09）。
fn to_outcome(
    result: Result<OpenSettingsOutcome, OpenSystemSettingsError>,
) -> Result<CoreOutcome, OpenSettingsFailure> {
    match result {
        Ok(OpenSettingsOutcome::HandedOff) => Ok(CoreOutcome::HandedOff),
        Ok(OpenSettingsOutcome::NotOpened) => Ok(CoreOutcome::NotOpened),
        Ok(OpenSettingsOutcome::PageUnavailable) => Ok(CoreOutcome::PageUnavailable),
        Err(OpenSystemSettingsError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
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
pub(crate) fn outcome_from_ios_code(
    code: i32,
) -> Result<OpenSettingsOutcome, OpenSystemSettingsError> {
    match code {
        0 => Ok(OpenSettingsOutcome::HandedOff),
        1 => Ok(OpenSettingsOutcome::NotOpened),
        2 => Err(OpenSystemSettingsError::Internal("open settings present")),
        _ => Err(OpenSystemSettingsError::Internal("open settings outcome")),
    }
}

/// Android の結果コード表（SSOT。Java 側の定数と一致させる）。未知のコードは失敗に倒す。
#[allow(dead_code)] // #800 が結果受け口から使う
pub(crate) fn outcome_from_android_code(
    code: i32,
) -> Result<OpenSettingsOutcome, OpenSystemSettingsError> {
    match code {
        0 => Ok(OpenSettingsOutcome::HandedOff),
        1 => Ok(OpenSettingsOutcome::NotOpened),
        2 => Err(OpenSystemSettingsError::Internal(
            "open settings background",
        )),
        3 => Err(OpenSystemSettingsError::Internal("open settings present")),
        _ => Err(OpenSystemSettingsError::Internal("open settings outcome")),
    }
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use crate::contract::OpenSettingsOutcome;
    use crate::error::OpenSystemSettingsError;

    use fandhe_runtime::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<OpenSettingsOutcome, OpenSystemSettingsError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を契約のまま置く（画面用への写像は effect ドライバ側、公開入口は生の結果を返す）。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    #[allow(dead_code)] // 後続 issue（#799 / #800）の受け口が使う
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<OpenSettingsOutcome, OpenSystemSettingsError>,
    ) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// gallery の `android_main`（Activity 再生成）から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// OS へ設定画面の表示を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_open(request_id: u64, page: SettingsPage) -> Result<(), OpenSystemSettingsError> {
    crate::ios::begin_open(request_id, page)
}

#[cfg(target_os = "android")]
fn begin_open(request_id: u64, page: SettingsPage) -> Result<(), OpenSystemSettingsError> {
    crate::android::begin_open(request_id, page)
}

/// iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(
    state: &OpenSystemSettingsState,
    sender: &CompletionSender<OpenSystemSettingsState>,
) {
    use fandhe_core::screens::open_system_settings_show::{
        OpenSettingsPhase, finish_open, mark_opening,
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
    fandhe_runtime::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut OpenSystemSettingsState, result| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != OpenSettingsPhase::Idle {
                finish_open(s, generation, to_outcome(result));
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

/// 設定画面の表示を OS へ依頼し、結果を [`poll_open_system_settings`] で受け取るための要求 id を返す（iOS・Android）。
///
/// 画面の状態型に依存しない非同期の入口で、フレームワークの外のアプリも使える。1 プロセスに 1 要求で、
/// 進行中の要求があるうちは `Internal`（busy）で断る。依頼自体の失敗は `Err`（結果は届かない）。
/// macOS・Windows は同期の [`platform_open_system_settings`] を使う。Linux は `Unsupported`。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn begin_open_system_settings(page: SettingsPage) -> Result<u64, OpenSystemSettingsError> {
    let request_id = bridge::SLOT
        .try_begin()
        .ok_or(OpenSystemSettingsError::Internal("open settings busy"))?;
    if let Err(e) = begin_open(request_id, page) {
        bridge::SLOT.clear(request_id);
        return Err(e);
    }
    Ok(request_id)
}

/// iOS・Android 以外では OS コールバック型の実装がない（成功へは倒さず `Unsupported`）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn begin_open_system_settings(_page: SettingsPage) -> Result<u64, OpenSystemSettingsError> {
    Err(OpenSystemSettingsError::Unsupported("open-system-settings"))
}

/// [`begin_open_system_settings`] の結果を取り出す。届いていれば 1 度だけ `Some`、未着・旧 id・取り出し済みは `None`。
pub fn poll_open_system_settings(
    request_id: u64,
) -> Option<Result<OpenSettingsOutcome, OpenSystemSettingsError>> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    return bridge::SLOT.take_ready(request_id);
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = request_id;
        None
    }
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

/// 現在のプラットフォームの同期の [`OpenSystemSettings`] 実装を返す。iOS・Android はコールバック型で trait を通らず、
/// 実装の無い OS と同じく `Unsupported` を返す（成功へは倒さない）。
///
/// 画面の状態型に依存しない入口で、フレームワークの外のアプリも契約を直接呼べる。
pub fn platform_open_system_settings() -> Box<dyn OpenSystemSettings> {
    #[cfg(target_os = "windows")]
    return crate::windows::implementation();
    #[cfg(not(target_os = "windows"))]
    return Box::new(crate::contract::UnsupportedOpenSystemSettings);
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
    drive(state, sender, platform_open_system_settings().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 起床しないだけの口（受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

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
            to_outcome(Err(OpenSystemSettingsError::Unsupported("open settings"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(matches!(
            to_outcome(Err(OpenSystemSettingsError::Internal("open settings shell"))),
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
        use crate::contract::UnsupportedOpenSystemSettings;
        use fandhe_core::screens::open_system_settings_show as screen;
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicU32, Ordering};

        /// 呼び出し回数と受けた頁を記録し、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            last: Mutex<Option<SettingsPage>>,
            result: fn() -> Result<OpenSettingsOutcome, OpenSystemSettingsError>,
        }

        impl OpenSystemSettings for Fake {
            fn open(
                &self,
                page: SettingsPage,
            ) -> Result<OpenSettingsOutcome, OpenSystemSettingsError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                *self.last.lock().unwrap() = Some(page);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<OpenSettingsOutcome, OpenSystemSettingsError>) -> Fake {
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
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
            let f = fake(|| Err(OpenSystemSettingsError::Internal("open settings shell")));
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
        assert_eq!(ready, Some(Ok(OpenSettingsOutcome::NotOpened)));
        bridge::SLOT.clear(id);
    }
}
