//! Phase 11（親 #1141）型サンプル「バッジの設定」（Issue #1360）の effect ドライバ。
//! `fandhe_core::screens::app_badge_set`（端末依存ゼロ）の要求を観測し、値を
//! `crate::contract::BadgeValue` で検査してから OS へ渡し、結果を `CompletionSender`
//! 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、`device/photo_save.rs`
//! と同型）。契約と守る範囲は `fandhe_cap_app_badge` のモジュール doc を参照。
//!
//! - iOS（#1361）/ Android（#1362）: コールバック型のため pull 型 [`bridge`]（`RequestSlot`
//!   の facade）で毎フレーム `drive_callback` が結果を反映する。各 OS モジュールが実装済み。
//!   **dispatch はここで最終形にしてあり、
//!   後続 issue は OS 別モジュールだけを埋める**
//! - macOS / Windows / それ以外: [`AppBadge`] を UI スレッドで同期に呼ぶ。Linux は
//!   `UnsupportedAppBadge` で「非対応」を表示する

use crate::contract::{BadgeOutcome, BadgeValue};
use crate::error::AppBadgeError;
use fandhe_core::screens::app_badge_set::{
    AppBadgeSetState, CoreOutcome, OUT_OF_RANGE_MESSAGE, finish_set, mark_setting, take_request,
};

use fandhe_runtime::CompletionSender;

