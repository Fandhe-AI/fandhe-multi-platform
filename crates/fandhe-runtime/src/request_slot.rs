//! request/response 型端末機能（`device/share.rs` / `device/biometric_auth.rs` /
//! `device/file_pick.rs`）共通の「要求 1 件の追跡」（pull 型 bridge、Issue #943 /
//! 親 #941）。3 機能がそれぞれ逐語コピーしていた `Mutex<Option<Pending>>` +
//! `AtomicU64` 採番 + `snapshot`/`clear`/`reset` をここへ一本化する。
//!
//! [`RequestSlot`] は `CompletionSender` も画面の `generation` も保持しない
//! pull 型: OS 側コールバックは結果を置くだけで、画面への反映は毎フレームの
//! effect ドライバが **その時点の** `sender`/`state` で行う（`device/share.rs`
//! モジュール doc の「pull 型」節と同じ考え方）。各機能は `bridge` facade で
//! `RequestSlot<機能固有の Outcome>` を static に 1 個持ち、OS 側コールバックとの
//! 変換（`to_outcome`）は facade 側に残す（`RequestSlot` は変換済みの値を
//! 格納するだけ）。
//!
//! レンダラ非依存のため `fandhe-runtime` に置く（Issue #1166）。`fandhe-egui-proxy` の
//! 機能実装と、今後の機能 crate（`fandhe-platform-*` を使う側）が共通で使う。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// share / biometric_auth の `drive_callback` 後半で共通のポーリング間隔
/// （旧: 各モジュールが個別に持っていた `POLL_INTERVAL`）。
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// OS へ渡した要求と、届いた結果。
struct Pending<O> {
    request_id: u64,
    /// OS から届いた結果。first-wins（既に `Some` なら 2 回目以降の通知は無視）。
    ready: Option<O>,
}

/// 1 機能ぶんの「要求 1 件の追跡」。`O` は画面へ反映する完了結果の型
/// （機能ごとの facade が `to_outcome` 変換済みの値を渡す）。
///
/// 採番は機能（`RequestSlot` インスタンス）ごとに独立させる。グローバル 1 本の
/// カウンタにまとめない: OS 側（Java の `static long` 等）が見る request_id の
/// 系列を機能をまたいで変えないため（既存 3 実装の挙動を保つ）。
pub struct RequestSlot<O> {
    pending: Mutex<Option<Pending<O>>>,
    /// 0 は「未要求」の番兵として使わない（既存 3 実装と同じ）。
    next_request_id: AtomicU64,
}

impl<O> RequestSlot<O> {
    /// static 初期化用。`const fn` で使うため `Default` は実装しない。
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            pending: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Pending<O>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub fn begin(&self) -> u64 {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        *self.lock() = Some(Pending {
            request_id,
            ready: None,
        });
        request_id
    }

    /// 呼び出し側が決めた `request_id` で登録する（フレームワーク外のアプリが自分の id で OS 実装を直接呼ぶ入口用）。
    /// 採番は進めない。
    pub fn begin_with(&self, request_id: u64) {
        *self.lock() = Some(Pending {
            request_id,
            ready: None,
        });
    }

    /// 進行中（結果未着）の要求があれば `None`、無ければ確認と登録を同じロックで行い `request_id` を返す。
    /// 公開入口が `snapshot` → `begin` の 2 段で二重登録しないために使う。
    pub fn try_begin(&self) -> Option<u64> {
        self.try_begin_unless(|_| false)
    }

    /// [`RequestSlot::try_begin`] に加え、結果が届いて未取得の要求の id が `keep` を満たすときも `None`
    /// （その結果の持ち主がまだ取り出していないため、置き換えて落とさない）。判定と登録は同じロックで行う。
    pub fn try_begin_unless(&self, keep: impl FnOnce(u64) -> bool) -> Option<u64> {
        let mut pending = self.lock();
        if let Some(p) = pending.as_ref()
            && (p.ready.is_none() || keep(p.request_id))
        {
            return None;
        }
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        *pending = Some(Pending {
            request_id,
            ready: None,
        });
        Some(request_id)
    }

