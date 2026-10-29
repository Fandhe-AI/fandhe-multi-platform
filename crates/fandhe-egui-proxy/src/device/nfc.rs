//! Phase 11（親 #157）型サンプル「NFC タグ読み取り」（Issue #1044）の effect ドライバ。
//! `fandhe_core::screens::nfc_read`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::nfc::NfcReader` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::nfc`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch は #1044 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - Android: `android_nfc`（Issue #1045。`NfcReadHelper` を JNI で橋渡し）
//! - iOS・Windows: 対応表で保留（スタブのモジュールは作らず `Unsupported` 固定）。macOS は対象外、Linux は非対応
//! - Web: 本ドライバを通らず `fandhe-web-dom` が受け持つ（`NDEFReader`）
//!
//! # 期限（契約表 3）
//!
//! 起点は要求を観測した時の `Instant`（単調時計）。ワーカーへは残り時間を渡し、期限後に届いた
//! 成功は `TimedOut` に倒す（`Disabled` / `Interrupted` 等の失敗はそのまま通す）。OS 層は
//! リスナーを解除し終えてから戻る契約のため、フレーム側の安全網は `deadline + SETTLE_GRACE`。
//! 安全網で `TimedOut` を確定しても、`os_active` が下りるまで次の要求は発送しない。
//! 確定後にワーカーが返す結果は画面側の世代・状態照合（`finish_read`）で捨てられる。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::nfc_read::{
    finish_read, mark_running, take_request, CoreFailure, NfcReadState, Phase, ShownRecord,
    ShownTag,
};
use fandhe_device::nfc::{
    NdefRecordView, NfcReadFailure, NfcReadOutcome, NfcReader, READ_DEADLINE,
};

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

/// OS 層が期限後にリスナーを解除し終えるまでの猶予。仮の値で、#1040 で見直す。
/// ponytail: 固定 5 秒。OS 層の解除が常にこれ以内に終わる保証は無い（超えても次の発送は `os_active` が止める）。
const SETTLE_GRACE: Duration = Duration::from_secs(5);

type Outcome = Result<NfcReadOutcome, NfcReadFailure>;

/// 現在のプラットフォームに応じた `NfcReader` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn NfcReader> {
    #[cfg(target_os = "android")]
    {
        super::android_nfc::implementation()
    }
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(fandhe_device::nfc::UnsupportedNfcReader)
    }
}

/// ドライバの可変状態。要求の追跡は [`RequestSlot`] に任せ、ここは期限の起点だけを持つ。
/// テストが並列で干渉しないよう static から切り出して引数で渡す
/// （`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Session {
    slot: &'static RequestSlot<Outcome>,
    /// 進行中の要求を観測した時刻（単調時計）。
    started: Mutex<Option<Instant>>,
    /// 前回ワーカーの終了待ちで発送できずにいる要求（世代, 観測した時刻）。待機にも期限を適用する起点。
    waiting: Mutex<Option<(u64, Instant)>>,
    /// OS 操作を持つワーカーが走っている間 true。安全網でスロットを空にしても、
    /// ワーカー（リスナー解除まで済ませてから戻る）が終わるまで次の発送を止める。
    os_active: Arc<AtomicBool>,
    /// 期限に足すフレーム側の猶予。
    grace: Duration,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            started: Mutex::new(None),
            waiting: Mutex::new(None),
            os_active: Arc::new(AtomicBool::new(false)),
            grace: SETTLE_GRACE,
        }
    }
}

static SESSION: OnceLock<Session> = OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、保留中の要求を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、保留が残ると次の押下が発送されず飲み込まれる。
/// 古いワーカーの結果は照合で捨てられる。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.slot.reset();
        *s.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *s.waiting.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// 契約上の失敗を画面の失敗種別へ写す（タグの中身・OS の文言は元々持たない）。
fn to_core_failure(f: NfcReadFailure) -> CoreFailure {
    match f {
        NfcReadFailure::Unreadable => CoreFailure::Unreadable,
        NfcReadFailure::TooLarge => CoreFailure::TooLarge,
        NfcReadFailure::Disabled => CoreFailure::Disabled,
        NfcReadFailure::PermissionDenied => CoreFailure::PermissionDenied,
        NfcReadFailure::TimedOut => CoreFailure::TimedOut,
        NfcReadFailure::Interrupted => CoreFailure::Interrupted,
        NfcReadFailure::Unsupported => CoreFailure::Unsupported,
        NfcReadFailure::Failed => CoreFailure::Failed,
    }
}

fn to_core_record(r: NdefRecordView) -> ShownRecord {
    match r {
        NdefRecordView::Text { lang, text } => ShownRecord::Text { lang, text },
        NdefRecordView::Uri(u) => ShownRecord::Uri(u),
        NdefRecordView::Mime { type_name, len } => ShownRecord::Mime { type_name, len },
        NdefRecordView::External { type_name, len } => ShownRecord::External { type_name, len },
        NdefRecordView::Other { type_name, len } => ShownRecord::Other { type_name, len },
    }
}

