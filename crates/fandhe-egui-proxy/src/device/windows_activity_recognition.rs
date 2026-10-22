//! Windows 向け行動認識実装（Issue #613、親 #124）。`device/activity_recognition.rs` の Windows 経路から
//! UI スレッドで呼ばれ、WinRT `Windows.Devices.Sensors.ActivitySensor` の最新の行動を
//! 共有状態経由で pull させる（`ActivityRecognizer` トレイトを実装）。
//!
//! # 流れ
//!
//! - プロセスで 1 本の専用ワーカーに開始・停止を直列化する（`windows_midi.rs` と同型、契約表 11）。
//!   `start` / `stop` は共有状態（[`State`]）を更新して同じロックの内側でワーカーへ命令を送るだけで、
//!   UI スレッドをブロックしない
//! - ワーカーの開始: `GetDefaultAsync` を `SetCompleted` + `recv_timeout`（期限は
//!   `NO_READING_TIMEOUT_MS` と同じ値。OS 側に別の期限を持たない）で待つ。null は `Unsupported`、
//!   エラーは HRESULT を写す。取れたら写像できる種別を `SubscribedActivities` へ足し、
//!   `ReadingChanged` を登録し、初期値を `GetCurrentReadingAsync` で受ける
//! - `ReadingChanged` ハンドラは写像して [`State::store`] へ流すだけ（ブロック・panic・ログ出力をしない）
//! - 停止: `RemoveReadingChanged`・`SubscribedActivities().Clear()`・センサー破棄（結果は無視、冪等）
//! - ロックの内側で WinRT を呼ばない（イベントが同期発火しても自己デッドロックしないため）
//!
//! 世代（epoch）: 開始・停止のたびに進め、古い世代の完了・通知は [`State`] のロック内の判定で捨てる
//! （契約表 2）。購読の途中で失敗したら登録済みの分を外してから失敗を確定する（契約表 9）。
//!
//! HRESULT・行動種別・確度・デバイス ID はログにも画面にも出さない（security.md A09）。`unsafe` と
//! `RoInitialize` は使わない（既存の Windows 実装群と同じ前提）。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.activitysensor>
//!
//! # 既知の天井
//!
//! - 行動認識センサーを搭載した PC はほとんど無く、多くは `Unsupported` 表示になる
//! - app capability `activity` の要否、パッケージ無しの素の exe からの動作、拒否時の HRESULT は
//!   実機未確認（#603）。届かないと判明したら対応表の `done` を取り下げる
//! - `Idle` / `Fidgeting` は `Stationary` にまとめる。範囲外の値はサンプルごと捨てる
//! - ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use fandhe_device::activity_recognition::{
    ActivityFailure, ActivityKind, ActivityPoll, ActivityReading, Confidence,
};

/// `E_ACCESSDENIED`（0x80070005）。権限拒否の写像に使う。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// `ActivityType`（Unknown=0 / Idle=1 / Stationary=2 / Fidgeting=3 / Walking=4 / Running=5 /
/// InVehicle=6 / Biking=7）を種別へ写す。範囲外は `None`（サンプルごと捨てる）。
fn kind_from_raw(raw: i32) -> Option<ActivityKind> {
    match raw {
        0 => Some(ActivityKind::Unknown),
        1..=3 => Some(ActivityKind::Stationary),
        4 => Some(ActivityKind::Walking),
        5 => Some(ActivityKind::Running),
        6 => Some(ActivityKind::Automotive),
        7 => Some(ActivityKind::Cycling),
        _ => None,
    }
}

/// `ActivitySensorReadingConfidence`（High=0 / Low=1）を確度へ写す。範囲外は `None`（捨てる）。
fn confidence_from_raw(raw: i32) -> Option<Confidence> {
    match raw {
        0 => Some(Confidence::High),
        1 => Some(Confidence::Low),
        _ => None,
    }
}

