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

/// OS 側コールバックとの橋渡し（iOS / Android）。notification とは共有しない本機能専用。
///
/// 画面の effect は pull 型の [`SLOT`](bridge::SLOT)（`RequestSlot` の facade）、画面を経ない呼び出し
/// （`platform_app_badge`）は別チャネルの `WAITERS` で待つ。互いの保留を上書きしない
/// （OS 側は同時に 1 要求しか持てないため、先行の完了までは相手の依頼を拒否する。`fandhe-cap-calendar` と同形）。
#[cfg(any(target_os = "ios", target_os = "android", test))]
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
pub(crate) mod bridge {
    use crate::contract::BadgeOutcome;
    use crate::error::AppBadgeError;

    use fandhe_runtime::request_slot::RequestSlot;

    /// 依頼した値と OS の生の結果。画面用への写像は取り出す側が行う。
    pub(crate) type Outcome = (u32, Result<BadgeOutcome, AppBadgeError>);

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    type Waiter = std::sync::mpsc::Sender<Result<BadgeOutcome, AppBadgeError>>;

    /// 画面を経ない呼び出しの待ち合わせ。ID は [`SLOT`] の払い出しと衝突しないよう上位ビットを立てる。
    static WAITERS: std::sync::Mutex<Vec<(u64, Waiter)>> = std::sync::Mutex::new(Vec::new());
    static NEXT_EXTERNAL_ID: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(1 << 62);

    /// 画面を経ない呼び出しの待ちを登録して `(request_id, 受信側)` を返す。先行（画面の進行中の保留・別の待ち）が
    /// あれば `Internal` で拒否する。判定と登録は `WAITERS` のロックの内側で行い、[`begin_screen`] と直列化する。
    /// 結果は受信側のチャネルに入るため、[`SLOT`] の未取得の結果を置き換えない。
    pub(crate) fn begin_external() -> Result<
        (
            u64,
            std::sync::mpsc::Receiver<Result<BadgeOutcome, AppBadgeError>>,
        ),
        AppBadgeError,
    > {
        let mut w = WAITERS.lock().unwrap_or_else(|e| e.into_inner());
        if !w.is_empty() || SLOT.snapshot().is_some_and(|(_, r)| r.is_none()) {
            return Err(AppBadgeError::Internal);
        }
        let id = NEXT_EXTERNAL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        w.push((id, tx));
        Ok((id, rx))
    }

    /// 登録済みの待ちを外す（依頼自体が同期的に失敗したとき）。
    pub(crate) fn cancel_external(request_id: u64) {
        WAITERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| *id != request_id);
    }

    /// 画面の依頼を登録する。画面を経ない待ちが残っているとき、または [`SLOT`] に進行中の保留があるときは
    /// `None`（置き換えない）。
    pub(crate) fn begin_screen() -> Option<u64> {
        let w = WAITERS.lock().unwrap_or_else(|e| e.into_inner());
        if !w.is_empty() {
            return None;
        }
        super::SCREEN_OWNER.begin(&SLOT)
    }

    /// OS からの結果を置く。画面を経ない待ちの id ならその待ちへ、そうでなければ [`SLOT`] へ。
    /// `request_id` 不一致・未登録・既に結果ありは捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(
        request_id: u64,
        value: u32,
        result: Result<BadgeOutcome, AppBadgeError>,
    ) -> bool {
        let waiter = {
            let mut w = WAITERS.lock().unwrap_or_else(|e| e.into_inner());
            w.iter()
                .position(|(id, _)| *id == request_id)
                .map(|i| w.remove(i).1)
        };
        if let Some(tx) = waiter {
            return tx.send(result).is_ok();
        }
        SLOT.deliver(request_id, (value, result))
    }

    /// Android: Activity 再生成直後に `android_main` から呼ぶ。保留中の要求は復元せず破棄し、
    /// 以後届く旧 `request_id` の [`deliver`] を無視させる。待っている呼び出しは送信側を捨てて起こす
    /// （受信側は `Internal` で戻る）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset();
        WAITERS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// モバイル版の [`AppBadge`]（[`platform_app_badge`]）。OS の完了（許可ダイアログの応答を含む）まで `set` を block する
/// （期限は置かない。推測の期限で失敗にすると、後から設定が効く）。UI スレッド（iOS / Android のメイン）から呼ばない。
/// 同時に 1 件だけで、先行（画面の保留・別の待ち）が終わるまで後から呼ぶと `Internal` で即座に戻る。
#[cfg(any(target_os = "ios", target_os = "android"))]
struct CallbackAppBadge;