/// ワーカーの本体: 読み取り → 期限の照合。期限後に届いた成功は成功にしない。
fn run_worker(imp: &dyn NfcReader, started: Instant, deadline: Duration) -> Outcome {
    let result = imp.read_one(deadline.saturating_sub(started.elapsed()));
    match result {
        Ok(_) if started.elapsed() > deadline => Err(NfcReadFailure::TimedOut),
        r => r,
    }
}

/// 完了を画面状態へ反映する。
fn apply_outcome(s: &mut NfcReadState, generation: u64, outcome: Outcome) {
    let result = match outcome {
        Ok(NfcReadOutcome::Read(rs)) => Ok(ShownTag::Records(
            rs.into_iter().map(to_core_record).collect(),
        )),
        Ok(NfcReadOutcome::EmptyTag) => Ok(ShownTag::Empty),
        Err(f) => Err(to_core_failure(f)),
    };
    finish_read(s, generation, result);
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &NfcReadState,
    sender: &CompletionSender<NfcReadState>,
    imp: &Arc<dyn NfcReader>,
    session: &Session,
    deadline: Duration,
) {
    // フレーム側の安全網。OS 層が解除を終えて戻る前に結果を確定しない猶予を足す。
    let frame_deadline = deadline + session.grace;
    let slot = session.slot;
    if let Some(generation) = take_request(state) {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if slot.snapshot().is_some() {
            return;
        }
        // 期限の起点は要求を観測した時刻。前回ワーカーの終了待ちの間も同じ起点で測る。
        let started = {
            let mut w = session.waiting.lock().unwrap_or_else(|e| e.into_inner());
            match *w {
                Some((g, t)) if g == generation => t,
                _ => {
                    let t = Instant::now();
                    *w = Some((generation, t));
                    t
                }
            }
        };
        // 待機中に期限が来たら、OS を触らず TimedOut で完了させる。
        if started.elapsed() >= frame_deadline {
            *session.waiting.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = sender.send(Box::new(move |s: &mut NfcReadState| {
                mark_running(s);
                finish_read(s, generation, Err(CoreFailure::TimedOut));
            }));
            return;
        }
        // 前回のワーカーがリスナーを解除し終えるまで発送しない（同じ reader を二重に開かない）。
        if session.os_active.load(Ordering::Acquire) {
            sender.request_repaint_after(POLL_INTERVAL);
            return;
        }
        *session.waiting.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let request_id = slot.begin();
        *session.started.lock().unwrap_or_else(|e| e.into_inner()) = Some(started);
        let _ = sender.send(Box::new(mark_running));
        sender.request_repaint_after(POLL_INTERVAL);

        let worker_imp = imp.clone();
        session.os_active.store(true, Ordering::Release);
        let active = session.os_active.clone();
        let spawned = std::thread::Builder::new()
            .name("nfc".into())
            .spawn(move || {
                let result = run_worker(&*worker_imp, started, deadline);
                active.store(false, Ordering::Release);
                slot.deliver(request_id, result);
            });
        if spawned.is_err() {
            session.os_active.store(false, Ordering::Release);
            slot.deliver(request_id, Err(NfcReadFailure::Failed));
        }
        return;
    }
    if state.phase != Phase::Running {
        return;
    }
    let generation = state.generation;
    // 期限判定: 単調時計。確定後に届く結果は照合で捨てられる。
    let started = *session.started.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(started), Some((request_id, None))) = (started, slot.snapshot()) {
        // 判定と除去を原子的に行う。false は deliver が先に着地済み（成功を消さず後続に任せる）。
        if started.elapsed() >= frame_deadline && slot.clear_if_pending(request_id) {
            let _ = sender.send(Box::new(move |s: &mut NfcReadState| {
                finish_read(s, generation, Err(CoreFailure::TimedOut));
            }));
            return;
        }
    }
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut NfcReadState, outcome: Outcome| {
            apply_outcome(s, generation, outcome);
            true
        },
    );
}