    /// OS からの結果を置く（任意スレッド）。`request_id` 不一致・未登録（プロセス死後の
    /// 再配送等）・既に結果あり（重複通知）は捨てて `false`。
    pub fn deliver(&self, request_id: u64, outcome: O) -> bool {
        let mut pending = self.lock();
        let Some(p) = pending.as_mut().filter(|p| p.request_id == request_id) else {
            return false;
        };
        if p.ready.is_some() {
            return false;
        }
        p.ready = Some(outcome);
        true
    }

    /// 現在の要求と、届いていれば結果（クローン）。effect ドライバが毎フレーム観測する。
    pub fn snapshot(&self) -> Option<(u64, Option<O>)>
    where
        O: Clone,
    {
        self.lock()
            .as_ref()
            .map(|p| (p.request_id, p.ready.clone()))
    }

    /// `request_id` が現在の要求で結果が届いていれば、照合と取り出しと消去を同じロックで行って結果を返す。
    /// 同じ id を複数スレッドが呼んでも、取り出せるのは 1 つだけ（`snapshot` + `clear` の 2 段では両方が取れてしまう）。
    pub fn take_ready(&self, request_id: u64) -> Option<O> {
        let mut pending = self.lock();
        if pending
            .as_ref()
            .is_some_and(|p| p.request_id == request_id && p.ready.is_some())
        {
            return pending.take().and_then(|p| p.ready);
        }
        None
    }

    /// 完了クロージャが画面へ反映できたときだけ呼ぶ。`request_id` が現在の要求と
    /// 一致するときのみ消す。
    pub fn clear(&self, request_id: u64) {
        let mut pending = self.lock();
        if pending.as_ref().is_some_and(|p| p.request_id == request_id) {
            *pending = None;
        }
    }

    /// 期限切れの確定用。`request_id` が現在の要求で、かつ結果が未着のときだけ消して `true`。
    /// 判定と除去を同じロック内で行い、`snapshot` 後に `deliver` が着地した結果を消さない
    /// （`false` なら呼び出し側は失敗を確定せず、後続の `poll_and_finish` に任せる）。
    pub fn clear_if_pending(&self, request_id: u64) -> bool {
        let mut pending = self.lock();
        if pending
            .as_ref()
            .is_some_and(|p| p.request_id == request_id && p.ready.is_none())
        {
            *pending = None;
            return true;
        }
        false
    }

    /// 照合なしで丸ごと捨てる。Android の Activity 再生成直後に呼ぶ想定
    /// （`biometric_auth::reset_pending_tracking` の唯一の呼び出し元、PR #917 の
    /// codex-review 指摘由来）。share / file_pick は呼ばない（既存挙動を変えない）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn reset(&self) {
        *self.lock() = None;
    }
}

/// share / biometric_auth の `drive_callback` 後半（「毎フレーム `snapshot` →
/// 未着なら `on_waiting` フック + `POLL_INTERVAL` で起床 → 完了クロージャで
/// `apply` → 反映できたときだけ `clear`」）の共通骨格。
///
/// `file_pick::drive_mobile` はここを通さない: Restore 2 腕（Activity 再生成中の
/// 復元）を持ち、かつ待機中に `request_repaint_after` を呼ばない設計（再描画は
/// JNI/FFI の受け口が行う）のため、寄せるとポーリングが増えて挙動が変わる
/// （Issue #943 実装計画 §3.4）。
pub fn poll_and_finish<S: 'static, O: Clone + Send + 'static>(
    slot: &'static RequestSlot<O>,
    sender: &crate::CompletionSender<S>,
    on_waiting: impl FnOnce(u64),
    apply: impl FnOnce(&mut S, O) -> bool + Send + 'static,
) {
    let Some((request_id, ready)) = slot.snapshot() else {
        return;
    };
    let Some(outcome) = ready else {
        on_waiting(request_id);
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    };
    let _ = sender.send(Box::new(move |s: &mut S| {
        if apply(s, outcome) {
            slot.clear(request_id);
        }
    }));
}

/// 画面用 effect が [`RequestSlot`] の要求のうち「自分の分」を追うための印。
///
/// 画面と公開入口（画面の状態型に依存しない入口）が同じ slot を共有する機能で、画面側が公開入口の
/// 進行中の要求を置き換えたり、公開入口の結果を取って `clear` したりしないために使う。
/// 画面側は [`SlotOwner::begin`] で依頼を登録し、反映は [`poll_and_finish_owned`] で自分の id の結果だけを取る。
pub struct SlotOwner(AtomicU64);

