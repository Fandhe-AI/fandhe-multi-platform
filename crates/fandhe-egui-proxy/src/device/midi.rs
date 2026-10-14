//! Phase 11 の拡充ループ（親 #1137）型サンプル「MIDI 入力」（Issue #1398）の effect ドライバ。
//! `fandhe_core::screens::midi_show`（端末依存ゼロ）の要求を `take_command` で観測し、
//! `fandhe_device::midi::MidiInput` の購読を始め / 止め、届いた通知を `CompletionSender` 経由で
//! 注入する（`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::midi`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_midi`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS / iOS: `apple_midi`（iOS 14 以上。未満は失敗コードを返す）、
//! Android: `android_midi`（#1400。再生成時は `reset_pending_tracking` を `android_main` が呼ぶ）、Windows: `windows_midi`（#1402））。それ以外は
//! `UnsupportedMidiInput` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 並行性
//!
//! 実装は購読を 1 つだけ持つ（プロセスに 1 つの `MidiInput`）。ドライバは世代つきの sink を作って
//! 渡し、通知は sink → `CompletionSender` の 1 経路だけで画面へ届く。未適用の通知が
//! [`MAX_PENDING_BATCHES`] に達したら、追加のノートは送らず数えて捨てる（`dropped`、画面に出す）。
//! 開始の期限は単調時計（`Instant`）で測り、`Starting` の間だけ `request_repaint_after` で判定を
//! 予約する。期限切れ後は `stop()` を呼んで遅れて確定した購読を残さない。

use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::midi_show::{
    apply_update, expire_start, mark_command_taken, take_command, MidiCommand, MidiFailure,
    MidiNoteRow, MidiPhase, MidiShowState, MidiUpdate,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::midi::UnsupportedMidiInput;
use fandhe_device::midi::{
    MidiEvent, MidiInput, MidiSink, NoteEvent, MAX_PENDING_BATCHES, START_DEADLINE,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// プロセスに 1 つだけ持つ実装（`stop` が `start` と同じ実体に届くようにする）。
fn platform_midi() -> &'static Arc<dyn MidiInput> {
    static INPUT: OnceLock<Arc<dyn MidiInput>> = OnceLock::new();
    INPUT.get_or_init(|| {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            super::apple_midi::implementation()
        }
        #[cfg(target_os = "windows")]
        {
            super::windows_midi::implementation()
        }
        #[cfg(target_os = "android")]
        {
            super::android_midi::implementation()
        }
        #[cfg(not(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        )))]
        {
            Arc::new(UnsupportedMidiInput)
        }
    })
}

/// ドライバの可変状態。テストが並列で干渉しないよう static から切り出して引数で渡す。
#[derive(Default)]
struct Session {
    inner: Mutex<SessionInner>,
}

#[derive(Default)]
struct SessionInner {
    /// 開始待ちの世代・開始時刻（期限判定用）・開始完了と期限切れの排他確定フラグ。
    starting: Option<(u64, Instant, Arc<AtomicU8>)>,
    /// 処理済みの要求世代（`mark_command_taken` の反映前に二重処理しない）。
    handled: u64,
    /// 現在の sink の世代と取りこぼしカウンタ（「消去」の基準を同じカウンタから読む）。
    dropped: Option<(u64, Arc<AtomicU64>)>,
    /// 基準を返し済みの `clear_epoch`。
    cleared_epoch: u64,
}

/// 開始の結果が未確定。
const SETTLE_PENDING: u8 = 0;
/// `Started` が先に確定した（以後、期限切れにしない）。
const SETTLE_STARTED: u8 = 1;
/// 期限切れが先に確定した（以後、遅れて届いた `Started` を捨てる）。
const SETTLE_EXPIRED: u8 = 2;

static SESSION: OnceLock<Session> = OnceLock::new();

impl Session {
    /// 画面状態（generation 0 から再開）と食い違う記録を捨てる。作り直し前の `handled` が
    /// 作り直し後の同じ世代の「受信開始」を処理済みとして飲み込むのを防ぐ。poison は無視して進める。
    fn reset(&self) {
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = SessionInner::default();
    }
}

