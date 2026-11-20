//! 「一定時間後に状態を変える」予約表（レンダラ非依存、#1291）。
//!
//! 利用者が effect の中でスレッドと sleep を書かずに、時間で戻る表示（「コピーしました」を 2 秒後に消す等）と
//! 定期更新を書けるようにする。入口は [`crate::CompletionSender::after`]・[`crate::CompletionSender::cancel_timer`]。
//! 描画ループの持ち主（`fandhe-render-egui` の `ProxyApp`）が毎フレーム [`drain_timers`] で期限の来た予約を
//! `StateStore::apply` へ流す。スレッドは作らないため、スレッドを持てない wasm でも同じに動く。
//!
//! # 書き方（レベルトリガ）
//!
//! effect は毎フレーム呼ばれる。「状態がこうなら予約する」と毎フレーム書いてよい。同じキーの予約が残っている間の
//! 呼び出しは何もしないので、「要求済み」「処理中」の二段の状態は要らない。
//!
//! ```
//! use std::time::Duration;
//! use fandhe_runtime::CompletionSender;
//!
//! struct State { copied: bool, ticks: u32 }
//!
//! fn effect(s: &State, sender: &CompletionSender<State>) {
//!     if s.copied {
//!         let _ = sender.after("copied-reset", Duration::from_secs(2), |s: &mut State| s.copied = false);
//!     } else {
//!         sender.cancel_timer("copied-reset"); // 先に消えたなら予約も取り下げる
//!     }
//!     // 定期更新: 期限が来て適用されると、次のフレームの effect が同じキーで積み直す
//!     let _ = sender.after("tick", Duration::from_secs(1), |s: &mut State| s.ticks += 1);
//! }
//! # let _ = effect;
//! ```
//!
//! # ロックの内側で drop しない
//!
//! 予約のクロージャは任意の資源を捕まえる。その `Drop` が同じ表（`sender.cancel_timer` など）を呼ぶと、表のロックを持ったまま
//! drop した場合に再入で止まる。表のロックの内側では予約を表から取り出すだけにし、drop は**ロックを手放してから**行う
//! （[`Timers::cancel`]・`cancel_all`・満杯や既存キーで断る `arm_at`。期限が来た予約は取り出して `apply` するだけで、
//! `drain_timers` はロックの外で処理する）。表そのものの drop（`ProxyApp` の破棄）では、`Arc` の強参照がすでに 0 なので、
//! 捕まえた sender の `cancel_timer` は表に触れず `false` を返す。
//!
//! # 契約
//!
//! - 時間は単調時計（`web_time::Instant`。wasm では `performance.now`）で測る。システム時刻の変更で狂わない（契約表 3）。
//! - 期限の起点は [`CompletionSender::after`](crate::CompletionSender::after) を呼んだ時点。期限が来た予約はキーが空いた状態で
//!   フレーム先頭に適用されるため、そのフレーム末尾の effect が同じキーで再び予約できる（定期更新はこの形）。
//! - 期限を**延ばしたい**ときは、キーに世代を含め（`format!("reset:{}", s.copy_count)`）、適用するクロージャが
//!   状態の世代と照合して古ければ何もしない。同じキーの予約は延びないため、世代違いのキーで積む。
//! - 戻り値は `Result<`[`Armed`]`, `[`TimerRefused`]`>`。`Ok(AlreadyPending)` は同じキーが残っていた正常な結果で、
//!   `Err(Full | TooFar | Closed)` は**積まれていない**（件数が [`MAX_PENDING`] に達した・期限が時計の範囲を超えた・持ち主が破棄済み）。
//!   毎フレーム呼ぶ effect は `Err` を捨てずに扱う（ログに出す・状態に失敗を持つ等。黙って時間で戻らなくなる）。
//! - キーのうち `fandhe.` で始まるものはフレームワークが使う。利用者は使わない。
//! - 予約は描画ループの持ち主ごと。表の持ち主は `ProxyApp` だけで、sender は表を**弱く**参照する。
//!   アプリが破棄されれば予約も（予約のクロージャが抱えた sender の複製・資源ごと）解放され、以後の `after` は `Err(Closed)`。
//!   破棄後に状態へ適用されることはない。画面を切り替えても予約は消えない。画面が要らなくなる状態では
//!   effect が [`cancel_timer`](crate::CompletionSender::cancel_timer) するか、適用側で状態を見て何もしない。
//! - 起床は期限の瞬間ちょうどに来るとは限らない（OS・描画ループの粒度）。早く起きて期限前だったときは、
//!   [`drain_timers`] が残り時間でもう一度起床を予約するため、取りこぼさない。

use std::sync::Mutex;
use std::time::Duration;

use web_time::Instant;

use crate::{Completion, RepaintWaker};
use fandhe_core::StateStore;

