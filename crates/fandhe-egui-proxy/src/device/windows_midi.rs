//! Windows 向け MIDI 入力実装（Issue #1402、親 #1137）。`device/midi.rs::platform_midi` の
//! Windows 経路から呼ばれ、WinRT `Windows.Devices.Midi.MidiInPort` が型付きで渡すノートオン / オフを
//! `fandhe_device::midi::NoteEvent::from_wire` で検証して sink へ流す。
//!
//! # 方式: 常駐ワーカー 1 本 + 直列キュー + 世代（epoch）照合
//!
//! `start` / `stop` / 抜き差し / 列挙完了はすべて `mpsc` のキューへ流し、プロセス寿命の専用ワーカー
//! 1 本が順に処理する（契約表 11）。開始ごとにワーカーを立てないので、「停止」直後の「受信開始」が
//! 後片付け中のワーカーを理由に偽の失敗にならない。デバイスの列挙は `DeviceWatcher`
//! （初期列挙の `Added` → `EnumerationCompleted`、以後は抜き差し）で受け、`MessageReceived` は
//! WinRT のスレッドから直接 sink へ流す。epoch は `start` / `stop` のたびに進み、
//! ワーカー側（`Cmd`）と `Router::emit` の両方で照合して古い通知を捨てる。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 0 台は成功（`Started{0, 0}`）。1 台以上見つかったのに全部開けないときだけ
//!   `Failed(Internal("midi open"))`。一部だけ開けないときは `failed` で数える
//! - WinRT クラス未登録（`REGDB_E_CLASSNOTREG`）は `Unsupported("midi")`。それ以外の起動失敗は
//!   `Internal("midi start")`
//! - 利用者の停止は失敗にしない（`stop` は冪等）。OS 起点の watcher 停止（`Stopped`）は
//!   `Failed(Internal("midi watcher"))` を通知して片付ける（解除後の停止は届かない）
//!
//! # 並行性（契約表 2・11）
//!
//! ロックは `ROUTER` 1 本だけ。sink はそのロックの内側で呼ぶ（`stop` が戻った後に sink が走らない
//! 保証。sink は mpsc 送信と atomic だけでブロックしない）。ハンドラ内では `.get()` しない。
//!
//! 守る範囲: 1 プロセス内の順序、失敗を成功へ畳まないこと、`stop` 後に sink を呼ばないこと、
//! 全経路でのハンドラ解除と `Close`。演奏内容・デバイス ID・HRESULT はログへ出さない（A09）。
//! 守らない範囲: 他アプリがポートを排他で開いている場合（開けない入力として `failed` に数える）、
//! 複数ポート間の厳密な到着順、抜き差し確定との時間差による件数表示の一時的な食い違い。
//! `FromIdAsync` はワーカー内で待たない。`Completed` ハンドラが `Cmd::Opened`（epoch・id 付き）を
//! キューへ戻し、待機中も `Stop` / 抜き差しを処理できる。取り消し・世代不一致・抜去済み・古い要求で遅れて
//! 開いたポートは `Opened` の処理で必ず `Close` する。`Started` は保留中の open が 0 件になってから流す。
//!
//! 実機確認は未実施（#1397）。確認項目: unpackaged での動作、running status を使う機器で連続ノートが
//! 欠けないか、`FromIdAsync` の null 条件、他アプリがポートを開いているときの挙動。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fandhe_device::midi::{MidiEvent, MidiInput, MidiSink};
use fandhe_device::DeviceError;

/// epoch と sink を 1 本のロックで守る純ロジック（host でテストする）。
#[derive(Default)]
struct Router {
    epoch: u64,
    sink: Option<MidiSink>,
}

impl Router {
    /// 新しい購読を始める。epoch を進めて旧通知を無効にし、新しい epoch を返す。
    fn begin(&mut self, sink: MidiSink) -> u64 {
        self.epoch += 1;
        self.sink = Some(sink);
        self.epoch
    }

    /// `begin` と、その epoch のコマンド投入（`enqueue`）を同じロックの内側で行う。
    /// 投入順が epoch 順と一致する。投入に失敗したら購読を外して false を返す。
    fn begin_enqueue(&mut self, sink: MidiSink, enqueue: impl FnOnce(u64) -> bool) -> bool {
        let e = self.begin(sink);
        if enqueue(e) {
            true
        } else {
            self.end();
            false
        }
    }