/// `registry.rs` の `effect` 区画から `nfc_read` 画面用に配線される effect ドライバ。
pub fn nfc_read_effect(state: &NfcReadState, sender: &CompletionSender<NfcReadState>) {
    static IMPL: OnceLock<Arc<dyn NfcReader>> = OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        READ_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::nfc_read::{request_read, NfcReadState as State};
    use fandhe_device::nfc::UnsupportedNfcReader;
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<State>>;

    fn channel() -> (CompletionSender<State>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn text() -> Outcome {
        Ok(NfcReadOutcome::from_records(vec![NdefRecordView::Text {
            lang: "en".into(),
            text: "hi".into(),
        }]))
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Outcome,
        sleep: Duration,
    }
    impl NfcReader for Fake {
        fn read_one(&self, _: Duration) -> Outcome {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake_with(result: fn() -> Outcome, sleep: Duration) -> Arc<dyn NfcReader> {
        Arc::new(Fake { result, sleep })
    }

    fn fake(result: fn() -> Outcome) -> Arc<dyn NfcReader> {
        fake_with(result, Duration::ZERO)
    }

    fn session(grace: Duration) -> Session {
        Session {
            grace,
            ..Session::default()
        }
    }

    /// フレームを回して届いた完了を適用する。`Running` を抜けるか 5 秒で打ち切る。
    fn pump(
        state: &mut State,
        sender: &CompletionSender<State>,
        rx: &Rx,
        imp: &Arc<dyn NfcReader>,
        session: &Session,
        deadline: Duration,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, session, deadline);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if !matches!(state.phase, Phase::Requested | Phase::Running) || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn run(imp: Arc<dyn NfcReader>) -> State {
        let (sender, rx) = channel();
        let mut state = State::default();
        request_read(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &session(SETTLE_GRACE),
            Duration::from_secs(30),
        );
        state
    }

    fn failure(s: &State) -> CoreFailure {
        match &s.result {
            Some(Err(f)) => *f,
            r => panic!("失敗のはず: {r:?}"),
        }
    }

    #[test]
    fn success_and_empty_tag_reach_the_screen() {
        let s = run(fake(text));
        assert_eq!(
            s.result,
            Some(Ok(ShownTag::Records(vec![ShownRecord::Text {
                lang: "en".into(),
                text: "hi".into()
            }])))
        );
        let s = run(fake(|| Ok(NfcReadOutcome::EmptyTag)));
        assert_eq!(s.result, Some(Ok(ShownTag::Empty)));
    }

    #[test]
    fn failures_map_one_to_one() {
        use NfcReadFailure as F;
        let cases: [(fn() -> Outcome, CoreFailure); 8] = [
            (|| Err(F::Unreadable), CoreFailure::Unreadable),
            (|| Err(F::TooLarge), CoreFailure::TooLarge),
            (|| Err(F::Disabled), CoreFailure::Disabled),
            (|| Err(F::PermissionDenied), CoreFailure::PermissionDenied),
            (|| Err(F::TimedOut), CoreFailure::TimedOut),
            (|| Err(F::Interrupted), CoreFailure::Interrupted),
            (|| Err(F::Unsupported), CoreFailure::Unsupported),
            (|| Err(F::Failed), CoreFailure::Failed),
        ];
        for (f, expect) in cases {
            assert_eq!(failure(&run(fake(f))), expect);
        }
    }

    #[test]
    fn late_success_is_timeout_but_late_failure_passes_through() {
        for (result, expect) in [
            (text as fn() -> Outcome, CoreFailure::TimedOut),
            (|| Err(NfcReadFailure::Disabled), CoreFailure::Disabled),
        ] {
            let imp = fake_with(result, Duration::from_millis(60));
            let (sender, rx) = channel();
            let mut state = State::default();
            request_read(&mut state);
            pump(
                &mut state,
                &sender,
                &rx,
                &imp,
                &session(Duration::from_secs(5)),
                Duration::from_millis(20),
            );
            assert_eq!(failure(&state), expect);
        }
    }

    #[test]
    fn safety_net_times_out_and_late_worker_result_is_ignored() {
        let imp = fake_with(text, Duration::from_millis(300));
        let (sender, rx) = channel();
        let session = session(Duration::from_millis(10));
        let mut state = State::default();
        request_read(&mut state);
        let deadline = Duration::from_millis(20);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failure(&state), CoreFailure::TimedOut);
        std::thread::sleep(Duration::from_millis(350));
        drive(&state, &sender, &imp, &session, deadline);
        assert!(rx.try_recv().is_err());
        assert_eq!(failure(&state), CoreFailure::TimedOut);
    }

    #[test]
    fn no_new_dispatch_while_previous_worker_is_active() {
        let imp = fake_with(text, Duration::from_millis(200));
        let (sender, rx) = channel();
        let session = session(Duration::from_millis(10));
        let mut state = State::default();
        request_read(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &session,
            Duration::from_millis(20),
        );
        assert_eq!(failure(&state), CoreFailure::TimedOut);
        // ワーカーはまだ走っている: 再押下しても発送されない。
        request_read(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err());
        std::thread::sleep(Duration::from_millis(250));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let mut state = State::default();
        request_read(&mut state);
        mark_running(&mut state);
        apply_outcome(&mut state, 99, text());
        assert_eq!(
            (state.phase, state.result.is_none()),
            (Phase::Running, true)
        );
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(text);
        let session = session(SETTLE_GRACE);
        let mut state = State::default();
        request_read(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).count(), 1);
    }

    #[test]
    fn unsupported_is_never_success() {
        assert_eq!(
            failure(&run(Arc::new(UnsupportedNfcReader))),
            CoreFailure::Unsupported
        );
        #[cfg(not(target_os = "android"))]
        assert_eq!(
            platform().read_one(Duration::ZERO),
            Err(NfcReadFailure::Unsupported)
        );
    }
}