/// 同時に積める予約の件数の上限。超える [`crate::CompletionSender::after`] は [`TimerRefused::Full`] を返す。
pub const MAX_PENDING: usize = 1024;

/// 予約の結果（積めた場合）。[`crate::CompletionSender::after`] が返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Armed {
    /// 新しく積んだ。
    New,
    /// 同じキーの予約が既に残っていたので何もしなかった（期限も延ばしていない。正常な結果）。
    AlreadyPending,
}

/// 予約を断った理由。`AlreadyPending` と違い、**予約は積まれていない**ので、呼び出し側が扱いを決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimerRefused {
    /// 同時の予約が [`MAX_PENDING`] 件に達している。
    Full,
    /// 期限が時計の範囲を超える（`Instant` に足せない）。
    TooFar,
    /// 表の持ち主（描画ループの持ち主。`ProxyApp`）が破棄済み、または表につながっていない送信側。
    /// 外のワーカーが sender を持ち続けていても、破棄後の予約はここで断る（予約も、予約が抱えた資源も残らない）。
    /// 守らない範囲: 破棄と重なった呼び出し（表を取った直後に持ち主が破棄された）は `Ok` を返しても発火しない。
    Closed,
}

impl std::fmt::Display for TimerRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TimerRefused::Full => "timer table is full",
            TimerRefused::TooFar => "timer delay is too far",
            TimerRefused::Closed => "timer owner is closed",
        })
    }
}

impl std::error::Error for TimerRefused {}

struct Slot<S> {
    key: String,
    due: Instant,
    completion: Completion<S>,
}

/// キー付きの予約表。持ち主（描画ループの持ち主。`ProxyApp`）が `Arc` を持ち、`CompletionSender`（effect 側）は `Weak` で
/// 参照する（予約のクロージャが sender の複製を抱えても循環しない）。持ち主が [`drain_timers`] で流す。
pub struct Timers<S> {
    slots: Mutex<Vec<Slot<S>>>,
}

impl<S> Default for Timers<S> {
    fn default() -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
        }
    }
}

impl<S> Timers<S> {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Slot<S>>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `key` の予約が無ければ `now + delay` を期限に積んで `Ok(New)`。既にあれば何もせず `Ok(AlreadyPending)`（期限も延ばさない）。
    /// 期限が時計の範囲を超えるときは `Err(TooFar)`、予約が [`MAX_PENDING`] 件に達しているときは `Err(Full)`（どちらも積まない）。
    pub(crate) fn arm_at(
        &self,
        now: Instant,
        key: &str,
        delay: Duration,
        completion: Completion<S>,
    ) -> Result<Armed, TimerRefused> {
        let due = now.checked_add(delay).ok_or(TimerRefused::TooFar)?;
        let mut slots = self.lock();
        // 断る経路では、先にロックを手放してから、引数の `completion`（と捕まえた資源）を drop する
        // （モジュール doc「ロックの内側で drop しない」）。
        if slots.iter().any(|s| s.key == key) {
            drop(slots);
            drop(completion);
            return Ok(Armed::AlreadyPending);
        }
        if slots.len() >= MAX_PENDING {
            drop(slots);
            drop(completion);
            return Err(TimerRefused::Full);
        }
        slots.push(Slot {
            key: key.to_string(),
            due,
            completion,
        });
        Ok(Armed::New)
    }

    /// `key` の予約を取り下げる。あれば `true`。
    pub fn cancel(&self, key: &str) -> bool {
        // ロックの内側では表から取り出すだけ。drop（予約のクロージャと捕まえた資源の `Drop`）はロックを手放してから行う。
        let removed: Vec<Slot<S>> = {
            let mut slots = self.lock();
            let (gone, kept): (Vec<_>, Vec<_>) = slots.drain(..).partition(|s| s.key == key);
            *slots = kept;
            gone
        };
        let existed = !removed.is_empty();
        drop(removed);
        existed
    }

    /// 予約をすべて取り下げる。
    pub fn cancel_all(&self) {
        // `let` の文の終わりでロックが外れ、そのあとで drop する。
        let taken = std::mem::take(&mut *self.lock());
        drop(taken);
    }

    /// 期限が `now` までに来た予約を、期限の早い順に取り出す（取り出した分は表から消える）。
    pub(crate) fn take_due_at(&self, now: Instant) -> Vec<Completion<S>> {
        let mut slots = self.lock();
        let mut due = Vec::new();
        let mut i = 0;
        while i < slots.len() {
            if slots[i].due <= now {
                due.push(slots.remove(i));
            } else {
                i += 1;
            }
        }
        due.sort_by_key(|s| s.due);
        due.into_iter().map(|s| s.completion).collect()
    }