#[cfg(any(target_os = "ios", target_os = "android"))]
impl crate::contract::AppBadge for CallbackAppBadge {
    fn set(&self, value: BadgeValue) -> Result<BadgeOutcome, AppBadgeError> {
        let (request_id, rx) = bridge::begin_external()?;
        if let Err(e) = begin_set(request_id, value) {
            bridge::cancel_external(request_id);
            return Err(e);
        }
        // 結果が届くか、再生成で送信側が捨てられる（Disconnected）まで待つ。
        rx.recv().unwrap_or(Err(AppBadgeError::Internal))
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

/// 画面用 effect が追う自分の分の要求（公開入口 [`platform_app_badge`] は別の待ち口を使う）。
#[cfg(any(target_os = "ios", target_os = "android", test))]
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
static SCREEN_OWNER: fandhe_runtime::request_slot::SlotOwner =
    fandhe_runtime::request_slot::SlotOwner::new();

/// 公開入口などの先行が進行中で、画面の依頼を受けられないときの表示。
#[cfg(any(target_os = "ios", target_os = "android", test))]
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
const BUSY_MESSAGE: &str = "他のバッジ設定が処理中です。完了後にもう一度お試しください";

/// 依頼が失われた（再生成・別の依頼への置き換え）ときの表示。
#[cfg(any(target_os = "ios", target_os = "android", test))]
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
const LOST_MESSAGE: &str = "バッジ設定が中断されました。もう一度お試しください";

/// iOS / Android 版の effect ドライバ本体。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(state: &AppBadgeSetState, sender: &CompletionSender<AppBadgeSetState>) {
    use fandhe_core::screens::app_badge_set::BadgePhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_setting));
        let generation = request.generation;
        // 先行（公開入口の待ちなど）を置き換えず、画面には失敗として返す（黙って捨てない）。
        let Some(request_id) = bridge::begin_screen() else {
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
            finish_set(s, generation, Err(LOST_MESSAGE.to_string()));
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

#[cfg(test)]
mod bridge_tests {
    use super::bridge;
    use super::*;

    /// プロセス static の `SLOT`・待ち合わせを触るテストを直列化する。
    static BRIDGE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn stale_request_id_is_dropped_and_second_deliver_ignored() {
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
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
    fn external_wait_is_isolated_from_screen_slot_and_second_set_does_not_drop_first() {
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        // 1 回目の結果が届いて、受け手がまだ取り出していない。
        let (id1, rx1) = bridge::begin_external().expect("空き");
        // 待ちの間は画面の依頼も 2 つ目の待ちも拒否される。
        assert!(bridge::begin_screen().is_none());
        assert_eq!(
            bridge::begin_external().err(),
            Some(AppBadgeError::Internal)
        );
        assert!(bridge::deliver(id1, 1, Ok(BadgeOutcome::Submitted)));
        // 2 回目が来ても 1 回目の成功は落ちない。
        let (_id2, _rx2) = bridge::begin_external().expect("1 回目の配送後は空き");
        assert_eq!(rx1.recv(), Ok(Ok(BadgeOutcome::Submitted)));
        bridge::reset();
    }

    #[test]
    fn screen_pending_blocks_external_and_reset_wakes_waiter() {
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let screen = bridge::begin_screen().expect("空き");
        assert_eq!(
            bridge::begin_external().err(),
            Some(AppBadgeError::Internal)
        );
        assert!(bridge::SLOT.snapshot().is_some_and(|(id, _)| id == screen));
        bridge::reset();
        let (_id, rx) = bridge::begin_external().expect("再生成後は空き");
        let t = std::thread::spawn(move || rx.recv());
        std::thread::sleep(std::time::Duration::from_millis(30));
        bridge::reset();
        assert!(t.join().unwrap().is_err());
    }

    #[test]
    fn screen_request_fails_instead_of_hanging_when_slot_is_reset() {
        use fandhe_core::screens::app_badge_set::BadgePhase;
        struct Waker;
        impl fandhe_runtime::RepaintWaker for Waker {
            fn wake(&self) {}
            fn wake_after(&self, _delay: std::time::Duration) {}
        }
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        bridge::begin_screen().expect("空き");
        // Activity 再生成で slot が空になった後の 1 フレーム（Setting のまま）。
        bridge::reset();
        let mut s = AppBadgeSetState {
            phase: BadgePhase::Setting,
            ..Default::default()
        };
        let generation = s.generation;
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, std::sync::Arc::new(Waker));
        fandhe_runtime::request_slot::poll_and_finish_owned(
            &bridge::SLOT,
            &SCREEN_OWNER,
            &sender,
            |_| {},
            |_s: &mut AppBadgeSetState, _o| true,
            move |s: &mut AppBadgeSetState| {
                finish_set(s, generation, Err(LOST_MESSAGE.to_string()));
            },
        );
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        assert_eq!(s.phase, BadgePhase::Idle);
        assert!(s.error.contains(LOST_MESSAGE));
    }

    #[test]
    fn interrupted_result_wakes_external_waiter_with_internal() {
        let _g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::reset();
        let (id, rx) = bridge::begin_external().expect("空き");
        // 許可ダイアログ待ちの間に Activity が破棄され、Java が中断(4)を届ける。
        let (value, result) = crate::android::decode(5, 4);
        assert!(bridge::deliver(id, value, result));
        assert_eq!(rx.recv(), Ok(Err(AppBadgeError::Internal)));
        // 二重に届いても（Java は first-wins だが）待ちは既に外れていて何も起きない。
        assert!(!bridge::deliver(id, 5, Ok(BadgeOutcome::Submitted)));
        bridge::reset();
    }
}
