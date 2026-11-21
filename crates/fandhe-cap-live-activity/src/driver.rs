//! Phase 11 の拡充ループ（親 #88、Issue #294）型サンプル「ライブアクティビティ」の effect ドライバ。
//! `fandhe_core::screens::live_activity_show`（端末依存ゼロ）の要求を観測し、値を
//! `crate::contract::LiveValue` で検査してから OS へ渡し、結果を `CompletionSender`
//! 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `fandhe_device::live_activity` のモジュール doc を参照。
//!
//! # dispatch（最終形）
//!
//! [`platform_live_activity`] が OS ごとに実装を選ぶ。iOS は `ios_live_activity::implementation()`、
//! Android は `android_live_activity::implementation()`、それ以外（macOS・Windows・Linux）は
//! `UnsupportedLiveActivity` 固定。**後続の b 層は各 OS モジュールの `implementation()` だけを埋め**、
//! 本ファイル・`lib.rs` の `mod` 列・共通コア画面には触らない。実装は完了時に
//! [`bridge::deliver`] を 1 回だけ呼ぶ（pull 型。`request_slot::RequestSlot` の facade）。
//!
//! # 並行性
//!
//! 要求の照合（現在の `request_id` か・結果済みか）は `RequestSlot` のロックの内側で行い、
//! OS 呼び出し（`submit`）はロックの外で行う。処理中（結果未着）に届いた要求は二重に渡さず、完了後に
//! 取り込む。

use std::sync::{Arc, OnceLock};

use crate::contract::{LiveActivity, LiveOperation, LiveOutcome, LiveValue};
use crate::error::LiveActivityError;
use fandhe_core::screens::live_activity_show::{
    LiveActivityShowState, LiveCommand, LiveResult, apply_result, mark_command_taken, take_command,
};

use fandhe_runtime::CompletionSender;
use fandhe_runtime::request_slot::{
    POLL_INTERVAL, PublicEntry, RequestSlot, SlotOwner, poll_and_finish_owned,
};

/// OS ごとの実装を 1 つだけ持つ（iOS・Android は各 OS 実装、他 OS は非対応）。
pub fn platform_live_activity() -> &'static Arc<dyn LiveActivity> {
    register_teardown();
    static INSTANCE: OnceLock<Arc<dyn LiveActivity>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        #[cfg(target_os = "ios")]
        return crate::ios::implementation();
        #[cfg(target_os = "android")]
        return crate::android::implementation();
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        return Arc::new(crate::contract::UnsupportedLiveActivity);
    })
}

/// 画面の状態型に依存しない入口（`docs/architecture/consumer-app-structure.md` §8.2）。操作を OS へ渡し、
/// 開始成功・権限拒否・OS エラーなどの完了まで block して返す（期限は置かない。OS の操作は取り消せず、推測の期限で失敗を返すと
/// 後から開始されて再試行で重複するため、OS の結果か再生成まで待つ）。先行が未完了の間の後続は
/// `Internal("live_activity busy")` で拒否する。UI スレッドから呼ばない。Activity 再生成で待っている呼び出しは
/// `Internal("live_activity reset")` で戻る。待ちを打ち切りたいときは [`begin_live_activity`] / [`poll_live_activity`] /
/// [`discard_live_activity`] を使う。
pub fn run_live_activity(op: LiveOperation) -> Result<LiveOutcome, LiveActivityError> {
    register_teardown();
    let request_id = begin_live_activity(op)?;
    bridge::PUBLIC.wait(&bridge::SLOT, request_id, bridge::lost)
}

/// 公開入口: 操作を OS へ渡し、結果を受け取るための要求 id を返す。結果は [`poll_live_activity`] で 1 度だけ取り出す。
/// 待ちを打ち切るときは [`discard_live_activity`] で捨てる（同期の [`run_live_activity`] は OS の完了まで待つ。打ち切れるのはこちら）。
///
/// - 進行中の要求（画面経由を含む）と、自分の結果が未取得のうちは `Internal("live_activity busy")` で断る
/// - 画面側の届いた未取得の結果は捨てずに画面の受け取り待ちへ移して始める（画面はそのまま受け取る）
/// - 依頼自体の失敗は `Err`（結果は届かない）。非対応の OS は `Unsupported`
pub fn begin_live_activity(op: LiveOperation) -> Result<u64, LiveActivityError> {
    register_teardown();
    let request_id =
        bridge::begin_public().ok_or(LiveActivityError::Internal("live_activity busy"))?;
    if let Err(e) = platform_live_activity().submit(request_id, op) {
        bridge::PUBLIC.abort(&bridge::SLOT, request_id);
        return Err(e);
    }
    Ok(request_id)
}