impl SlotOwner {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// 進行中の要求が無いときだけ登録して `request_id` を返し、自分の分として覚える。
    /// 先行（公開入口・別の画面要求）が進行中なら `None`（置き換えない）。呼び出し側は画面へ「処理中」の失敗を返す。
    pub fn begin<O>(&self, slot: &RequestSlot<O>) -> Option<u64> {
        let request_id = slot.try_begin()?;
        self.0.store(request_id, Ordering::Release);
        Some(request_id)
    }

    /// [`SlotOwner::begin`] に加え、`others` のいずれかの持ち主の結果が届いて未取得のときも `None`（取り出す前に置き換えて
    /// 落とさない。自分自身を含めれば、自分の未取得の結果も守れる）。画面と公開入口が 1 つの slot を共有し、片方が結果を取り出す方式（pull 型）の機能で使う。
    pub fn begin_excluding<O>(&self, slot: &RequestSlot<O>, others: &[&SlotOwner]) -> Option<u64> {
        let request_id = slot.try_begin_unless(|id| others.iter().any(|o| o.current() == id))?;
        self.0.store(request_id, Ordering::Release);
        Some(request_id)
    }

    /// [`SlotOwner::begin`] に加え、自分の分以外（公開入口）の結果が届いて未取得のときも `None`（取り出す前に置き換えて落とさない）。
    /// 自分の分の未取得の結果は置き換える。画面と公開入口が 1 つの slot を共有する pull 型の機能で、画面側の登録に使う。
    pub fn begin_keeping_foreign<O>(&self, slot: &RequestSlot<O>) -> Option<u64> {
        let request_id = slot.try_begin_unless(|id| id != self.current())?;
        self.0.store(request_id, Ordering::Release);
        Some(request_id)
    }

    /// 既に登録済みの `request_id`（`slot.begin()` の戻り値等）を自分の分として覚える。
    /// 先行の要求を置き換える画面用の登録で使う。
    pub fn adopt(&self, request_id: u64) {
        self.0.store(request_id, Ordering::Release);
    }

    /// 自分の分の `request_id`（無ければ `0`）。
    pub fn current(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// 自分の分の印を外す（`request_id` が現在の印のときだけ）。結果を取り出し終えたときに呼ぶ。
    pub fn release(&self, request_id: u64) {
        let _ = self
            .0
            .compare_exchange(request_id, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl Default for SlotOwner {
    fn default() -> Self {
        Self::new()
    }
}

/// [`poll_and_finish`] の自分の分だけ版。`owner` の id が slot の現在の要求のときだけ結果を取って反映する。
/// 自分の分が無ければ何もしない。slot の要求が別の id に置き換わっていた、または slot が空になっていた
/// （Activity 再生成の `reset` 等で自分の要求は失われた）ときは、印を外して `on_lost` を送り、失敗を確定させる。
pub fn poll_and_finish_owned<S: 'static, O: Clone + Send + 'static>(
    slot: &'static RequestSlot<O>,
    owner: &'static SlotOwner,
    sender: &crate::CompletionSender<S>,
    on_waiting: impl FnOnce(u64),
    apply: impl FnOnce(&mut S, O) -> bool + Send + 'static,
    on_lost: impl FnOnce(&mut S) + Send + 'static,
) {
    let mine = owner.current();
    if mine == 0 {
        return;
    }
    // 完了の反映と同じフレームで観測し直しても、`on_lost` は世代照合で捨てられる（`finish_*` は完了済みの画面を変えない）。
    if slot.snapshot().is_none_or(|(id, _)| id != mine) {
        owner.release(mine);
        let _ = sender.send(Box::new(on_lost));
        return;
    }
    poll_and_finish(slot, sender, on_waiting, move |s, outcome| {
        let done = apply(s, outcome);
        if done {
            owner.release(mine);
        }
        done
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn begin_with_registers_caller_id() {
        let slot: RequestSlot<u8> = RequestSlot::new();
        slot.begin_with(7);
        assert!(slot.deliver(7, 1));
        assert_eq!(slot.take_ready(7), Some(1));
    }

    #[test]
    fn try_begin_refuses_while_pending_and_allows_after_ready() {
        let slot: RequestSlot<u8> = RequestSlot::new();
        let id = slot.try_begin().expect("空き");
        assert!(slot.try_begin().is_none());
        assert!(slot.deliver(id, 1));
        assert!(slot.try_begin().is_some());
    }

    use super::*;

    #[test]
    fn begin_then_snapshot_has_no_result_yet() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        assert_eq!(slot.snapshot(), Some((id, None)));
    }

    #[test]
    fn begin_then_matching_deliver_places_result_in_snapshot() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        assert!(slot.deliver(id, 42));
        assert_eq!(slot.snapshot(), Some((id, Some(42))));
    }

    #[test]
    fn deliver_with_mismatched_request_id_is_rejected() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        assert!(!slot.deliver(id.wrapping_add(1), 42));
        assert_eq!(slot.snapshot(), Some((id, None)));
    }

    #[test]
    fn deliver_second_time_is_rejected_first_wins() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        assert!(slot.deliver(id, 1));
        assert!(!slot.deliver(id, 2));
        assert_eq!(slot.snapshot(), Some((id, Some(1))));
    }

    #[test]
    fn deliver_without_begin_is_discarded_as_process_death_redelivery() {
        let slot = RequestSlot::<u8>::new();
        assert!(!slot.deliver(1, 42));
        assert_eq!(slot.snapshot(), None);
    }

    #[test]
    fn double_begin_invalidates_old_request_id() {
        let slot = RequestSlot::<u8>::new();
        let old_id = slot.begin();
        let new_id = slot.begin();
        assert_ne!(old_id, new_id);
        assert!(!slot.deliver(old_id, 1));
        slot.clear(old_id);
        assert_eq!(
            slot.snapshot(),
            Some((new_id, None)),
            "旧 id の clear は新要求を消さない"
        );
    }

    #[test]
    fn clear_with_matching_request_id_removes_pending() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        slot.clear(id);
        assert_eq!(slot.snapshot(), None);
    }

    #[test]
    fn clear_with_mismatched_request_id_leaves_pending() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        slot.clear(id.wrapping_add(1));
        assert!(slot.snapshot().is_some());
    }