    /// `end` と、その epoch の `Stop` 投入を同じロックの内側で行う（`begin_enqueue` と順序が揃う）。
    fn end_enqueue(&mut self, enqueue: impl FnOnce(u64)) {
        self.end();
        enqueue(self.epoch);
    }

    /// 購読を外す。以後の通知は epoch 不一致で捨てる。
    fn end(&mut self) {
        self.epoch += 1;
        self.sink = None;
    }

    /// 現在の epoch の sink へ通知を渡す。false を返されたら受け手なしとして外す。
    fn emit(&mut self, epoch: u64, ev: MidiEvent) {
        if epoch != self.epoch {
            return;
        }
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        if !sink(ev) {
            self.sink = None;
        }
    }

    /// `epoch` が現在の購読か（古い `Start` コマンドを処理前に捨てるため）。
    fn is_current(&self, epoch: u64) -> bool {
        epoch == self.epoch && self.sink.is_some()
    }
}

static ROUTER: Mutex<Router> = Mutex::new(Router {
    epoch: 0,
    sink: None,
});

/// ロックの poison は無視して進める（sink 内の panic で以後の停止が効かなくなるのを避ける）。
fn with_router<R>(f: impl FnOnce(&mut Router) -> R) -> R {
    f(&mut ROUTER.lock().unwrap_or_else(|e| e.into_inner()))
}

/// 列挙完了時の開始結果。1 台以上見つかって全部開けないときだけ失敗（0 台は成功）。
fn start_event(opened: usize, failed: usize) -> MidiEvent {
    if opened == 0 && failed > 0 {
        MidiEvent::Failed(DeviceError::Internal("midi open"))
    } else {
        MidiEvent::Started {
            sources: opened,
            failed,
        }
    }
}

/// 見つかった入力の台帳。`None` は「見つかったが開けなかった」入力。
struct Inputs<P> {
    map: HashMap<String, Option<P>>,
}

impl<P> Inputs<P> {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// 同じ id の二重 `Added` は無視する（false）。
    fn insert(&mut self, id: String, port: Option<P>) -> bool {
        if self.map.contains_key(&id) {
            return false;
        }
        self.map.insert(id, port);
        true
    }

    /// 抜かれた入力を台帳から外し、閉じるべきポートを返す。
    fn remove(&mut self, id: &str) -> Option<P> {
        self.map.remove(id).flatten()
    }

    /// (開けた数, 開けなかった数)。
    fn counts(&self) -> (usize, usize) {
        let opened = self.map.values().filter(|p| p.is_some()).count();
        (opened, self.map.len() - opened)
    }

    /// 後片付け用に全ポートを取り出す。
    fn drain(&mut self) -> Vec<P> {
        self.map.drain().filter_map(|(_, p)| p).collect()
    }
}