/// [`begin_live_activity`] の結果を取り出す。届いていれば 1 度だけ `Some`。未着（`None` のまま）なら OS はまだ進行中で、
/// 待てないときは [`discard_live_activity`] で打ち切る。旧 id・公開入口でない id・取り出し済み・捨てた id・0 は `None`。
/// 要求が失われた（Android の Activity 再生成）ときは `Err(Internal("live_activity reset"))` を 1 度だけ返す。
///
/// 打ち切った（`discard` した）要求は、OS 側で後から開始・更新・終了されうる（未確定）。そのため結果は届かない。
/// 並行して同じ id を poll しても、結果が二重に返ることはない。
pub fn poll_live_activity(request_id: u64) -> Option<Result<LiveOutcome, LiveActivityError>> {
    register_teardown();
    bridge::PUBLIC.poll(&bridge::SLOT, request_id, bridge::lost)
}

/// [`begin_live_activity`] の自分の要求を捨てて待ちを打ち切る。`request_id` が公開入口の現在の要求のときだけ `true`
/// （他人の id・取り出し済み・0 は `false`）。以後 [`poll_live_activity`] は `None`。
///
/// 外すのは受け取りの印だけ（OS が進行中なら slot は残す）で、OS の操作は取り消せない。捨てた要求は OS 側で後から完了しうる（未確定）。
/// 後から届いた結果は id で照合して捨て、新しい要求へ結びつけない。OS がまだ進行中の間は、次の開始
/// （画面・公開とも）は `Internal("live_activity busy")` で断られる（OS 側が置き換えて重複させないため）。
pub fn discard_live_activity(request_id: u64) -> bool {
    register_teardown();
    bridge::PUBLIC.discard(&bridge::SLOT, request_id)
}

/// `LiveActivityError` / 成功結果を画面用の結果へ写像する。`Internal` と未知のエラー（`Cancelled` を含む）は
/// 固定文言の失敗にし、OS の文言は入れない（security.md A09）。
fn to_result(result: Result<LiveOutcome, LiveActivityError>) -> LiveResult {
    match result {
        Ok(LiveOutcome::Started) => LiveResult::Started,
        Ok(LiveOutcome::Updated) => LiveResult::Updated,
        Ok(LiveOutcome::Ended) => LiveResult::Ended,
        Ok(LiveOutcome::Dismissed) => LiveResult::Dismissed,
        Err(LiveActivityError::PermissionDenied) => LiveResult::Denied,
        Err(LiveActivityError::Unsupported) => LiveResult::Unsupported,
        Err(LiveActivityError::InvalidInput) => LiveResult::OutOfRange,
        Err(_) => LiveResult::Failed("OS のライブ表示の操作に失敗しました".to_string()),
    }
}

/// OS 側との橋渡し（全 OS で同じ経路を通す、pull 型、`RequestSlot` の facade）。
/// 本機能専用の [`SLOT`](bridge::SLOT) を、画面 effect（`SCREEN_OWNER`）と公開入口（[`begin_live_activity`]・
/// 同期の [`run_live_activity`]、`PUBLIC`）が持ち主の印で分けて共有する。結果は OS の生の値で置き、
/// 画面用への写像（[`to_result`]）は画面が取り出すときに行う。
pub(crate) mod bridge {
    use super::*;

    pub(crate) type Outcome = Result<LiveOutcome, LiveActivityError>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();
    /// 画面 effect が追う自分の分の要求。
    pub(crate) static SCREEN_OWNER: SlotOwner = SlotOwner::new();
    /// 公開入口が追う自分の分の要求（poll・discard の直列化を含む）。
    pub(crate) static PUBLIC: PublicEntry = PublicEntry::new();

    /// 画面側の開始。進行中の要求と、公開側の未取得の結果は置き換えない（OS 側は同時に 1 要求しか持たず、
    /// 後続が先行を無通知で置き換えるため）。自分の未取得の結果は置き換える。
    pub(super) fn begin_screen() -> Option<u64> {
        SCREEN_OWNER.begin_excluding(&SLOT, &[PUBLIC.owner()])
    }

    /// 公開入口の開始。進行中の要求と自分の未取得の結果があれば `None`（busy）。画面側の届いた未取得の結果は
    /// 捨てずに画面の受け取り待ちへ移す。
    pub(super) fn begin_public() -> Option<u64> {
        PUBLIC.begin(&SLOT, &SCREEN_OWNER)
    }