    /// 残っている予約のうち最も早い期限までの時間（期限切れは `ZERO`）。無ければ `None`。
    pub(crate) fn next_in_at(&self, now: Instant) -> Option<Duration> {
        self.lock()
            .iter()
            .map(|s| s.due.saturating_duration_since(now))
            .min()
    }
}

/// 期限の来た予約を `store.apply` へ順に流す。描画ループの持ち主がフレーム先頭で
/// [`drain_completions`](crate::drain_completions) の直後に呼ぶ。まだ残る予約があれば、その期限に合わせて
/// `waker` へ起床を予約し直す（早く起きて取りこぼした場合の保険。モジュール doc 参照）。
pub fn drain_timers<S>(store: &mut StateStore<S>, timers: &Timers<S>, waker: &dyn RepaintWaker) {
    drain_timers_at(store, timers, waker, Instant::now());
}

fn drain_timers_at<S>(
    store: &mut StateStore<S>,
    timers: &Timers<S>,
    waker: &dyn RepaintWaker,
    now: Instant,
) {
    for completion in timers.take_due_at(now) {
        store.apply(completion);
    }
    if let Some(left) = timers.next_in_at(now) {
        waker.wake_after(left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Default)]
    struct St {
        log: String,
    }

    #[derive(Default)]
    struct W(Mutex<Vec<Duration>>);
    impl RepaintWaker for W {
        fn wake(&self) {}
        fn wake_after(&self, d: Duration) {
            self.0.lock().unwrap().push(d);
        }
    }

    fn push(c: char) -> Completion<St> {
        Box::new(move |s: &mut St| s.log.push(c))
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// 期限前は何も起きず、期限に来た時点で適用される。時計は差し替えた `now` で進める。
    #[test]
    fn fires_only_once_due() {
        let t0 = Instant::now();
        let timers = Timers::default();
        let mut store = StateStore::new(St::default());
        let w = W::default();
        assert_eq!(timers.arm_at(t0, "k", ms(100), push('a')), Ok(Armed::New));

        drain_timers_at(&mut store, &timers, &w, t0 + ms(99));
        assert_eq!(store.state().log, "", "期限前は適用しない");
        assert_eq!(
            *w.0.lock().unwrap(),
            vec![ms(1)],
            "残り時間で起床を予約し直す"
        );

        drain_timers_at(&mut store, &timers, &w, t0 + ms(100));
        assert_eq!(store.state().log, "a");
        drain_timers_at(&mut store, &timers, &w, t0 + ms(500));
        assert_eq!(store.state().log, "a", "二度は適用しない");
        assert_eq!(
            w.0.lock().unwrap().len(),
            1,
            "残りが無ければ起床を予約しない"
        );
    }

    /// 同じキーの予約が残っている間は積まず、期限も延ばさない。適用後は同じキーで積み直せる（定期更新の形）。
    #[test]
    fn same_key_is_idempotent_until_fired() {
        let t0 = Instant::now();
        let timers = Timers::default();
        let mut store = StateStore::new(St::default());
        let w = W::default();
        assert_eq!(timers.arm_at(t0, "k", ms(100), push('a')), Ok(Armed::New));
        assert_eq!(
            timers.arm_at(t0 + ms(50), "k", ms(100), push('b')),
            Ok(Armed::AlreadyPending)
        );
        drain_timers_at(&mut store, &timers, &w, t0 + ms(100));
        assert_eq!(store.state().log, "a", "後からの予約で期限は延びない");
        assert_eq!(
            timers.arm_at(t0 + ms(100), "k", ms(100), push('c')),
            Ok(Armed::New)
        );
        drain_timers_at(&mut store, &timers, &w, t0 + ms(200));
        assert_eq!(store.state().log, "ac");
    }

    #[test]
    fn cancel_removes_only_that_key() {
        let t0 = Instant::now();
        let timers = Timers::default();
        let mut store = StateStore::new(St::default());
        let w = W::default();
        timers.arm_at(t0, "a", ms(10), push('a')).unwrap();
        timers.arm_at(t0, "b", ms(10), push('b')).unwrap();
        assert!(timers.cancel("a") && !timers.cancel("a"));
        drain_timers_at(&mut store, &timers, &w, t0 + ms(10));
        assert_eq!(store.state().log, "b");
        timers.arm_at(t0, "c", ms(10), push('c')).unwrap();
        timers.cancel_all();
        drain_timers_at(&mut store, &timers, &w, t0 + ms(10));
        assert_eq!(store.state().log, "b");
    }

    /// 同時に期限が来たら、期限の早い順に適用する。
    #[test]
    fn due_order_is_by_deadline() {
        let t0 = Instant::now();
        let timers = Timers::default();
        let mut store = StateStore::new(St::default());
        let w = W::default();
        timers.arm_at(t0, "late", ms(30), push('l')).unwrap();
        timers.arm_at(t0, "early", ms(10), push('e')).unwrap();
        drain_timers_at(&mut store, &timers, &w, t0 + ms(100));
        assert_eq!(store.state().log, "el");
    }

    /// 溢れる期限は積まず、件数の上限を超える予約も積まない。取り下げれば空きができる。
    #[test]
    fn rejects_overflowing_delay_and_caps_pending() {
        let t0 = Instant::now();
        let timers = Timers::default();
        assert_eq!(
            timers.arm_at(t0, "huge", Duration::MAX, push('x')),
            Err(TimerRefused::TooFar)
        );
        assert!(timers.next_in_at(t0).is_none(), "溢れた予約は残らない");
        for i in 0..MAX_PENDING {
            assert_eq!(
                timers.arm_at(t0, &format!("k{i}"), ms(10), push('a')),
                Ok(Armed::New)
            );
        }
        assert_eq!(
            timers.arm_at(t0, "over", ms(10), push('b')),
            Err(TimerRefused::Full)
        );
        // 上限に達していても、残っているキーは「予約済み」であって「断った」ではない。
        assert_eq!(
            timers.arm_at(t0, "k5", ms(10), push('b')),
            Ok(Armed::AlreadyPending)
        );
        assert!(timers.cancel("k0"));
        assert_eq!(timers.arm_at(t0, "over", ms(10), push('b')), Ok(Armed::New));
    }

    /// `f` が 5 秒以内に終わることを別スレッドで確かめる（再入でロックを取り直して止まる実装はここで落ちる）。
    fn finishes_within_timeout(f: impl FnOnce() + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            f();
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "表のロックを持ったまま drop して再入で止まった"
        );
    }

    /// drop のときに同じ表の `cancel("other")` を呼ぶ資源。ロックの内側で drop すると、同じ `Mutex` を取り直して止まる。
    struct ReentrantGuard(Arc<Timers<St>>);
    impl Drop for ReentrantGuard {
        fn drop(&mut self) {
            self.0.cancel("other");
        }
    }

    fn arm_with_guard(timers: &Arc<Timers<St>>, key: &str) -> Result<Armed, TimerRefused> {
        let guard = ReentrantGuard(timers.clone());
        timers.arm_at(
            Instant::now(),
            key,
            ms(60_000),
            Box::new(move |_: &mut St| {
                let _ = &guard;
            }),
        )
    }

    /// 取り下げる予約のクロージャが、`Drop` で同じ表の別の予約を取り下げても止まらない。
    #[test]
    fn cancel_does_not_drop_closures_under_the_lock() {
        finishes_within_timeout(|| {
            let timers = Arc::new(Timers::default());
            arm_with_guard(&timers, "victim").unwrap();
            timers
                .arm_at(Instant::now(), "other", ms(60_000), push('o'))
                .unwrap();
            assert!(timers.cancel("victim"));
            assert!(!timers.cancel("other"), "Drop が取り下げ済み");
        });
    }

    #[test]
    fn cancel_all_does_not_drop_closures_under_the_lock() {
        finishes_within_timeout(|| {
            let timers = Arc::new(Timers::default());
            arm_with_guard(&timers, "victim").unwrap();
            timers
                .arm_at(Instant::now(), "other", ms(60_000), push('o'))
                .unwrap();
            timers.cancel_all();
            assert!(timers.next_in_at(Instant::now()).is_none());
        });
    }

    /// 既存キー・満杯で断る `arm_at` が、引数のクロージャをロックを手放してから drop する。
    #[test]
    fn refused_arm_does_not_drop_the_closure_under_the_lock() {
        finishes_within_timeout(|| {
            let timers = Arc::new(Timers::default());
            arm_with_guard(&timers, "k").unwrap();
            timers
                .arm_at(Instant::now(), "other", ms(60_000), push('o'))
                .unwrap();
            // 既存キー: 引数のクロージャ（ガード付き）が drop され、Drop が "other" を取り下げる。
            assert_eq!(arm_with_guard(&timers, "k"), Ok(Armed::AlreadyPending));
            assert!(!timers.cancel("other"));
            // 満杯: 同じ形。
            for i in 0..MAX_PENDING - 2 {
                timers
                    .arm_at(Instant::now(), &format!("f{i}"), ms(60_000), push('f'))
                    .unwrap();
            }
            timers
                .arm_at(Instant::now(), "other", ms(60_000), push('o'))
                .unwrap();
            assert_eq!(arm_with_guard(&timers, "full"), Err(TimerRefused::Full));
            assert!(!timers.cancel("other"));
        });
    }

    #[test]
    fn timers_are_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<Timers<St>>>();
    }
}
