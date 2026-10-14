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
//! 4 OS（macOS/iOS/Android/Windows）以外では利用者が無いため、`mod request_slot;`
//! 宣言（`device.rs`）に `allow(dead_code)` を付けて許容する。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// share / biometric_auth の `drive_callback` 後半で共通のポーリング間隔
/// （旧: 各モジュールが個別に持っていた `POLL_INTERVAL`）。
pub(crate) const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

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
pub(crate) struct RequestSlot<O> {
    pending: Mutex<Option<Pending<O>>>,
    /// 0 は「未要求」の番兵として使わない（既存 3 実装と同じ）。
    next_request_id: AtomicU64,
}

impl<O> RequestSlot<O> {
    /// static 初期化用。
    pub(crate) const fn new() -> Self {
        Self {
            pending: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Pending<O>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin(&self) -> u64 {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        *self.lock() = Some(Pending {
            request_id,
            ready: None,
        });
        request_id
    }

    /// OS からの結果を置く（任意スレッド）。`request_id` 不一致・未登録（プロセス死後の
    /// 再配送等）・既に結果あり（重複通知）は捨てて `false`。
    pub(crate) fn deliver(&self, request_id: u64, outcome: O) -> bool {
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
    pub(crate) fn snapshot(&self) -> Option<(u64, Option<O>)>
    where
        O: Clone,
    {
        self.lock()
            .as_ref()
            .map(|p| (p.request_id, p.ready.clone()))
    }

    /// 完了クロージャが画面へ反映できたときだけ呼ぶ。`request_id` が現在の要求と
    /// 一致するときのみ消す。
    pub(crate) fn clear(&self, request_id: u64) {
        let mut pending = self.lock();
        if pending.as_ref().is_some_and(|p| p.request_id == request_id) {
            *pending = None;
        }
    }

    /// 照合なしで丸ごと捨てる。Android の Activity 再生成直後に呼ぶ想定
    /// （`biometric_auth::reset_pending_tracking` の唯一の呼び出し元、PR #917 の
    /// codex-review 指摘由来）。share / file_pick は呼ばない（既存挙動を変えない）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset(&self) {
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
pub(crate) fn poll_and_finish<S: 'static, O: Clone + Send + 'static>(
    slot: &'static RequestSlot<O>,
    sender: &crate::app::CompletionSender<S>,
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

#[cfg(test)]
mod tests {
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

    static POLL_SLOT: RequestSlot<u8> = RequestSlot::new();
    /// `POLL_SLOT` はプロセス内で 1 つの static のため、このグループのテストのみ
    /// 直列化する（`RequestSlot` 単体のテストはローカルインスタンスのため不要）。
    static POLL_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn sender_with_channel() -> (
        crate::app::CompletionSender<u8>,
        std::sync::mpsc::Receiver<crate::app::Completion<u8>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (crate::app::CompletionSender::new(tx, ctx), rx)
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
}