    /// OS からの結果を置く。`request_id` 不一致・未登録・既に結果ありは捨てて `false`（先に届いたものだけを採る）。
    /// 打ち切られた要求の遅れた結果は持ち主がいないため、次の開始が置き換えて捨てる。
    /// b 層の実装は完了時に 1 回だけ呼ぶ。
    #[allow(dead_code)]
    pub(crate) fn deliver(request_id: u64, result: Outcome) -> bool {
        SLOT.deliver(request_id, result)
    }

    /// 失われた要求を公開側へ知らせる値。
    pub(super) fn lost() -> Outcome {
        Err(LiveActivityError::Internal("live_activity reset"))
    }
}

/// 画面の依頼を観測して OS へ渡し、届いた結果を画面へ反映する。
fn drive(
    state: &LiveActivityShowState,
    sender: &CompletionSender<LiveActivityShowState>,
    ui: &dyn LiveActivity,
) {
    // 自分の要求が進行中の間は次の依頼を取らない（結果の反映後に取り込む）。
    if bridge::SCREEN_OWNER.current() == 0
        && let Some((command, generation)) = take_command(state)
    {
        let _ = sender.send(Box::new(move |s: &mut LiveActivityShowState| {
            mark_command_taken(s, generation)
        }));
        // 公開入口の要求が進行中・未取得のときは OS へ渡さず、画面へ処理中の失敗を返す。
        let Some(request_id) = bridge::begin_screen() else {
            let result = to_result(Err(LiveActivityError::Internal("live_activity busy")));
            let _ = sender.send(Box::new(move |s: &mut LiveActivityShowState| {
                apply_result(s, generation, result)
            }));
            return;
        };
        // 範囲外の値は OS へ渡さずその場で画面へ返す。
        let accepted = to_operation(command).and_then(|op| ui.submit(request_id, op));
        if let Err(e) = accepted {
            bridge::deliver(request_id, Err(e));
        }
        // 結果は次フレーム以降の poll で拾う。入力がなくても起こす。
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }

    let generation = state.generation;
    poll_and_finish_owned(
        &bridge::SLOT,
        &bridge::SCREEN_OWNER,
        sender,
        |_| {},
        move |s: &mut LiveActivityShowState, result| {
            // 古い世代の完了は `apply_result` が捨てる。詰まらせないよう常に消す。
            apply_result(s, generation, to_result(result));
            true
        },
        // 自分の要求が失われた（Activity 再生成の reset 等）。待ち続けず失敗で終える。
        move |s: &mut LiveActivityShowState| {
            apply_result(
                s,
                generation,
                to_result(Err(LiveActivityError::Internal("live_activity reset"))),
            );
        },
    );
}

/// 画面の要求を検証済みの操作へ変換する。OS 型へ変換する前の唯一の検証点を通す。
fn to_operation(command: LiveCommand) -> Result<LiveOperation, LiveActivityError> {
    Ok(match command {
        LiveCommand::Start(v) => LiveOperation::Start(LiveValue::new(v)?),
        LiveCommand::Update(v) => LiveOperation::Update(LiveValue::new(v)?),
        LiveCommand::End => LiveOperation::End,
    })
}

/// `registry.rs` から `live_activity_show` 画面用に配線される effect ドライバ。
pub fn live_activity_show_effect(
    state: &LiveActivityShowState,
    sender: &CompletionSender<LiveActivityShowState>,
) {
    register_teardown();
    drive(state, sender, platform_live_activity().as_ref());
}

/// Android: Activity 再生成直後に登録表（`fandhe_runtime::teardown`）から呼ばれる。保留中の要求は復元せず破棄して以後届く旧
/// `request_id` の完了を無視させ、OS 側の表示も閉じる。再生成後の画面は `Idle` から始まるため、
/// 表示を残すと「開始」の次の操作と食い違う（契約表 1・9）。
pub fn reset_pending_tracking() {
    bridge::SLOT.reset();
    platform_live_activity().dismiss_all();
}