    #[test]
    fn reset_discards_pending_request_so_late_delivery_is_ignored() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        slot.reset();
        assert_eq!(slot.snapshot(), None);
        assert!(
            !slot.deliver(id, 42),
            "reset 後の旧 request_id は無視される"
        );
        assert_eq!(
            slot.snapshot(),
            None,
            "無視された deliver は新しい要求を作らない"
        );
    }

    #[test]
    fn clear_if_pending_keeps_delivered_result() {
        let slot = RequestSlot::<u8>::new();
        let id = slot.begin();
        assert!(!slot.clear_if_pending(id + 1));
        assert!(slot.deliver(id, 7));
        assert!(!slot.clear_if_pending(id));
        assert_eq!(slot.snapshot(), Some((id, Some(7))));
        let id2 = slot.begin();
        assert!(slot.clear_if_pending(id2));
        assert_eq!(slot.snapshot(), None);
    }

    static POLL_SLOT: RequestSlot<u8> = RequestSlot::new();
    /// `POLL_SLOT` はプロセス内で 1 つの static のため、このグループのテストのみ
    /// 直列化する（`RequestSlot` 単体のテストはローカルインスタンスのため不要）。
    static POLL_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl crate::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    fn sender_with_channel() -> (
        crate::CompletionSender<u8>,
        std::sync::mpsc::Receiver<crate::Completion<u8>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            crate::CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    #[test]
    fn owned_poll_reports_lost_when_slot_was_reset() {
        static SLOT: RequestSlot<u8> = RequestSlot::new();
        static OWNER: SlotOwner = SlotOwner::new();
        let (sender, rx) = sender_with_channel();
        let id = OWNER.begin(&SLOT).expect("空き");
        // 要求中に slot が空になる（Activity 再生成）。
        SLOT.reset();
        poll_and_finish_owned(
            &SLOT,
            &OWNER,
            &sender,
            |_| panic!("呼ばれない"),
            |_s, _o: u8| true,
            |s: &mut u8| *s = 9,
        );
        let mut state = 0u8;
        rx.try_recv().expect("on_lost が送られる")(&mut state);
        assert_eq!(state, 9);
        assert_eq!(OWNER.current(), 0);
        // 印が外れた後は何もしない。
        poll_and_finish_owned(&SLOT, &OWNER, &sender, |_| {}, |_s, _o: u8| true, |_s| {});
        assert!(rx.try_recv().is_err());
        let _ = id;
    }

    #[test]
    fn poll_and_finish_is_noop_without_pending_request() {
        let _guard = POLL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = POLL_SLOT.begin();
        POLL_SLOT.clear(id);

        let (sender, rx) = sender_with_channel();
        poll_and_finish(
            &POLL_SLOT,
            &sender,
            |_| panic!("呼ばれない"),
            |_s, _o: u8| true,
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn poll_and_finish_calls_on_waiting_and_reschedules_when_not_ready() {
        let _guard = POLL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = POLL_SLOT.begin();

        let (sender, rx) = sender_with_channel();
        let mut called_with = None;
        poll_and_finish(
            &POLL_SLOT,
            &sender,
            |request_id| called_with = Some(request_id),
            |_s, _o: u8| true,
        );
        assert_eq!(called_with, Some(id));
        assert!(rx.try_recv().is_err());

        POLL_SLOT.clear(id);
    }

    #[test]
    fn poll_and_finish_clears_only_when_apply_reports_true() {
        let _guard = POLL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = POLL_SLOT.begin();
        assert!(POLL_SLOT.deliver(id, 7u8));

        let (sender, rx) = sender_with_channel();
        poll_and_finish(
            &POLL_SLOT,
            &sender,
            |_| {},
            |s: &mut u8, o| {
                *s = o;
                false
            },
        );
        let apply = rx.try_recv().expect("送信されている");
        let mut state = 0u8;
        apply(&mut state);
        assert_eq!(state, 7);
        assert_eq!(
            POLL_SLOT.snapshot(),
            Some((id, Some(7))),
            "apply=false なので残る"
        );

        POLL_SLOT.clear(id);
    }

    #[test]
    fn poll_and_finish_clears_when_apply_reports_true_result() {
        let _guard = POLL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = POLL_SLOT.begin();
        assert!(POLL_SLOT.deliver(id, 9u8));

        let (sender, rx) = sender_with_channel();
        poll_and_finish(
            &POLL_SLOT,
            &sender,
            |_| {},
            |s: &mut u8, o| {
                *s = o;
                true
            },
        );
        let apply = rx.try_recv().expect("送信されている");
        let mut state = 0u8;
        apply(&mut state);
        assert_eq!(state, 9);
        assert_eq!(POLL_SLOT.snapshot(), None, "apply=true なので clear される");
    }

    #[test]
    fn owner_does_not_replace_pending_and_is_lost_when_replaced() {
        static SLOT: RequestSlot<u8> = RequestSlot::new();
        static OWNER: SlotOwner = SlotOwner::new();
        // 公開入口側の依頼が進行中なら、画面側は置き換えられない。
        let public = SLOT.try_begin().expect("空き");
        assert!(OWNER.begin(&SLOT).is_none());
        assert_eq!(OWNER.current(), 0);
        assert!(SLOT.snapshot().is_some_and(|(id, _)| id == public));
        SLOT.deliver(public, 1);
        // 結果が置かれた後は画面側が登録でき、以後の要求は自分の id になる。
        let mine = OWNER.begin(&SLOT).expect("空き");
        assert_eq!(OWNER.current(), mine);
        assert_ne!(mine, public);
        SLOT.reset();
    }

    #[test]
    fn begin_excluding_keeps_other_owners_unconsumed_result() {
        static SLOT: RequestSlot<u8> = RequestSlot::new();
        static A: SlotOwner = SlotOwner::new();
        static B: SlotOwner = SlotOwner::new();
        let a = A.begin_excluding(&SLOT, &[&B]).expect("空き");
        // 結果が届いても A が取り出すまで、B は置き換えられない。
        assert!(SLOT.deliver(a, 7));
        assert!(B.begin_excluding(&SLOT, &[&A]).is_none());
        assert!(
            SLOT.snapshot()
                .is_some_and(|(id, r)| id == a && r == Some(7))
        );
        // 取り出して clear した後は B が登録できる。
        SLOT.clear(a);
        assert!(B.begin_excluding(&SLOT, &[&A]).is_some());
        SLOT.reset();
    }
}