/// 失敗 HRESULT を契約の失敗へ写す。0（S_OK + null）は搭載センサー無し。
fn failure_from_hresult(code: i32) -> ActivityFailure {
    match code {
        0 => ActivityFailure::Unsupported,
        E_ACCESSDENIED => ActivityFailure::PermissionDenied,
        _ => ActivityFailure::Failed,
    }
}

/// 非同期操作の終了状態から失敗 HRESULT を決める。`Error` のときは `GetResults()` が返す元の
/// HRESULT を保つ（`-1` に潰すと `E_ACCESSDENIED` が `Failed` になる）。
/// `results_code` は `GetResults()` が失敗したときの HRESULT。
#[cfg(any(target_os = "windows", test))]
fn async_failure_code(is_error: bool, results_code: Option<i32>) -> i32 {
    match (is_error, results_code) {
        (true, Some(code)) => code,
        _ => -1,
    }
}

/// ワーカーと `read` が共有する状態。1 本の `Mutex` で守り、判定と更新を同じロックの内側で行う。
#[derive(Default)]
struct State {
    /// 開始・停止のたびに進む世代。
    epoch: u64,
    /// 購読を望んでいるか。
    active: bool,
    /// 最新の行動。
    latest: Option<ActivityReading>,
    /// 確定した失敗（世代つき）。
    failure: Option<(u64, ActivityFailure)>,
    /// 次に振る受信番号（購読し直しても巻き戻さない）。
    next_seq: u64,
    /// この世代で `ReadingChanged` の通知を受け取り済みか。遅れて完了した初期値
    /// （`GetCurrentReadingAsync`）が新しい通知を上書きしないための判定。
    changed_seen: bool,
}

impl State {
    /// 購読を始める。世代を進め、値と失敗を消して新しい世代を返す。
    fn begin(&mut self) -> u64 {
        self.epoch += 1;
        self.active = true;
        self.latest = None;
        self.failure = None;
        self.changed_seen = false;
        self.epoch
    }

    /// 購読を止める。世代を進めて古い世代の通知を無効にし、最新値を捨てる。
    fn end(&mut self) {
        self.epoch += 1;
        self.active = false;
        self.latest = None;
        self.failure = None;
        self.changed_seen = false;
    }

    /// 世代が現在で購読中か。
    fn is_current(&self, epoch: u64) -> bool {
        self.active && self.epoch == epoch
    }

    /// 変更通知の値。現在の世代かつ購読中のときだけ最新値を入れる。入れたら `true`。
    fn store(&mut self, epoch: u64, kind: ActivityKind, confidence: Confidence) -> bool {
        if !self.is_current(epoch) {
            return false;
        }
        self.changed_seen = true;
        self.put(kind, confidence)
    }

    /// 初期値。変更通知を受信済みなら古い値として捨てる（判定と更新を同じロックで行う）。
    fn store_initial(&mut self, epoch: u64, kind: ActivityKind, confidence: Confidence) -> bool {
        if !self.is_current(epoch) || self.changed_seen {
            return false;
        }
        self.put(kind, confidence)
    }

    fn put(&mut self, kind: ActivityKind, confidence: Confidence) -> bool {
        self.next_seq += 1;
        self.latest = Some(ActivityReading {
            kind,
            confidence,
            seq: self.next_seq,
        });
        true
    }

    /// 無効なサンプルを受けたとき、現在の世代なら保持中の最新値を捨てる（`ActivityPoll` の契約は
    /// 「無効なサンプルを捨てた後」は `Waiting`。以前の値を返し続けない）。
    fn discard(&mut self, epoch: u64) {
        if self.is_current(epoch) {
            self.changed_seen = true;
            self.latest = None;
        }
    }

    /// 初期値の無効サンプル。変更通知を受信済みなら何もしない。
    fn discard_initial(&mut self, epoch: u64) {
        if self.is_current(epoch) && !self.changed_seen {
            self.latest = None;
        }
    }