/// 終了処理と Android の再生成時の破棄を登録表（`fandhe_runtime::teardown`）へ登録する（Issue #1453）。
/// 公開の入口・effect・公開型のメソッドの先頭で呼ぶ。初めて使われたときに登録され、2 回目以降は何もしない。
pub(crate) fn register_teardown() {
    #[cfg(target_os = "android")]
    fandhe_runtime::teardown::register(
        fandhe_runtime::teardown::Teardown::new("live-activity")
            .on_android_recreate(crate::reset_pending_tracking),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedLiveActivity;
    use fandhe_core::screens::live_activity_show::{
        LivePhase, end, error_text, start, status_text, update,
    };
    use std::sync::Mutex;

    /// `bridge` のプロセス共有 static（slot・持ち主）を触るテストを直列化し、始める前に空へ戻す。
    static BRIDGE_LOCK: Mutex<()> = Mutex::new(());

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let g = BRIDGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        bridge::SLOT.reset();
        bridge::SCREEN_OWNER.release(bridge::SCREEN_OWNER.current());
        bridge::PUBLIC
            .owner()
            .release(bridge::PUBLIC.owner().current());
        g
    }

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    /// `submit` の呼び出しを記録する偽実装。`submit_error` が `Some` なら依頼自体を失敗させる。
    struct Fake {
        calls: Mutex<Vec<(u64, LiveOperation)>>,
        submit_error: Option<fn() -> LiveActivityError>,
    }

    impl Fake {
        fn new(submit_error: Option<fn() -> LiveActivityError>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                submit_error,
            }
        }
        fn count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl LiveActivity for Fake {
        fn submit(&self, request_id: u64, op: LiveOperation) -> Result<(), LiveActivityError> {
            self.calls.lock().unwrap().push((request_id, op));
            match self.submit_error {
                Some(f) => Err(f()),
                None => Ok(()),
            }
        }
        fn dismiss_all(&self) {}
    }

    type St = LiveActivityShowState;

    fn channel() -> (
        CompletionSender<St>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<St>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    /// 1 回の `drive` で送られた完了を全部状態へ適用する。
    fn pump(
        state: &mut St,
        ui: &dyn LiveActivity,
        tx: &CompletionSender<St>,
        rx: &std::sync::mpsc::Receiver<fandhe_runtime::Completion<St>>,
    ) {
        let snapshot = state.clone();
        drive(&snapshot, tx, ui);
        while let Ok(c) = rx.try_recv() {
            c(state);
        }
    }

    fn screen_id() -> u64 {
        bridge::SCREEN_OWNER.current()
    }

    #[test]
    fn no_request_does_not_call_os() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(ui.count(), 0);
        assert_eq!(st, St::default());
    }

    #[test]
    fn flow_start_update_dismissed() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(ui.count(), 1);
        assert_eq!(st.phase, LivePhase::Starting);
        assert!(bridge::deliver(screen_id(), Ok(LiveOutcome::Started)));
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Active { value: 0 });
        assert_eq!(screen_id(), 0);

        update(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        // 閉じられていた: 失敗ではなく正常終了。
        assert!(bridge::deliver(screen_id(), Ok(LiveOutcome::Dismissed)));
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert_eq!(error_text(&st), "");
        assert!(!status_text(&st).is_empty());
    }

    #[test]
    fn out_of_range_does_not_call_os() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        st.phase = LivePhase::Active { value: 9999 };
        update(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(ui.count(), 0);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Active { value: 9999 });
        assert!(!error_text(&st).is_empty());
    }

    #[test]
    fn permission_denied_is_error_not_success() {
        let _g = fresh();
        let ui = Fake::new(Some(|| LiveActivityError::PermissionDenied));
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
        assert_eq!(status_text(&st), "");
    }

    #[test]
    fn unsupported_live_activity_never_succeeds_through_driver() {
        let _g = fresh();
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &UnsupportedLiveActivity, &tx, &rx);
        pump(&mut st, &UnsupportedLiveActivity, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
    }

    #[test]
    fn stale_request_id_and_duplicate_delivery_are_dropped() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        let id = screen_id();
        assert!(!bridge::deliver(id + 1, Ok(LiveOutcome::Started)));
        assert!(bridge::deliver(
            id,
            Err(LiveActivityError::Internal("live_activity start"))
        ));
        assert!(!bridge::deliver(id, Ok(LiveOutcome::Started)));
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
    }

    #[test]
    fn request_during_pending_is_not_submitted_twice() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(ui.count(), 1);
        // 結果未着のまま、未発送の要求が再び見えても渡さない。
        st.command = Some(LiveCommand::Start(0));
        pump(&mut st, &ui, &tx, &rx);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(ui.count(), 1);
    }

    /// 画面の要求が `reset` で失われたら、処理中のまま止まらず失敗で終え、持ち主の印も外す。
    #[test]
    fn screen_request_lost_by_reset_ends_in_failure() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        start(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Starting);
        assert_ne!(screen_id(), 0);
        bridge::SLOT.reset();
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
        assert_eq!(screen_id(), 0);
    }

    #[test]
    fn to_result_covers_all_paths() {
        assert_eq!(to_result(Ok(LiveOutcome::Started)), LiveResult::Started);
        assert_eq!(to_result(Ok(LiveOutcome::Updated)), LiveResult::Updated);
        assert_eq!(to_result(Ok(LiveOutcome::Ended)), LiveResult::Ended);
        assert_eq!(to_result(Ok(LiveOutcome::Dismissed)), LiveResult::Dismissed);
        assert_eq!(
            to_result(Err(LiveActivityError::PermissionDenied)),
            LiveResult::Denied
        );
        assert_eq!(
            to_result(Err(LiveActivityError::Unsupported)),
            LiveResult::Unsupported
        );
        assert_eq!(
            to_result(Err(LiveActivityError::InvalidInput)),
            LiveResult::OutOfRange
        );
        for e in [
            LiveActivityError::Internal("live_activity start"),
            LiveActivityError::Cancelled,
        ] {
            assert!(matches!(to_result(Err(e)), LiveResult::Failed(_)));
        }
    }

    #[test]
    fn end_is_submitted_as_end() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St {
            phase: LivePhase::Active { value: 3 },
            ..St::default()
        };
        end(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        assert!(matches!(ui.calls.lock().unwrap()[0].1, LiveOperation::End));
    }

    fn poll(id: u64) -> Option<Result<LiveOutcome, LiveActivityError>> {
        poll_live_activity(id)
    }

    /// 打ち切り: poll は `None` のまま戻れ、OS が進行中の間は次の開始が busy。後から届いた結果は捨て、
    /// 次の要求へ結びつかない（重複して開始させない）。
    #[test]
    fn discard_gives_up_waiting_and_drops_the_late_result() {
        let _g = fresh();
        let first = bridge::begin_public().unwrap();
        assert_eq!(poll(first), None);
        assert!(discard_live_activity(first));
        assert_eq!(poll(first), None);
        assert!(!discard_live_activity(first));
        // OS はまだ進行中: 画面も公開も始められない。
        assert!(bridge::begin_public().is_none());
        assert!(bridge::begin_screen().is_none());
        // 遅れて完了。結果は持ち主がいない。
        assert!(bridge::deliver(first, Ok(LiveOutcome::Started)));
        let second = bridge::begin_public().expect("OS の完了後は次を受け付ける");
        assert_ne!(first, second);
        assert_eq!(poll(first), None);
        assert!(!bridge::deliver(first, Ok(LiveOutcome::Started)));
        assert_eq!(poll(second), None, "古い結果が新しい要求へ結びつかない");
        assert!(bridge::deliver(second, Ok(LiveOutcome::Ended)));
        assert_eq!(poll(second), Some(Ok(LiveOutcome::Ended)));
    }

    /// 同期の待ち（`run_live_activity` が使う `wait`）は、結果が届けば戻り、再生成の `reset` でも
    /// `live_activity reset` で戻る（止まったままにならない）。
    #[test]
    fn sync_wait_returns_on_result_and_on_reset() {
        let _g = fresh();
        let id = bridge::begin_public().unwrap();
        let h = std::thread::spawn(move || bridge::PUBLIC.wait(&bridge::SLOT, id, bridge::lost));
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(bridge::deliver(id, Ok(LiveOutcome::Started)));
        assert_eq!(h.join().unwrap(), Ok(LiveOutcome::Started));

        let id = bridge::begin_public().unwrap();
        let h = std::thread::spawn(move || bridge::PUBLIC.wait(&bridge::SLOT, id, bridge::lost));
        std::thread::sleep(std::time::Duration::from_millis(60));
        bridge::SLOT.reset();
        assert_eq!(
            h.join().unwrap(),
            Err(LiveActivityError::Internal("live_activity reset"))
        );
        assert!(bridge::begin_public().is_some());
    }

    /// 公開入口の進行中は画面の依頼を OS へ渡さず、処理中の失敗を画面へ返す。画面の進行中は公開入口も断る。
    #[test]
    fn screen_and_public_exclude_each_other() {
        let _g = fresh();
        let ui = Fake::new(None);
        let (tx, rx) = channel();
        let mut st = St::default();
        let public = bridge::begin_public().unwrap();
        start(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(ui.count(), 0);
        assert_eq!(st.phase, LivePhase::Idle);
        assert!(!error_text(&st).is_empty());
        assert!(bridge::deliver(public, Ok(LiveOutcome::Started)));
        assert_eq!(poll(public), Some(Ok(LiveOutcome::Started)));

        start(&mut st);
        pump(&mut st, &ui, &tx, &rx);
        assert_eq!(ui.count(), 1);
        assert!(bridge::begin_public().is_none());
    }
}