/// `AppBadgeError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。`value` は 0 の成功を「消去」の文言に分けるために使う。
fn to_outcome(
    value: u32,
    result: Result<BadgeOutcome, AppBadgeError>,
) -> Result<CoreOutcome, String> {
    match result {
        Ok(BadgeOutcome::Submitted) if value == 0 => Ok(CoreOutcome::Cleared),
        Ok(BadgeOutcome::Submitted) => Ok(CoreOutcome::Submitted),
        Ok(BadgeOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(AppBadgeError::PermissionDenied) => Ok(CoreOutcome::Denied),
        Err(AppBadgeError::Unsupported) => Ok(CoreOutcome::Unsupported),
        Err(AppBadgeError::InvalidInput) => Err(OUT_OF_RANGE_MESSAGE.to_string()),
        Err(_) => Err("OS のバッジ設定に失敗しました".to_string()),
    }
}

/// OS 側コールバックとの橋渡し（iOS / Android、pull 型、`RequestSlot` の facade）。
/// notification とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use crate::contract::BadgeOutcome;
    use crate::error::AppBadgeError;

    use fandhe_runtime::request_slot::RequestSlot;

    /// 依頼した値と OS の生の結果。画面用への写像は取り出す側が行う
    /// （画面経由でも公開入口 [`super::poll_set_badge`] でも同じ slot を使う）。
    pub(crate) type Outcome = (u32, Result<BadgeOutcome, AppBadgeError>);

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// OS からの結果を置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(
        request_id: u64,
        value: u32,
        result: Result<BadgeOutcome, AppBadgeError>,
    ) -> bool {
        SLOT.deliver(request_id, (value, result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる（`photo_save::bridge::reset` と同じ）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// バッジ設定を依頼して要求 id を返す。保留は 1 件だけで、進行中に新しい依頼を出すと `Internal`
/// （画面経由の依頼と slot を共有し、どちらも先行を置き換えない。画面側は「処理中」の失敗を表示する。
/// 失敗時の clear で既存依頼を壊さない）。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn begin_set_badge(value: BadgeValue) -> Result<u64, AppBadgeError> {
    let Some(request_id) = bridge::SLOT.try_begin() else {
        return Err(AppBadgeError::Internal);
    };
    match begin_set(request_id, value) {
        Ok(()) => Ok(request_id),
        Err(e) => {
            bridge::SLOT.clear(request_id);
            Err(e)
        }
    }
}

/// [`begin_set_badge`] の結果が届くまで待つ。期限は置かない（OS の許可ダイアログの応答待ちを
/// 推測の期限で失敗にしない）。Activity 再生成（[`bridge::reset`]）で slot が捨てられたら `Internal`。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn wait_set_badge(request_id: u64) -> Result<BadgeOutcome, AppBadgeError> {
    loop {
        match bridge::SLOT.snapshot() {
            Some((id, Some((_, result)))) if id == request_id => {
                bridge::SLOT.clear(id);
                return result;
            }
            Some((id, None)) if id == request_id => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => return Err(AppBadgeError::Internal),
        }
    }
}

/// モバイル版の [`AppBadge`]（[`platform_app_badge`]）。OS の完了まで `set` を block する。
/// UI スレッド（iOS / Android のメイン）から呼ばない。
#[cfg(any(target_os = "ios", target_os = "android"))]
struct CallbackAppBadge;

#[cfg(any(target_os = "ios", target_os = "android"))]
impl crate::contract::AppBadge for CallbackAppBadge {
    fn set(&self, value: BadgeValue) -> Result<BadgeOutcome, AppBadgeError> {
        wait_set_badge(begin_set_badge(value)?)
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    bridge::reset();
}

/// OS へ設定を依頼する。`Ok` は受理済みで結果は後から [`bridge::deliver`] で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。
#[cfg(target_os = "ios")]
fn begin_set(request_id: u64, value: BadgeValue) -> Result<(), AppBadgeError> {
    crate::ios::begin_set(request_id, value)
}

#[cfg(target_os = "android")]
fn begin_set(request_id: u64, value: BadgeValue) -> Result<(), AppBadgeError> {
    crate::android::begin_set(request_id, value)
}

/// 画面用 effect が追う自分の分の要求（公開入口 [`platform_app_badge`] と slot を共有するため）。
#[cfg(any(target_os = "ios", target_os = "android"))]
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// 公開入口などの先行が進行中で、画面の依頼を受けられないときの表示。
#[cfg(any(target_os = "ios", target_os = "android"))]
const BUSY_MESSAGE: &str = "他のバッジ設定が処理中です。完了後にもう一度お試しください";

/// iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(state: &AppBadgeSetState, sender: &CompletionSender<AppBadgeSetState>) {
    use fandhe_core::screens::app_badge_set::BadgePhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_setting));
        let generation = request.generation;
        // 先行（公開入口の待ちなど）を置き換えず、画面には失敗として返す（黙って捨てない）。
        let Some(request_id) = SCREEN_OWNER.begin(&bridge::SLOT) else {
            let _ = sender.send(Box::new(move |s: &mut AppBadgeSetState| {
                finish_set(s, generation, Err(BUSY_MESSAGE.to_string()));
            }));
            return;
        };
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
    fandhe_runtime::request_slot::poll_and_finish_owned(
        &bridge::SLOT,
        &SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut AppBadgeSetState, (value, result)| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != BadgePhase::Idle {
                finish_set(s, generation, to_outcome(value, result));
                true
            } else {
                false
            }
        },
        move |s: &mut AppBadgeSetState| {
            finish_set(s, generation, Err(BUSY_MESSAGE.to_string()));
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
    badge: &dyn crate::contract::AppBadge,
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

/// OS 別の [`AppBadge`] 実装を返す（画面の状態型に依存しない入口、`docs/architecture/consumer-app-structure.md` §8.2）。
/// iOS / Android はコールバック型のため、OS の完了（許可ダイアログの応答を含む）まで `set` を block する。
/// 同時に 1 件だけで、先行が終わるまで後から呼ぶと `Internal` で即座に戻る。Linux は常に `Unsupported`。
pub fn platform_app_badge() -> Box<dyn crate::contract::AppBadge> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    return Box::new(CallbackAppBadge);
    #[cfg(target_os = "macos")]
    return crate::macos::implementation();
    #[cfg(target_os = "windows")]
    return crate::windows::implementation();
    #[cfg(not(any(
        target_os = "ios",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    )))]
    return Box::new(crate::contract::UnsupportedAppBadge);
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
    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    #[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

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
            to_outcome(3, Err(AppBadgeError::PermissionDenied)),
            Ok(CoreOutcome::Denied)
        );
        assert_eq!(
            to_outcome(0, Err(AppBadgeError::PermissionDenied)),
            Ok(CoreOutcome::Denied)
        );
        assert_eq!(
            to_outcome(3, Err(AppBadgeError::Unsupported)),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(30000, Err(AppBadgeError::InvalidInput)),
            Err(OUT_OF_RANGE_MESSAGE.to_string())
        );
        assert!(to_outcome(3, Err(AppBadgeError::Internal)).is_err());
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    mod sync_path {
        use super::*;
        use crate::contract::{AppBadge, UnsupportedAppBadge};
        use fandhe_core::screens::app_badge_set::{BadgePhase, clear, set};
        use std::sync::atomic::{AtomicU32, Ordering};

        /// 呼び出し回数を数え、決めた結果を返す偽実装。
        struct Fake {
            calls: AtomicU32,
            result: fn() -> Result<BadgeOutcome, AppBadgeError>,
        }

        impl AppBadge for Fake {
            fn set(&self, _v: BadgeValue) -> Result<BadgeOutcome, AppBadgeError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                (self.result)()
            }
        }

        fn fake(result: fn() -> Result<BadgeOutcome, AppBadgeError>) -> Fake {
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
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
            let sender = CompletionSender::new(tx, std::sync::Arc::new(NoopWaker));
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
            let f = fake(|| Err(AppBadgeError::PermissionDenied));
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

    /// プロセス static の `SLOT` を触るテストを直列化する。
    static BRIDGE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = bridge::SLOT.begin();
        assert!(!bridge::deliver(id + 1000, 1, Ok(BadgeOutcome::Submitted)));
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
        assert!(bridge::deliver(id, 1, Err(AppBadgeError::PermissionDenied)));
        assert!(!bridge::deliver(id, 1, Ok(BadgeOutcome::Submitted)));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert_eq!(ready, Some((1, Err(AppBadgeError::PermissionDenied))));
        bridge::SLOT.clear(id);
    }

    #[test]
    fn wait_returns_delivered_result_and_reset_unblocks() {
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = bridge::SLOT.begin();
        let t = std::thread::spawn(move || wait_set_badge(id));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(bridge::deliver(id, 1, Ok(BadgeOutcome::Cancelled)));
        assert_eq!(t.join().unwrap(), Ok(BadgeOutcome::Cancelled));

        let id = bridge::SLOT.begin();
        let t = std::thread::spawn(move || wait_set_badge(id));
        std::thread::sleep(std::time::Duration::from_millis(50));
        bridge::SLOT.reset();
        assert_eq!(t.join().unwrap(), Err(AppBadgeError::Internal));
    }

    #[test]
    fn screen_does_not_replace_public_request_and_public_does_not_replace_screen() {
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::SLOT.reset();
        // 公開入口の待ちが進行中のとき、画面の登録は拒否され、公開側の id はそのまま。
        let public = bridge::SLOT.try_begin().expect("空き");
        assert!(SCREEN_OWNER.begin(&bridge::SLOT).is_none());
        assert!(bridge::SLOT.snapshot().is_some_and(|(id, _)| id == public));
        bridge::SLOT.clear(public);
        // 画面の要求が進行中のとき、公開入口は置き換えず拒否する。
        let screen = SCREEN_OWNER.begin(&bridge::SLOT).expect("空き");
        assert_eq!(
            begin_set_badge(BadgeValue::new(1).unwrap()),
            Err(AppBadgeError::Internal)
        );
        assert!(bridge::SLOT.snapshot().is_some_and(|(id, _)| id == screen));
        bridge::SLOT.reset();
    }
}