/// Activity 再生成時（`android_main` の再実行）に、保留中の開始とドライバの記録を破棄する
/// （Issue #1400）。画面状態 `MidiShowState` は再生成で `default()` に戻るため、ドライバ側の
/// 処理済み世代と購読（Rust 側 epoch・sink）も揃えて捨てる。Java 側は旧インスタンスの
/// `onDestroy` が close 済みで、epoch が進むため旧通知は新しい購読に混ざらない。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.reset();
    }
    platform_midi().stop();
}

fn to_failure(e: &DeviceError) -> MidiFailure {
    match e {
        DeviceError::Unsupported(_) => MidiFailure::Unsupported,
        DeviceError::PermissionDenied(_) => MidiFailure::PermissionDenied,
        // OS の生コードは画面へ出さない（security.md A09）。
        _ => MidiFailure::Failed("MIDI の開始".to_string()),
    }
}

fn send_update(sender: &CompletionSender<MidiShowState>, generation: u64, update: MidiUpdate) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut MidiShowState| {
        apply_update(s, generation, update);
    }));
}

/// 世代つきの sink を作る。`Notes` だけ保留数と上限を管理し、他の通知は捨てない。
fn make_sink(
    sender: CompletionSender<MidiShowState>,
    generation: u64,
    settle: Arc<AtomicU8>,
    started: Instant,
    deadline: Duration,
    dropped: Arc<AtomicU64>,
) -> MidiSink {
    let pending = Arc::new(AtomicUsize::new(0));
    Arc::new(move |ev: MidiEvent| -> bool {
        match ev {
            MidiEvent::Notes(notes) => {
                if notes.is_empty() {
                    return true;
                }
                if pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING_BATCHES {
                    pending.fetch_sub(1, Ordering::SeqCst);
                    dropped.fetch_add(notes.len() as u64, Ordering::SeqCst);
                    return true;
                }
                let rows: Vec<MidiNoteRow> = notes.iter().map(to_row).collect();
                let (p, d) = (pending.clone(), dropped.clone());
                let sent = sender.send(Box::new(move |s: &mut MidiShowState| {
                    p.fetch_sub(1, Ordering::SeqCst);
                    apply_update(
                        s,
                        generation,
                        MidiUpdate::Notes(rows, d.load(Ordering::SeqCst)),
                    );
                }));
                if sent.is_err() {
                    pending.fetch_sub(1, Ordering::SeqCst);
                    return false;
                }
                true
            }
            MidiEvent::Started { sources, failed } => {
                // 期限切れが先に確定していたら、購読は停止済みなので Listening にしない。
                // 期限（単調時計）を過ぎてから届いた Started は成功にせず PENDING のまま残し、
                // 次の drive の期限切れ処理に確定させる。
                if started.elapsed() < deadline
                    && settle
                        .compare_exchange(
                            SETTLE_PENDING,
                            SETTLE_STARTED,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .is_ok()
                {
                    send_update(&sender, generation, MidiUpdate::Started(sources, failed));
                }
                true
            }
            MidiEvent::Sources { sources, failed } => {
                send_update(&sender, generation, MidiUpdate::Sources(sources, failed));
                true
            }
            MidiEvent::Failed(e) => {
                send_update(&sender, generation, MidiUpdate::Failed(to_failure(&e)));
                true
            }
        }
    })
}

fn to_row(n: &NoteEvent) -> MidiNoteRow {
    MidiNoteRow {
        on: n.on,
        channel: n.channel,
        note: n.note,
        velocity: n.velocity,
    }
}

/// 1 フレームぶんの処理。要求の発送、開始の期限判定を行う。
fn drive(
    state: &MidiShowState,
    sender: &CompletionSender<MidiShowState>,
    input: &Arc<dyn MidiInput>,
    session: &Session,
    deadline: Duration,
) {
    let Ok(mut inner) = session.inner.lock() else {
        return;
    };
    if let Some((command, generation)) = take_command(state) {
        if inner.handled != generation {
            inner.handled = generation;
            let _ = sender.send(Box::new(move |s: &mut MidiShowState| {
                mark_command_taken(s, generation)
            }));
            match command {
                MidiCommand::Start => {
                    let settle = Arc::new(AtomicU8::new(SETTLE_PENDING));
                    let now = Instant::now();
                    inner.starting = Some((generation, now, settle.clone()));
                    let dropped = Arc::new(AtomicU64::new(0));
                    inner.dropped = Some((generation, dropped.clone()));
                    let sink =
                        make_sink(sender.clone(), generation, settle, now, deadline, dropped);
                    if let Err(e) = input.start(sink) {
                        inner.starting = None;
                        send_update(sender, generation, MidiUpdate::Failed(to_failure(&e)));
                    }
                }
                MidiCommand::Stop => {
                    inner.starting = None;
                    input.stop();
                }
            }
        }
    }

    // 方式: 消去をドライバの集計と順序付ける。「消去」を見た時点の最新カウンタを基準として
    // 完了通知で返す（通知は届いた順に適用され、カウンタは単調増加）。画面側が最後に適用した値を
    // 基準にすると、受信スレッドで先に増えた超過分を消去後に再表示してしまう。
    // 通知に集計世代を持たせる案より、カウンタを触る場所が drive の 1 か所で済む。
    // 取りこぼしの読み取りと drive の間（同一フレーム内）の増分は消去前として扱う。
    if state.clear_epoch != inner.cleared_epoch {
        inner.cleared_epoch = state.clear_epoch;
        if let Some((g, d)) = inner.dropped.clone() {
            if g == state.generation {
                send_update(sender, g, MidiUpdate::DroppedBase(d.load(Ordering::SeqCst)));
            }
        }
    }

    let Some((generation, started, settle)) = inner.starting.clone() else {
        return;
    };
    if state.phase != MidiPhase::Starting || state.generation != generation {
        // 確定済み、または停止・差し替え済み。
        inner.starting = None;
        return;
    }
    let elapsed = started.elapsed();
    if elapsed >= deadline {
        inner.starting = None;
        // Started と排他で確定する。Started が先ならそれが勝ち（購読は生きていて画面も追従する）。
        if settle
            .compare_exchange(
                SETTLE_PENDING,
                SETTLE_EXPIRED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            let _ = sender.send(Box::new(move |s: &mut MidiShowState| {
                expire_start(s, generation)
            }));
            // 遅れて確定した購読を残さない。
            input.stop();
        }
    } else {
        sender.request_repaint_after(deadline - elapsed);
    }
}

/// `registry.rs` の `effect` 区画から `midi_show` 画面用に配線される effect ドライバ。
pub fn midi_show_effect(state: &MidiShowState, sender: &CompletionSender<MidiShowState>) {
    drive(
        state,
        sender,
        platform_midi(),
        SESSION.get_or_init(Session::default),
        START_DEADLINE,
    );
}

/// アプリ終了時に購読を外す（`device::shutdown` から呼ばれる）。
pub(super) fn shutdown() {
    platform_midi().stop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::midi_show::{start, stop, START_TIMEOUT};
    use std::sync::mpsc;

    struct Fake {
        error: Mutex<Option<DeviceError>>,
        auto_started: Option<usize>,
        sink: Mutex<Option<MidiSink>>,
        stops: AtomicUsize,
    }

    impl Fake {
        fn new(error: Option<DeviceError>, auto_started: Option<usize>) -> Arc<Self> {
            Arc::new(Self {
                error: Mutex::new(error),
                auto_started,
                sink: Mutex::new(None),
                stops: AtomicUsize::new(0),
            })
        }
        fn sink(&self) -> MidiSink {
            self.sink.lock().unwrap().clone().expect("sink")
        }
    }

    impl MidiInput for Fake {
        fn start(&self, sink: MidiSink) -> Result<(), DeviceError> {
            if let Some(e) = self.error.lock().unwrap().take() {
                return Err(e);
            }
            *self.sink.lock().unwrap() = Some(sink.clone());
            if let Some(n) = self.auto_started {
                sink(MidiEvent::Started {
                    sources: n,
                    failed: 0,
                });
            }
            Ok(())
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    type Rx = mpsc::Receiver<crate::app::Completion<MidiShowState>>;

    fn channel() -> (CompletionSender<MidiShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn pump(state: &mut MidiShowState, rx: &Rx) {
        while let Ok(c) = rx.try_recv() {
            c(state);
        }
    }

    fn note(n: u8) -> MidiEvent {
        MidiEvent::Notes(vec![NoteEvent::from_wire(0x9, 0, n, 100).unwrap()])
    }

    fn run(fake: &Arc<Fake>) -> (MidiShowState, CompletionSender<MidiShowState>, Rx, Session) {
        let (sender, rx) = channel();
        let mut state = MidiShowState::default();
        let session = Session::default();
        start(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        pump(&mut state, &rx);
        (state, sender, rx, session)
    }

    #[test]
    fn sync_started_becomes_listening_and_command_is_consumed() {
        let fake = Fake::new(None, Some(2));
        let (state, ..) = run(&fake);
        assert_eq!(
            state.phase,
            MidiPhase::Listening {
                sources: 2,
                failed: 0
            }
        );
        assert_eq!(take_command(&state), None);
    }

    #[test]
    fn errors_are_never_success() {
        for (e, expect_unsupported) in [
            (DeviceError::Unsupported("midi"), true),
            (DeviceError::Internal("midi start"), false),
        ] {
            let fake = Fake::new(Some(e), None);
            let (state, ..) = run(&fake);
            match state.phase {
                MidiPhase::Failed(MidiFailure::Unsupported) => assert!(expect_unsupported),
                MidiPhase::Failed(MidiFailure::Failed(_)) => assert!(!expect_unsupported),
                p => panic!("{p:?}"),
            }
        }
    }

    #[test]
    fn reset_lets_recreated_screen_start_again() {
        let fake = Fake::new(None, Some(0));
        let (sender, _rx) = channel();
        let session = Session::default();
        let input: Arc<dyn MidiInput> = fake.clone();
        let mut state = MidiShowState::default();
        start(&mut state);
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        // Activity 再生成: 画面状態だけ generation 0 に戻る。
        let mut state = MidiShowState::default();
        start(&mut state);
        *fake.sink.lock().unwrap() = None;
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        assert!(
            fake.sink.lock().unwrap().is_none(),
            "リセット無しでは飲み込まれる"
        );
        session.reset();
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        assert!(
            fake.sink.lock().unwrap().is_some(),
            "リセット後は処理される"
        );
    }

    #[test]
    fn double_drive_before_mark_applied_starts_once() {
        let fake = Fake::new(None, Some(0));
        let (sender, rx) = channel();
        let mut state = MidiShowState::default();
        let session = Session::default();
        start(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        // mark が未適用のまま再実行
        *fake.sink.lock().unwrap() = None;
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        assert!(fake.sink.lock().unwrap().is_none(), "二重に start しない");
        pump(&mut state, &rx);
    }

    #[test]
    fn late_notes_from_old_sink_after_stop_are_dropped() {
        let fake = Fake::new(None, Some(1));
        let (mut state, sender, rx, session) = run(&fake);
        let old = fake.sink();
        stop(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        assert_eq!(fake.stops.load(Ordering::SeqCst), 1);
        old(note(60));
        pump(&mut state, &rx);
        assert!(state.events.is_empty());
        assert_eq!(state.phase, MidiPhase::Idle);
    }

    #[test]
    fn pending_is_capped_and_overflow_is_counted() {
        let fake = Fake::new(None, Some(1));
        let (mut state, _sender, rx, _s) = run(&fake);
        let sink = fake.sink();
        for i in 0..300u32 {
            assert!(sink(note((i % 128) as u8)));
        }
        let mut applied = 0;
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
            applied += 1;
        }
        assert_eq!(applied, MAX_PENDING_BATCHES);
        assert_eq!(state.total, MAX_PENDING_BATCHES as u64);
        assert_eq!(state.dropped, 300 - MAX_PENDING_BATCHES as u64);
        // 適用後は再び受けられる
        assert!(sink(note(1)));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn clear_hides_overflow_accumulated_before_clear() {
        use fandhe_core::screens::midi_show::clear;
        let fake = Fake::new(None, Some(1));
        let (mut state, sender, rx, session) = run(&fake);
        let sink = fake.sink();
        assert!(sink(note(1)));
        pump(&mut state, &rx);
        // 画面が知らない間に上限超過で 100 件増える。
        for _ in 0..(MAX_PENDING_BATCHES + 100) {
            sink(note(2));
        }
        pump(&mut state, &rx);
        let before = state.dropped;
        assert!(before >= 100);
        clear(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        drive(&state, &sender, &input, &session, Duration::from_secs(5));
        pump(&mut state, &rx);
        assert_eq!(state.dropped, 0);
        // 消去後の通知に消去前の超過分は載らない。取りこぼしは数える。
        assert!(sink(note(3)));
        pump(&mut state, &rx);
        assert_eq!(state.dropped, 0);
        for _ in 0..(MAX_PENDING_BATCHES + 3) {
            sink(note(4));
        }
        pump(&mut state, &rx);
        assert_eq!(state.dropped, 3);
    }

    #[test]
    fn sink_returns_false_when_receiver_is_gone() {
        let fake = Fake::new(None, Some(1));
        let (_state, _sender, rx, _s) = run(&fake);
        let sink = fake.sink();
        drop(rx);
        assert!(!sink(note(60)));
    }

    #[test]
    fn start_deadline_fails_and_stops_input() {
        let fake = Fake::new(None, None); // Started が来ない
        let (sender, rx) = channel();
        let mut state = MidiShowState::default();
        let session = Session::default();
        start(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        let d = Duration::from_millis(20);
        drive(&state, &sender, &input, &session, d);
        pump(&mut state, &rx);
        assert_eq!(state.phase, MidiPhase::Starting);
        std::thread::sleep(Duration::from_millis(40));
        drive(&state, &sender, &input, &session, d);
        pump(&mut state, &rx);
        assert!(matches!(
            &state.phase,
            MidiPhase::Failed(MidiFailure::Failed(m)) if m == START_TIMEOUT
        ));
        assert_eq!(fake.stops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn started_after_expiry_is_dropped() {
        let fake = Fake::new(None, None);
        let (sender, rx) = channel();
        let mut state = MidiShowState::default();
        let session = Session::default();
        start(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        let d = Duration::from_millis(20);
        drive(&state, &sender, &input, &session, d);
        pump(&mut state, &rx);
        std::thread::sleep(Duration::from_millis(40));
        drive(&state, &sender, &input, &session, d);
        // 期限切れ確定の後に購読側が Started を返しても、Listening にしない。
        fake.sink()(MidiEvent::Started {
            sources: 1,
            failed: 0,
        });
        pump(&mut state, &rx);
        assert!(matches!(state.phase, MidiPhase::Failed(_)));
    }

    #[test]
    fn late_started_before_next_drive_is_expired() {
        let fake = Fake::new(None, None);
        let (sender, rx) = channel();
        let mut state = MidiShowState::default();
        let session = Session::default();
        start(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        let d = Duration::from_millis(20);
        drive(&state, &sender, &input, &session, d);
        std::thread::sleep(Duration::from_millis(40));
        // 期限超過後・次の drive 前に届いた Started は成功にならない。
        fake.sink()(MidiEvent::Started {
            sources: 1,
            failed: 0,
        });
        drive(&state, &sender, &input, &session, d);
        pump(&mut state, &rx);
        assert!(matches!(state.phase, MidiPhase::Failed(_)));
        assert_eq!(fake.stops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn started_before_expiry_wins_and_is_not_stopped() {
        let fake = Fake::new(None, None);
        let (sender, rx) = channel();
        let mut state = MidiShowState::default();
        let session = Session::default();
        start(&mut state);
        let input: Arc<dyn MidiInput> = fake.clone();
        let d = Duration::from_millis(20);
        drive(&state, &sender, &input, &session, d);
        fake.sink()(MidiEvent::Started {
            sources: 1,
            failed: 0,
        });
        std::thread::sleep(Duration::from_millis(40));
        // Started は未適用（state は Starting のまま）で期限を超えた状況。
        drive(&state, &sender, &input, &session, d);
        pump(&mut state, &rx);
        assert_eq!(
            state.phase,
            MidiPhase::Listening {
                sources: 1,
                failed: 0
            }
        );
        assert_eq!(fake.stops.load(Ordering::SeqCst), 0);
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        let sink: MidiSink = Arc::new(|_| true);
        assert!(matches!(
            platform_midi().start(sink),
            Err(DeviceError::Unsupported("midi"))
        ));
    }
}