    /// 現在の世代のときだけ失敗を確定する。確定したら `true`。
    fn fail(&mut self, epoch: u64, f: ActivityFailure) -> bool {
        if self.is_current(epoch) {
            self.failure = Some((epoch, f));
            true
        } else {
            false
        }
    }

    /// `read` の結果。失敗があれば `Err`、無ければ最新値か `Waiting`。許可待ちは返さない。
    fn poll(&self) -> Result<ActivityPoll, ActivityFailure> {
        if let Some((e, f)) = self.failure {
            if self.is_current(e) {
                return Err(f);
            }
        }
        if !self.active {
            return Ok(ActivityPoll::Waiting);
        }
        Ok(self
            .latest
            .map_or(ActivityPoll::Waiting, ActivityPoll::Reading))
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
    use std::time::Duration;

    use windows::Devices::Sensors::{
        ActivitySensor, ActivitySensorReading, ActivitySensorReadingChangedEventArgs,
    };
    use windows::Foundation::TypedEventHandler;
    use windows_core::Ref;
    use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

    use super::*;
    use fandhe_device::activity_recognition::{ActivityRecognizer, NO_READING_TIMEOUT_MS};

    static STATE: Mutex<State> = Mutex::new(State {
        epoch: 0,
        active: false,
        latest: None,
        failure: None,
        next_seq: 0,
        changed_seen: false,
    });

    fn lock() -> MutexGuard<'static, State> {
        STATE.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// ワーカーへの命令。
    enum Cmd {
        Start(u64),
        Stop,
    }

    fn sender() -> &'static Sender<Cmd> {
        static TX: OnceLock<Sender<Cmd>> = OnceLock::new();
        TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Cmd>();
            std::thread::Builder::new()
                .name("fandhe-activity-recognition".into())
                .spawn(move || worker(rx))
                // スレッドを作れないのは資源枯渇で、回復できない初期化不変条件。
                .expect("行動認識ワーカーを起動できない");
            tx
        })
    }

    pub(crate) struct WindowsActivityRecognizer;

    pub(crate) fn implementation() -> Arc<dyn ActivityRecognizer> {
        Arc::new(WindowsActivityRecognizer)
    }

    impl ActivityRecognizer for WindowsActivityRecognizer {
        fn start(&self) -> Result<(), ActivityFailure> {
            let tx = sender();
            let mut st = lock();
            if st.active {
                return Ok(());
            }
            let epoch = st.begin();
            // 状態の更新と命令の送信を同じロックの内側で行い、命令の順序を状態の順序と揃える。
            if tx.send(Cmd::Start(epoch)).is_err() {
                st.end();
                return Err(ActivityFailure::Failed);
            }
            Ok(())
        }

        fn read(&self) -> Result<ActivityPoll, ActivityFailure> {
            lock().poll()
        }

        fn stop(&self) {
            let tx = sender();
            let mut st = lock();
            st.end();
            // 送信に失敗してもワーカーが居ない（購読も無い）ので無視してよい。
            let _ = tx.send(Cmd::Stop);
        }
    }

    /// ワーカーが保持する購読（センサーと `ReadingChanged` のトークン）。
    type Held = (ActivitySensor, i64);

    fn worker(rx: mpsc::Receiver<Cmd>) {
        let mut held: Option<Held> = None;
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Stop => release(&mut held),
                Cmd::Start(epoch) => {
                    release(&mut held);
                    held = subscribe(epoch);
                }
            }
        }
        release(&mut held);
    }

    /// 購読を外す。結果は無視して冪等にする。
    fn release(held: &mut Option<Held>) {
        if let Some((sensor, token)) = held.take() {
            unsubscribe(&sensor, token);
        }
    }

    fn unsubscribe(sensor: &ActivitySensor, token: i64) {
        let _ = sensor.RemoveReadingChanged(token);
        if let Ok(subs) = sensor.SubscribedActivities() {
            let _ = subs.Clear();
        }
    }

    /// 1 件の読みを写像して現在の世代なら共有状態へ入れる。範囲外はサンプルごと捨て、保持中の
    /// 最新値も消す（契約表 6）。プロパティ取得の失敗は契約上の失敗として確定する。
    fn deliver(epoch: u64, reading: &ActivitySensorReading, initial: bool) {
        let activity = match reading.Activity() {
            Ok(a) => a.0,
            Err(e) => {
                lock().fail(epoch, failure_from_hresult(e.code().0));
                return;
            }
        };
        let confidence = match reading.Confidence() {
            Ok(c) => c.0,
            Err(e) => {
                lock().fail(epoch, failure_from_hresult(e.code().0));
                return;
            }
        };
        let mut st = lock();
        match (kind_from_raw(activity), confidence_from_raw(confidence)) {
            (Some(kind), Some(conf)) => {
                if initial {
                    st.store_initial(epoch, kind, conf);
                } else {
                    st.store(epoch, kind, conf);
                }
            }
            _ if initial => st.discard_initial(epoch),
            _ => st.discard(epoch),
        }
    }

    /// `GetDefaultAsync` の結果。期限切れは状態を変えない（ドライバの期限が `NoReading` を確定する）。
    enum Fetched {
        Sensor(ActivitySensor),
        Failed(i32),
        TimedOut,
    }

    fn fetch_sensor() -> Fetched {
        let op = match ActivitySensor::GetDefaultAsync() {
            Ok(op) => op,
            Err(e) => return Fetched::Failed(e.code().0),
        };
        let (tx, rx) = mpsc::channel();
        let set = op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
            let result = match (op.ok(), status) {
                (Ok(op), AsyncStatus::Completed) => op.GetResults().map_err(|e| e.code().0),
                // エラーは元の HRESULT を取り出す。キャンセル等は -1（偽の成功を作らない）。
                (Ok(op), AsyncStatus::Error) => Err(async_failure_code(
                    true,
                    op.GetResults().err().map(|e| e.code().0),
                )),
                _ => Err(async_failure_code(false, None)),
            };
            // 受信側が期限切れで落ちていても無視してよい。
            let _ = tx.send(result);
            Ok(())
        }));
        if let Err(e) = set {
            return Fetched::Failed(e.code().0);
        }
        let timeout = Duration::from_millis(NO_READING_TIMEOUT_MS as u64);
        match rx.recv_timeout(timeout) {
            Ok(Ok(sensor)) => Fetched::Sensor(sensor),
            Ok(Err(code)) => Fetched::Failed(code),
            Err(RecvTimeoutError::Timeout) => {
                let _ = op.Cancel();
                Fetched::TimedOut
            }
            Err(RecvTimeoutError::Disconnected) => Fetched::Failed(-1),
        }
    }

    /// 購読を張る。失敗時は登録済みの分を外してから失敗を確定する（契約表 9）。
    fn subscribe(epoch: u64) -> Option<Held> {
        if !lock().is_current(epoch) {
            return None;
        }
        let sensor = match fetch_sensor() {
            Fetched::Sensor(s) => s,
            Fetched::Failed(code) => {
                lock().fail(epoch, failure_from_hresult(code));
                return None;
            }
            Fetched::TimedOut => return None,
        };
        // 待っている間に停止・世代交代していたら破棄して終わり。
        if !lock().is_current(epoch) {
            return None;
        }

        let subs = match sensor.SubscribedActivities() {
            Ok(v) => v,
            Err(e) => {
                lock().fail(epoch, failure_from_hresult(e.code().0));
                return None;
            }
        };
        let supported = match sensor.SupportedActivities() {
            Ok(v) => v,
            Err(e) => {
                let _ = subs.Clear();
                lock().fail(epoch, failure_from_hresult(e.code().0));
                return None;
            }
        };
        let mut any = false;
        for a in supported {
            if kind_from_raw(a.0).is_none() {
                continue;
            }
            // 一部だけ登録できた不完全な購読を成功扱いにしない。追加済みの分を外して失敗を確定する。
            if let Err(e) = subs.Append(a) {
                let _ = subs.Clear();
                lock().fail(epoch, failure_from_hresult(e.code().0));
                return None;
            }
            any = true;
        }
        if !any {
            let _ = subs.Clear();
            lock().fail(epoch, ActivityFailure::Unsupported);
            return None;
        }

        let token = sensor.ReadingChanged(&TypedEventHandler::new(
            move |_: Ref<'_, ActivitySensor>,
                  args: Ref<'_, ActivitySensorReadingChangedEventArgs>| {
                // 読み取りの失敗を握りつぶさない。失敗を確定すると `read` が `Err` を返し、
                // ドライバが停止・失敗表示へつなぐ（古い行動を出し続けない）。
                match args.ok().and_then(|a| a.Reading()) {
                    Ok(reading) => deliver(epoch, &reading, false),
                    Err(e) => {
                        lock().fail(epoch, failure_from_hresult(e.code().0));
                    }
                }
                Ok(())
            },
        ));
        let token = match token {
            Ok(t) => t,
            Err(e) => {
                let _ = subs.Clear();
                lock().fail(epoch, failure_from_hresult(e.code().0));
                return None;
            }
        };

        // 世代の再確認と保持の決定を 1 回のロックで行う（外した後に届く通知は `store` が捨てる）。
        if !lock().is_current(epoch) {
            unsubscribe(&sensor, token);
            return None;
        }

        // 初期値。ワーカーは待たない。null（HRESULT 0）は値なしのまま、それ以外の失敗は元の
        // HRESULT を契約上の失敗へ写して現在の世代に確定し、購読を止める（契約表 5）。
        let fail_and_stop = {
            let sensor = sensor.clone();
            move |code: i32| {
                if code == 0 {
                    return;
                }
                if lock().fail(epoch, failure_from_hresult(code)) {
                    unsubscribe(&sensor, token);
                }
            }
        };
        match sensor.GetCurrentReadingAsync() {
            Ok(op) => {
                let on_fail = fail_and_stop.clone();
                let set =
                    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
                        match (op.ok(), status) {
                            (Ok(op), AsyncStatus::Completed) => match op.GetResults() {
                                Ok(reading) => deliver(epoch, &reading, true),
                                Err(e) => on_fail(e.code().0),
                            },
                            (Ok(op), AsyncStatus::Error) => on_fail(async_failure_code(
                                true,
                                op.GetResults().err().map(|e| e.code().0),
                            )),
                            _ => on_fail(async_failure_code(false, None)),
                        }
                        Ok(())
                    }));
                if let Err(e) = set {
                    fail_and_stop(e.code().0);
                }
            }
            Err(e) => fail_and_stop(e.code().0),
        }
        Some((sensor, token))
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_mapping() {
        assert_eq!(kind_from_raw(0), Some(ActivityKind::Unknown));
        for raw in 1..=3 {
            assert_eq!(kind_from_raw(raw), Some(ActivityKind::Stationary));
        }
        assert_eq!(kind_from_raw(4), Some(ActivityKind::Walking));
        assert_eq!(kind_from_raw(5), Some(ActivityKind::Running));
        assert_eq!(kind_from_raw(6), Some(ActivityKind::Automotive));
        assert_eq!(kind_from_raw(7), Some(ActivityKind::Cycling));
        assert_eq!(kind_from_raw(8), None);
        assert_eq!(kind_from_raw(-1), None);
    }

    #[test]
    fn confidence_mapping() {
        assert_eq!(confidence_from_raw(0), Some(Confidence::High));
        assert_eq!(confidence_from_raw(1), Some(Confidence::Low));
        assert_eq!(confidence_from_raw(2), None);
        assert_eq!(confidence_from_raw(-1), None);
    }

    #[test]
    fn hresult_mapping_never_yields_success() {
        assert_eq!(failure_from_hresult(0), ActivityFailure::Unsupported);
        assert_eq!(
            failure_from_hresult(E_ACCESSDENIED),
            ActivityFailure::PermissionDenied
        );
        assert_eq!(failure_from_hresult(-1), ActivityFailure::Failed);
    }

    #[test]
    fn async_error_keeps_access_denied() {
        let code = async_failure_code(true, Some(E_ACCESSDENIED));
        assert_eq!(
            failure_from_hresult(code),
            ActivityFailure::PermissionDenied
        );
        assert_eq!(async_failure_code(false, None), -1);
        assert_eq!(async_failure_code(true, None), -1);
        assert_eq!(
            failure_from_hresult(async_failure_code(false, Some(0))),
            ActivityFailure::Failed
        );
    }

    #[test]
    fn state_drops_stale_and_stopped_stores() {
        let mut s = State::default();
        let e1 = s.begin();
        assert!(s.store(e1, ActivityKind::Walking, Confidence::High));
        let e2 = {
            s.end();
            s.begin()
        };
        // 古い世代の通知は捨てる。
        assert!(!s.store(e1, ActivityKind::Running, Confidence::High));
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
        s.end();
        // 停止後の通知も捨てる。
        assert!(!s.store(e2, ActivityKind::Running, Confidence::High));
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
    }

    #[test]
    fn state_seq_is_monotonic_across_restarts_and_stop_clears_latest() {
        let mut s = State::default();
        let e = s.begin();
        s.store(e, ActivityKind::Walking, Confidence::High);
        let first = match s.poll() {
            Ok(ActivityPoll::Reading(r)) => r.seq,
            other => panic!("unexpected {other:?}"),
        };
        s.end();
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
        let e = s.begin();
        // 開始直後は直前の値を返さない。
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
        s.store(e, ActivityKind::Walking, Confidence::High);
        match s.poll() {
            Ok(ActivityPoll::Reading(r)) => assert!(r.seq > first),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn late_initial_does_not_override_change_notification() {
        let mut s = State::default();
        let e = s.begin();
        assert!(s.store(e, ActivityKind::Running, Confidence::High));
        // 遅れて完了した初期値（歩行）は捨てる。
        assert!(!s.store_initial(e, ActivityKind::Walking, Confidence::High));
        s.discard_initial(e);
        match s.poll() {
            Ok(ActivityPoll::Reading(r)) => assert_eq!(r.kind, ActivityKind::Running),
            other => panic!("unexpected {other:?}"),
        }
        // 通知が先に無ければ初期値は入る。
        s.end();
        let e = s.begin();
        assert!(s.store_initial(e, ActivityKind::Walking, Confidence::High));
    }

    #[test]
    fn invalid_sample_clears_latest() {
        let mut s = State::default();
        let e = s.begin();
        s.store(e, ActivityKind::Walking, Confidence::High);
        // 古い世代の無効サンプルは最新値を消さない。
        s.discard(e + 1);
        assert!(matches!(s.poll(), Ok(ActivityPoll::Reading(_))));
        s.discard(e);
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
    }

    #[test]
    fn state_failure_is_per_epoch_and_never_success() {
        let mut s = State::default();
        let e1 = s.begin();
        s.fail(e1, ActivityFailure::Unsupported);
        assert_eq!(s.poll(), Err(ActivityFailure::Unsupported));
        s.end();
        // 古い世代の失敗は確定しない。
        s.fail(e1, ActivityFailure::Failed);
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
        let e2 = s.begin();
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
        s.fail(e1, ActivityFailure::Failed);
        assert_eq!(s.poll(), Ok(ActivityPoll::Waiting));
        s.fail(e2, ActivityFailure::PermissionDenied);
        assert_eq!(s.poll(), Err(ActivityFailure::PermissionDenied));
    }
}