/// 現在の実装。
pub(crate) fn implementation() -> Arc<dyn MidiInput> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(win::WindowsMidi)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(fandhe_device::midi::UnsupportedMidiInput)
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::sync::{mpsc, OnceLock};

    use windows::core::{Interface, Ref, HSTRING};
    use windows::Devices::Enumeration::{
        DeviceInformation, DeviceInformationUpdate, DeviceWatcher,
    };
    use windows::Devices::Midi::{
        MidiInPort, MidiMessageReceivedEventArgs, MidiMessageType, MidiNoteOffMessage,
        MidiNoteOnMessage,
    };
    use windows::Foundation::TypedEventHandler;

    use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

    use fandhe_device::midi::NoteEvent;

    use super::*;
    use crate::device::windows_notification::ComInit;

    /// `REGDB_E_CLASSNOTREG`（WinRT クラスが登録されていない環境）。
    const CLASS_NOT_REG: i32 = 0x8004_0154_u32 as i32;

    enum Cmd {
        Start(u64),
        /// `end` 後の epoch。これより新しい世代のセッションは壊さない。
        Stop(u64),
        Added(u64, HSTRING),
        Removed(u64, HSTRING),
        Enumerated(u64),
        /// OS 起点の watcher 停止（Aborted 等）。利用者の Stop は解除後に停止するので届かない。
        Stopped(u64),
        /// `FromIdAsync` の完了。`None` は開けなかった（エラー・キャンセル・null）。
        Opened(u64, HSTRING, u64, Option<MidiInPort>),
    }

    type Port = (MidiInPort, i64);

    struct Session {
        epoch: u64,
        watcher: DeviceWatcher,
        added: i64,
        removed: i64,
        enumerated_token: i64,
        stopped: i64,
        inputs: Inputs<Port>,
        enumerated: bool,
        /// `Started` を流し終えたか（以後の変化は `Sources`）。
        started: bool,
        /// `FromIdAsync` の完了待ちの id → open 要求番号。同じ id の抜き差しで古い完了が
        /// 新しい要求を横取りしないよう、番号が一致した完了だけを適用する。
        pending: HashMap<String, u64>,
        /// 次に発行する open 要求番号。
        next_req: u64,
    }

    /// `None` はワーカーを立てられなかったことの記録（以後の `start` は失敗）。
    static QUEUE: OnceLock<Option<mpsc::Sender<Cmd>>> = OnceLock::new();

    fn queue() -> Option<&'static mpsc::Sender<Cmd>> {
        QUEUE
            .get_or_init(|| {
                let (tx, rx) = mpsc::channel();
                let tx2 = tx.clone();
                std::thread::Builder::new()
                    .name("fandhe-midi".into())
                    .spawn(move || worker(rx, tx2))
                    .ok()
                    .map(|_| tx)
            })
            .as_ref()
    }

    pub(super) struct WindowsMidi;

    impl MidiInput for WindowsMidi {
        fn start(&self, sink: MidiSink) -> Result<(), DeviceError> {
            // 世代更新と投入は同じロックで行い、投入順を世代順に揃える（mpsc 送信はブロックしない）。
            let ok = with_router(|r| {
                r.begin_enqueue(sink, |e| {
                    queue().is_some_and(|q| q.send(Cmd::Start(e)).is_ok())
                })
            });
            if ok {
                Ok(())
            } else {
                Err(DeviceError::Internal("midi start"))
            }
        }

        fn stop(&self) {
            // sink を先に外す（以後 sink は呼ばれない）。後片付けはワーカーが行う。
            with_router(|r| {
                r.end_enqueue(|e| {
                    if let Some(q) = queue() {
                        let _ = q.send(Cmd::Stop(e));
                    }
                })
            });
        }
    }

    fn map_start_error(e: &windows::core::Error) -> DeviceError {
        if e.code().0 == CLASS_NOT_REG {
            DeviceError::Unsupported("midi")
        } else {
            DeviceError::Internal("midi start")
        }
    }

    /// 全ポートの解除と `Close`、watcher のハンドラ解除と停止（開始途中の失敗経路でも使う）。
    fn teardown(mut s: Session) {
        for (port, token) in s.inputs.drain() {
            let _ = port.RemoveMessageReceived(token);
            let _ = port.Close();
        }
        let _ = s.watcher.RemoveAdded(s.added);
        let _ = s.watcher.RemoveRemoved(s.removed);
        let _ = s.watcher.RemoveEnumerationCompleted(s.enumerated_token);
        // 自分の Stop で Stopped が届いて失敗扱いにならないよう、先に解除する。
        let _ = s.watcher.RemoveStopped(s.stopped);
        // Started / EnumerationCompleted 以外では失敗するので結果は無視する。
        let _ = s.watcher.Stop();
    }

    fn open_session(epoch: u64, tx: &mpsc::Sender<Cmd>) -> Result<Session, DeviceError> {
        let err = |e: windows::core::Error| map_start_error(&e);
        let selector = MidiInPort::GetDeviceSelector().map_err(err)?;
        let watcher = DeviceInformation::CreateWatcherAqsFilter(&selector).map_err(err)?;

        let txa = tx.clone();
        let added = watcher
            .Added(&TypedEventHandler::new(
                move |_: Ref<'_, DeviceWatcher>, info: Ref<'_, DeviceInformation>| {
                    let id = info.ok()?.Id()?;
                    let _ = txa.send(Cmd::Added(epoch, id));
                    Ok(())
                },
            ))
            .map_err(err)?;
        let txr = tx.clone();
        let removed = match watcher.Removed(&TypedEventHandler::new(
            move |_: Ref<'_, DeviceWatcher>, upd: Ref<'_, DeviceInformationUpdate>| {
                let id = upd.ok()?.Id()?;
                let _ = txr.send(Cmd::Removed(epoch, id));
                Ok(())
            },
        )) {
            Ok(t) => t,
            Err(e) => {
                let _ = watcher.RemoveAdded(added);
                return Err(map_start_error(&e));
            }
        };
        let txe = tx.clone();
        let enumerated_token = match watcher.EnumerationCompleted(&TypedEventHandler::new(
            move |_: Ref<'_, DeviceWatcher>, _: Ref<'_, windows::core::IInspectable>| {
                let _ = txe.send(Cmd::Enumerated(epoch));
                Ok(())
            },
        )) {
            Ok(t) => t,
            Err(e) => {
                let _ = watcher.RemoveAdded(added);
                let _ = watcher.RemoveRemoved(removed);
                return Err(map_start_error(&e));
            }
        };
        let txs = tx.clone();
        let stopped = match watcher.Stopped(&TypedEventHandler::new(
            move |_: Ref<'_, DeviceWatcher>, _: Ref<'_, windows::core::IInspectable>| {
                let _ = txs.send(Cmd::Stopped(epoch));
                Ok(())
            },
        )) {
            Ok(t) => t,
            Err(e) => {
                let _ = watcher.RemoveAdded(added);
                let _ = watcher.RemoveRemoved(removed);
                let _ = watcher.RemoveEnumerationCompleted(enumerated_token);
                return Err(map_start_error(&e));
            }
        };
        let session = Session {
            epoch,
            watcher,
            added,
            removed,
            enumerated_token,
            stopped,
            inputs: Inputs::new(),
            enumerated: false,
            started: false,
            pending: HashMap::new(),
            next_req: 0,
        };
        if let Err(e) = session.watcher.Start() {
            teardown(session);
            return Err(map_start_error(&e));
        }
        Ok(session)
    }

    /// 開いたポートへ `MessageReceived` を登録する。ノートだけを 1 回のコールバック 1 件で流す
    /// （worker から呼ぶ。登録に失敗したら `Close` して `None`）。
    fn register_port(epoch: u64, port: MidiInPort) -> Option<Port> {
        let token = port.MessageReceived(&TypedEventHandler::new(
            move |_: Ref<'_, MidiInPort>, args: Ref<'_, MidiMessageReceivedEventArgs>| {
                let msg = args.ok()?.Message()?;
                let note = match msg.Type()? {
                    MidiMessageType::NoteOn => {
                        let m = msg.cast::<MidiNoteOnMessage>()?;
                        NoteEvent::from_wire(0x9, m.Channel()?, m.Note()?, m.Velocity()?)
                    }
                    MidiMessageType::NoteOff => {
                        let m = msg.cast::<MidiNoteOffMessage>()?;
                        NoteEvent::from_wire(0x8, m.Channel()?, m.Note()?, m.Velocity()?)
                    }
                    _ => None,
                };
                if let Some(n) = note {
                    with_router(|r| r.emit(epoch, MidiEvent::Notes(vec![n])));
                }
                Ok(())
            },
        ));
        match token {
            Ok(t) => Some((port, t)),
            Err(_) => {
                let _ = port.Close();
                None
            }
        }
    }

    /// `FromIdAsync` を発行し、完了を `Cmd::Opened` としてキューへ戻す（待たない）。
    /// 発行できなければ false（呼び出し側が「開けなかった」として数える）。
    fn request_open(epoch: u64, id: &HSTRING, req: u64, tx: &mpsc::Sender<Cmd>) -> bool {
        let Ok(op) = MidiInPort::FromIdAsync(id) else {
            return false;
        };
        let (tx, id) = (tx.clone(), id.clone());
        op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
            let port = match (op.ok(), status) {
                (Ok(op), AsyncStatus::Completed) => op.GetResults().ok(),
                _ => None,
            };
            // ワーカーが無い（送れない）ときは port が drop されるだけ。
            let _ = tx.send(Cmd::Opened(epoch, id.clone(), req, port));
            Ok(())
        }))
        .is_ok()
    }

    /// 列挙完了かつ open 保留 0 件になったら `Started`（初回）/ `Sources`（以後）を流す。
    /// 開始が失敗確定（全部開けない）なら true（呼び出し側が teardown する）。
    fn settle(s: &mut Session) -> bool {
        if !s.enumerated || !s.pending.is_empty() {
            return false;
        }
        let (opened, failed) = s.inputs.counts();
        if s.started {
            with_router(|r| {
                r.emit(
                    s.epoch,
                    MidiEvent::Sources {
                        sources: opened,
                        failed,
                    },
                )
            });
            return false;
        }
        s.started = true;
        let ev = start_event(opened, failed);
        let failed_start = matches!(ev, MidiEvent::Failed(_));
        with_router(|r| r.emit(s.epoch, ev));
        failed_start
    }

    fn worker(rx: mpsc::Receiver<Cmd>, tx: mpsc::Sender<Cmd>) {
        // 失敗しても worker は続け、以後の `Start` を `Failed` にする（成功へ倒さない）。
        let com = ComInit::new("midi start");
        let mut session: Option<Session> = None;
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Start(e) => {
                    // 古い世代の Start は、新しいセッションを壊す前に捨てる。
                    if !with_router(|r| r.is_current(e)) {
                        continue;
                    }
                    if let Some(s) = session.take() {
                        teardown(s);
                    }
                    let result = match &com {
                        Ok(_) => open_session(e, &tx),
                        Err(_) => Err(DeviceError::Internal("midi start")),
                    };
                    match result {
                        Ok(s) => session = Some(s),
                        Err(err) => with_router(|r| r.emit(e, MidiEvent::Failed(err))),
                    }
                }
                Cmd::Stop(e) => {
                    if session.as_ref().is_some_and(|s| s.epoch < e) {
                        if let Some(s) = session.take() {
                            teardown(s);
                        }
                    }
                }
                Cmd::Added(e, id) => {
                    let Some(s) = session.as_mut().filter(|s| s.epoch == e) else {
                        continue;
                    };
                    let key = id.to_string_lossy();
                    if s.inputs.map.contains_key(&key) || s.pending.contains_key(&key) {
                        continue;
                    }
                    let req = s.next_req;
                    s.next_req += 1;
                    if request_open(e, &id, req, &tx) {
                        s.pending.insert(key, req);
                    } else {
                        s.inputs.insert(key, None);
                    }
                    if settle(s) {
                        if let Some(s) = session.take() {
                            teardown(s);
                        }
                    }
                }
                Cmd::Opened(e, id, req, port) => {
                    let key = id.to_string_lossy();
                    let live = session.as_mut().filter(|s| s.epoch == e);
                    let Some(s) = live.filter(|s| s.pending.get(&key) == Some(&req)) else {
                        // 取り消し・世代不一致・抜去済みで遅れて開いたポートは閉じる。
                        if let Some(p) = port {
                            let _ = p.Close();
                        }
                        continue;
                    };
                    s.pending.remove(&key);
                    s.inputs.insert(key, port.and_then(|p| register_port(e, p)));
                    if settle(s) {
                        if let Some(s) = session.take() {
                            teardown(s);
                        }
                    }
                }
                Cmd::Removed(e, id) => {
                    let Some(s) = session.as_mut().filter(|s| s.epoch == e) else {
                        continue;
                    };
                    let key = id.to_string_lossy();
                    // 保留中なら外し、遅れて届く `Opened` を捨てて `Close` させる。
                    s.pending.remove(&key);
                    if let Some((port, token)) = s.inputs.remove(&key) {
                        let _ = port.RemoveMessageReceived(token);
                        let _ = port.Close();
                    }
                    if settle(s) {
                        if let Some(s) = session.take() {
                            teardown(s);
                        }
                    }
                }
                Cmd::Stopped(e) => {
                    // 世代が一致する間は OS 起点の停止。成功を装わず失敗を通知して片付ける。
                    if session.as_ref().is_some_and(|s| s.epoch == e) {
                        with_router(|r| {
                            r.emit(e, MidiEvent::Failed(DeviceError::Internal("midi watcher")))
                        });
                        if let Some(s) = session.take() {
                            teardown(s);
                        }
                    }
                }
                Cmd::Enumerated(e) => {
                    let Some(s) = session.as_mut().filter(|s| s.epoch == e) else {
                        continue;
                    };
                    s.enumerated = true;
                    if settle(s) {
                        if let Some(s) = session.take() {
                            teardown(s);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn counting_sink(ret: bool) -> (MidiSink, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let sink: MidiSink = Arc::new(move |_| {
            n2.fetch_add(1, Ordering::SeqCst);
            ret
        });
        (sink, n)
    }

    fn started() -> MidiEvent {
        MidiEvent::Started {
            sources: 0,
            failed: 0,
        }
    }

    #[test]
    fn start_event_は0台を成功にし全部開けないときだけ失敗() {
        assert!(matches!(
            start_event(0, 0),
            MidiEvent::Started {
                sources: 0,
                failed: 0
            }
        ));
        assert!(matches!(
            start_event(0, 2),
            MidiEvent::Failed(DeviceError::Internal("midi open"))
        ));
        assert!(matches!(
            start_event(1, 1),
            MidiEvent::Started {
                sources: 1,
                failed: 1
            }
        ));
    }

    #[test]
    fn router_は_end_後に_sink_を呼ばない() {
        let (sink, n) = counting_sink(true);
        let mut r = Router::default();
        let e = r.begin(sink);
        r.emit(e, started());
        assert_eq!(n.load(Ordering::SeqCst), 1);
        r.end();
        r.emit(e, started());
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert!(!r.is_current(e));
    }

    #[test]
    fn router_は古い_epoch_を捨てる() {
        let (s1, n1) = counting_sink(true);
        let (s2, n2) = counting_sink(true);
        let mut r = Router::default();
        let e1 = r.begin(s1);
        let e2 = r.begin(s2);
        r.emit(e1, started());
        assert_eq!(n1.load(Ordering::SeqCst), 0);
        r.emit(e2, started());
        assert_eq!(n2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn 世代更新と投入は同じロックで順序が揃う() {
        use std::sync::mpsc;
        let router = Arc::new(Mutex::new(Router::default()));
        let (tx, rx) = mpsc::channel::<u64>();
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let (r, tx) = (router.clone(), tx.clone());
                std::thread::spawn(move || {
                    for _ in 0..200 {
                        let mut g = r.lock().unwrap();
                        if i % 2 == 0 {
                            let sink: MidiSink = Arc::new(|_| true);
                            g.begin_enqueue(sink, |e| tx.send(e).is_ok());
                        } else {
                            g.end_enqueue(|e| tx.send(e).unwrap());
                        }
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        drop(tx);
        let got: Vec<u64> = rx.iter().collect();
        assert!(got.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn router_は_false_を返した_sink_を以後呼ばない() {
        let (sink, n) = counting_sink(false);
        let mut r = Router::default();
        let e = r.begin(sink);
        r.emit(e, started());
        r.emit(e, started());
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn inputs_は二重追加を無視し数え直す() {
        let mut i: Inputs<u8> = Inputs::new();
        assert!(i.insert("a".into(), Some(1)));
        assert!(!i.insert("a".into(), None));
        assert!(i.insert("b".into(), None));
        assert_eq!(i.counts(), (1, 1));
        assert_eq!(i.remove("b"), None);
        assert_eq!(i.counts(), (1, 0));
        assert_eq!(i.remove("a"), Some(1));
        assert_eq!(i.counts(), (0, 0));
    }

    #[test]
    fn inputs_の_drain_は開けたポートを全部返す() {
        let mut i: Inputs<u8> = Inputs::new();
        i.insert("a".into(), Some(1));
        i.insert("b".into(), Some(2));
        i.insert("c".into(), None);
        let mut v = i.drain();
        v.sort();
        assert_eq!(v, vec![1, 2]);
        assert_eq!(i.counts(), (0, 0));
    }
}
