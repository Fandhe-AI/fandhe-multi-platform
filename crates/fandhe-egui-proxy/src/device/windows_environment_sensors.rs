//! Windows 向け気圧・照度・近接センサー実装（Issue #612、親 #123）。`device/environment_sensors.rs` の
//! Windows 経路の dispatch 先で、UI スレッドの driver から種類ごとに `start` / `read` / `stop` を呼ばれる。
//! WinRT `Windows.Devices.Sensors` の `Barometer` / `LightSensor` / `ProximitySensor` を使う
//! （`ReadingChanged` は購読しない。最新値を同期に pull する。契約は `fandhe-device` の
//! `environment_sensors` 参照）。
//!
//! # 流れ
//!
//! - 気圧 / 照度の `start`: `GetDefault()`。null（`e.code().0 == 0`）は `NotPresent`、`E_ACCESSDENIED` は
//!   `PermissionDenied`、他は `Failed`。`ReportInterval` を `max(MinimumReportInterval, 100 ms)` にする。
//!   設定に失敗したら 0 へ戻して保持せず失敗（契約表 9）。気圧は `ReportThreshold` を 0 にして
//!   小さな変化でも読みを出させる（Windows 10 1809 未満には無いので失敗は無視する）
//! - 気圧 / 照度の `read`: `GetCurrentReading()`。null は値なし。値は加工する前に
//!   `validate_pressure_hpa` / `validate_lux` で検査し、範囲外は値なし
//! - 近接の `start`: 世代を進めて専用ワーカーへ命令を送るだけ（UI スレッドを止めない）。ワーカーが
//!   `ProximitySensor::GetDeviceSelector` → `DeviceInformation::FindAllAsyncAqsFilter` →
//!   `ProximitySensor::FromId` を 1 回だけ行い、世代が現在のときだけ共有状態へ入れる。列挙 0 件・
//!   `FromId` の null は `NotPresent`。待機の期限は driver と同じ `NO_READING_TIMEOUT_MS` で、
//!   期限切れは状態を変えず driver の `NoReading` に任せる
//! - 近接の `read`: 共有状態を見て `GetCurrentReading().IsDetected()` を `Near` / `Far` にする
//! - `stop`: `ReportInterval` を 0 に戻して保持を捨てる（結果は無視）。近接は世代を進めて捨てる。冪等
//!
//! # 気圧の古さ判定
//!
//! `Timestamp` は壁時計由来のため経過時間の計算には使わない（契約表 3）。前回と同じ読みかどうかの
//! 等値比較にだけ使い、`Timestamp` が変わった読みを受信として単調時計（`Instant`）で記録する。
//! 開始後の最初の読みは年齢が分からないので基準として記録するだけで値なしにする
//! （OS に残っていた古い読みを出さない）。受信から `PRESSURE_STALE_MS` を超えたら値なし。
//!
//! # 背景遷移（最小化）
//!
//! eframe は最小化中に `update` を呼ばないため、フレームからは最小化を観測できない。
//! `flashlight.rs` と同型のウォッチャー（プロセスに 1 本）が HWND の `IsIconic` を 200 ms 間隔で見て、
//! 最小化で気圧の受信記録を捨てて `note_backgrounded`、復元で `note_resumed` を呼び、
//! `InvalidateRect` で 1 フレーム起こす（背景のまま走るフレームが再描画を予約せず、ポーリングが
//! 止まるのを防ぐ。契約表 4）。背景中に OS 側の購読は止めない。
//!
//! HRESULT・センサー値・デバイス ID・セレクタ文字列はログにも画面にも出さない（security.md A09）。
//! `unsafe` は `IsIconic` / `InvalidateRect` の呼び出し 2 箇所に限る。
//!
//! 一次情報:
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.barometer>、
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.lightsensor>、
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.sensors.proximitysensor>
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 近接の開始・停止・列挙結果の順序（1 本のワーカーと 1 本の `Mutex`、世代照合）、嘘の値
//! （0・前回値・範囲外・2 秒超の気圧）を出さない、失敗の種別を取り違えない、停止と失敗で保持を捨てる。
//! 守らない範囲: 絶対精度、近接の距離、OS 間で値がそろうこと、最小化中の表示、複数プロセス。
//!
//! # 既知の天井
//!
//! - パッケージ無しの素の exe での動作と、プライバシー設定で拒否されたときの HRESULT は実機未確認（#602）
//! - 近接を `ReadingChanged` なしで pull して値が出るかは実機待ち
//! - 気圧がほぼ一定の機種で OS が新しい読みを出さないと、古さ判定で `NoReading` になりうる
//! - 最小化からの復元フレームでの再描画再開（`InvalidateRect`）は実機待ち
//! - ローカルの型検査は `x86_64-pc-windows-gnu` のみ（MSVC 無し）
//!
//! 実機での確認は #602 で追う。届かないと判明したら対応表の `done` を取り下げる。

use std::time::Instant;

use fandhe_device::environment_sensors::{
    is_fresh, EnvSensorFailure, Proximity, PRESSURE_STALE_MS,
};

/// `E_ACCESSDENIED`（0x80070005）。権限拒否の写像に使う。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// 最小化の観測間隔。
#[cfg(any(target_os = "windows", test))]
const MINIMIZE_POLL_MS: u64 = 200;

/// 開始系の呼び出し（`GetDefault` / `MinimumReportInterval` / `SetReportInterval` /
/// `GetDeviceSelector` / `FindAllAsync` / `FromId`）の失敗 HRESULT を契約の失敗へ写す。
/// 0（S_OK + null）は端末にセンサーが無い。Windows 10 から公開 API があるので `UnsupportedOnOs` は返さない。
fn start_failure_from_hresult(code: i32) -> EnvSensorFailure {
    match code {
        0 => EnvSensorFailure::NotPresent,
        E_ACCESSDENIED => EnvSensorFailure::PermissionDenied,
        _ => EnvSensorFailure::Failed,
    }
}

/// 読み取り系（`GetCurrentReading` と各プロパティ）の失敗 HRESULT を写す。0（null の読み）は値なしで `None`。
fn read_failure_from_hresult(code: i32) -> Option<EnvSensorFailure> {
    match code {
        0 => None,
        E_ACCESSDENIED => Some(EnvSensorFailure::PermissionDenied),
        _ => Some(EnvSensorFailure::Failed),
    }
}

/// 列挙結果の件数から失敗を決める。0 件は端末に近接センサーが無い。
#[cfg(any(target_os = "windows", test))]
fn enumerate_outcome(count: u32) -> Result<(), EnvSensorFailure> {
    if count == 0 {
        Err(EnvSensorFailure::NotPresent)
    } else {
        Ok(())
    }
}

/// `IsDetected` を契約の 2 値へ写す。
#[cfg(any(target_os = "windows", test))]
fn proximity_from_detected(detected: bool) -> Proximity {
    if detected {
        Proximity::Near
    } else {
        Proximity::Far
    }
}

/// `ReportInterval`（ms）。センサーの最小値と 100 ms の大きいほう。
#[cfg(any(target_os = "windows", test))]
fn report_interval(min: u32) -> u32 {
    min.max(100)
}

/// 非同期操作の終了状態から失敗 HRESULT を決める。`Error` のときは `GetResults()` が返す元の
/// HRESULT を保つ（`-1` に潰すと `E_ACCESSDENIED` が `Failed` になる）。
#[cfg(any(target_os = "windows", test))]
fn async_failure_code(is_error: bool, results_code: Option<i32>) -> i32 {
    match (is_error, results_code) {
        (true, Some(code)) => code,
        _ => -1,
    }
}

/// 気圧の古さ判定の状態。`Timestamp`（壁時計）は等値比較だけに使い、経過は `Instant` で測る。
#[derive(Debug, Default)]
struct PressureFreshness {
    /// 前回の読みの `Timestamp`。
    last_ts: Option<i64>,
    /// `Timestamp` が変わった読みを受けた単調時刻。
    received_at: Option<Instant>,
}

impl PressureFreshness {
    /// 読みの `Timestamp` を観測し、いま新しい値として扱えるかを返す。
    /// 最初の読みは年齢が分からないため基準として記録するだけで `false`。
    fn observe(&mut self, ts: i64, now: Instant) -> bool {
        match self.last_ts {
            None => {
                self.last_ts = Some(ts);
                false
            }
            Some(prev) if prev != ts => {
                self.last_ts = Some(ts);
                self.received_at = Some(now);
                true
            }
            Some(_) => self.received_at.is_some_and(|r| {
                let elapsed_ms = now.saturating_duration_since(r).as_secs_f64() * 1000.0;
                is_fresh(0.0, elapsed_ms, PRESSURE_STALE_MS)
            }),
        }
    }

    /// 背景遷移で受信記録を捨てる。基準の `Timestamp` は残し、復元後は新しい読みだけを受ける。
    fn drop_received(&mut self) {
        self.received_at = None;
    }
}

/// 近接の共有状態。ワーカーと `read` が 1 本の `Mutex` の内側で判定と更新を行う（契約表 2）。
#[cfg(any(target_os = "windows", test))]
struct ProxState<T> {
    /// 開始・停止のたびに進む世代。
    epoch: u64,
    /// 購読を望んでいるか。
    active: bool,
    /// 取得済みのセンサー。
    sensor: Option<T>,
    /// 確定した失敗（世代つき）。
    failure: Option<(u64, EnvSensorFailure)>,
}

#[cfg(any(target_os = "windows", test))]
impl<T: Clone> ProxState<T> {
    const fn new() -> Self {
        Self {
            epoch: 0,
            active: false,
            sensor: None,
            failure: None,
        }
    }

    /// 開始する。世代を進め、センサーと失敗を消して新しい世代を返す。
    fn begin(&mut self) -> u64 {
        self.epoch += 1;
        self.active = true;
        self.sensor = None;
        self.failure = None;
        self.epoch
    }

    /// 停止する。世代を進めて保持を捨てる。冪等。
    fn end(&mut self) {
        self.epoch += 1;
        self.active = false;
        self.sensor = None;
        self.failure = None;
    }

    fn is_current(&self, epoch: u64) -> bool {
        self.active && self.epoch == epoch
    }

    /// 現在の世代のときだけセンサーを入れる。
    fn install(&mut self, epoch: u64, sensor: T) -> bool {
        if !self.is_current(epoch) {
            return false;
        }
        self.sensor = Some(sensor);
        true
    }

    /// 現在の世代のときだけ失敗を確定する。
    fn fail(&mut self, epoch: u64, f: EnvSensorFailure) -> bool {
        if !self.is_current(epoch) {
            return false;
        }
        self.failure = Some((epoch, f));
        true
    }

    /// 現在の世代の失敗なら `Err`、センサーがあれば clone、無ければ `Ok(None)`。
    fn poll(&self) -> Result<Option<T>, EnvSensorFailure> {
        if !self.active {
            return Ok(None);
        }
        if let Some((e, f)) = self.failure {
            if e == self.epoch {
                return Err(f);
            }
        }
        Ok(self.sensor.clone())
    }
}

/// 最小化の立ち上がり・立ち下がり。
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Minimized,
    Restored,
}

/// 最小化状態の変化検出。同じ状態の連続では何も返さない。
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default)]
struct MinimizeEdge {
    was: bool,
}

#[cfg(any(target_os = "windows", test))]
impl MinimizeEdge {
    fn update(&mut self, minimized: bool) -> Option<Edge> {
        let prev = std::mem::replace(&mut self.was, minimized);
        match (prev, minimized) {
            (false, true) => Some(Edge::Minimized),
            (true, false) => Some(Edge::Restored),
            _ => None,
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
    use std::time::Duration;

    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Sensors::{Barometer, LightSensor, ProximitySensor};
    use windows::Win32::Graphics::Gdi::InvalidateRect;
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;
    use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

    use super::*;
    use fandhe_device::environment_sensors::{
        validate_lux, validate_pressure_hpa, EnvReading, EnvironmentSensors, SensorKind,
        NO_READING_TIMEOUT_MS,
    };

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 気圧: 保持しているセンサーと古さ判定。
    type PressureSlot = Option<(Barometer, PressureFreshness)>;

    pub(super) struct WindowsEnvironmentSensors {
        light: Mutex<Option<LightSensor>>,
    }

    pub(crate) fn implementation() -> Arc<dyn EnvironmentSensors> {
        Arc::new(WindowsEnvironmentSensors {
            light: Mutex::new(None),
        })
    }

    /// 気圧の保持（プロセスで 1 個。背景遷移で受信記録を捨てるためウォッチャーからも触る）。
    static PRESSURE: Mutex<PressureSlot> = Mutex::new(None);

    // ---- 近接 ----

    static PROX: Mutex<ProxState<ProximitySensor>> = Mutex::new(ProxState::new());

    enum Cmd {
        Start(u64),
        Stop,
    }

    fn sender() -> &'static Sender<Cmd> {
        static TX: OnceLock<Sender<Cmd>> = OnceLock::new();
        TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Cmd>();
            std::thread::Builder::new()
                .name("fandhe-environment-sensors-proximity".into())
                .spawn(move || worker(rx))
                // スレッドを作れないのは資源枯渇で、回復できない初期化不変条件。
                .expect("近接センサーのワーカーを起動できない");
            tx
        })
    }

    /// 近接の列挙を 1 本に直列化するワーカー。`Stop` 以外は開始のたびに 1 回だけ列挙する。
    fn worker(rx: mpsc::Receiver<Cmd>) {
        while let Ok(cmd) = rx.recv() {
            if let Cmd::Start(epoch) = cmd {
                enumerate(epoch);
            }
        }
    }

    enum Found {
        Sensor(ProximitySensor),
        Failed(EnvSensorFailure),
        TimedOut,
    }

    /// 列挙して 1 件目を `FromId` で取る。待機の前後で世代を確かめる。
    fn enumerate(epoch: u64) {
        if !lock(&PROX).is_current(epoch) {
            return;
        }
        match find_sensor() {
            Found::Sensor(s) => {
                // 待っている間に停止・世代交代していたら破棄される。
                lock(&PROX).install(epoch, s);
            }
            Found::Failed(f) => {
                lock(&PROX).fail(epoch, f);
            }
            // 期限切れは状態を変えない。driver の期限が `NoReading` を確定する。
            Found::TimedOut => {}
        }
    }

    fn find_sensor() -> Found {
        let fail = |code: i32| Found::Failed(start_failure_from_hresult(code));
        let selector = match ProximitySensor::GetDeviceSelector() {
            Ok(s) => s,
            Err(e) => return fail(e.code().0),
        };
        let op = match DeviceInformation::FindAllAsyncAqsFilter(&selector) {
            Ok(op) => op,
            Err(e) => return fail(e.code().0),
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
            return fail(e.code().0);
        }
        let timeout = Duration::from_millis(NO_READING_TIMEOUT_MS as u64);
        let devices = match rx.recv_timeout(timeout) {
            Ok(Ok(d)) => d,
            Ok(Err(code)) => return fail(code),
            Err(RecvTimeoutError::Timeout) => {
                let _ = op.Cancel();
                return Found::TimedOut;
            }
            Err(RecvTimeoutError::Disconnected) => return fail(-1),
        };
        let count = match devices.Size() {
            Ok(n) => n,
            Err(e) => return fail(e.code().0),
        };
        if let Err(f) = enumerate_outcome(count) {
            return Found::Failed(f);
        }
        let id = match devices.GetAt(0).and_then(|d| d.Id()) {
            Ok(id) => id,
            Err(e) => return fail(e.code().0),
        };
        match ProximitySensor::FromId(&id) {
            Ok(s) => Found::Sensor(s),
            Err(e) => fail(e.code().0),
        }
    }

    // ---- 気圧 / 照度の共通処理 ----

    /// `ReportInterval` を整える。失敗したら 0 へ戻して失敗を返す（保持しない）。
    macro_rules! configure {
        ($sensor:expr) => {{
            let sensor = $sensor;
            let interval = report_interval(
                sensor
                    .MinimumReportInterval()
                    .map_err(|e| start_failure_from_hresult(e.code().0))?,
            );
            if let Err(e) = sensor.SetReportInterval(interval) {
                let _ = sensor.SetReportInterval(0);
                return Err(start_failure_from_hresult(e.code().0));
            }
            sensor
        }};
    }

    impl WindowsEnvironmentSensors {
        fn start_pressure(&self) -> Result<(), EnvSensorFailure> {
            let mut guard = lock(&PRESSURE);
            if guard.is_some() {
                return Ok(());
            }
            let sensor =
                Barometer::GetDefault().map_err(|e| start_failure_from_hresult(e.code().0))?;
            let sensor = configure!(sensor);
            // 1809 未満には無い。失敗しても読み取りは成立するので無視する。
            if let Ok(th) = sensor.ReportThreshold() {
                let _ = th.SetHectopascals(0.0);
            }
            *guard = Some((sensor, PressureFreshness::default()));
            Ok(())
        }

        fn read_pressure(&self) -> Result<Option<EnvReading>, EnvSensorFailure> {
            let sensor = match lock(&PRESSURE).as_ref() {
                Some((s, _)) => s.clone(),
                None => return Ok(None),
            };
            // WinRT の呼び出しはロックの外で行う。
            let (raw, ts) = match read_barometer(&sensor) {
                Ok(Some(v)) => v,
                Ok(None) => return Ok(None),
                Err(f) => return Err(f),
            };
            let mut guard = lock(&PRESSURE);
            // 呼び出し中に停止されていたら値を返さない。
            let Some((_, fresh)) = guard.as_mut() else {
                return Ok(None);
            };
            let is_fresh = fresh.observe(ts, Instant::now());
            // 範囲検査は値を加工する前に行う。
            Ok(validate_pressure_hpa(raw)
                .filter(|_| is_fresh)
                .map(EnvReading::PressureHpa))
        }

        fn start_light(&self) -> Result<(), EnvSensorFailure> {
            let mut guard = lock(&self.light);
            if guard.is_some() {
                return Ok(());
            }
            let sensor =
                LightSensor::GetDefault().map_err(|e| start_failure_from_hresult(e.code().0))?;
            *guard = Some(configure!(sensor));
            Ok(())
        }

        fn read_light(&self) -> Result<Option<EnvReading>, EnvSensorFailure> {
            let Some(sensor) = lock(&self.light).clone() else {
                return Ok(None);
            };
            let reading = match sensor.GetCurrentReading() {
                Ok(r) => r,
                Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
            };
            let raw = match reading.IlluminanceInLux() {
                Ok(v) => f64::from(v),
                Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
            };
            if lock(&self.light).is_none() {
                return Ok(None);
            }
            Ok(validate_lux(raw).map(EnvReading::IlluminanceLux))
        }

        fn read_proximity(&self) -> Result<Option<EnvReading>, EnvSensorFailure> {
            let Some(sensor) = lock(&PROX).poll()? else {
                return Ok(None);
            };
            let reading = match sensor.GetCurrentReading() {
                Ok(r) => r,
                Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
            };
            match reading.IsDetected() {
                Ok(d) => {
                    // 呼び出し中に停止されていたら値を返さない。
                    if !lock(&PROX).active {
                        return Ok(None);
                    }
                    Ok(Some(EnvReading::Proximity(proximity_from_detected(d))))
                }
                Err(e) => read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
            }
        }
    }

    /// 気圧と `Timestamp`（`DateTime.UniversalTime`）を読む。null は値なし。
    fn read_barometer(sensor: &Barometer) -> Result<Option<(f64, i64)>, EnvSensorFailure> {
        let reading = match sensor.GetCurrentReading() {
            Ok(r) => r,
            Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
        };
        let raw = match reading.StationPressureInHectopascals() {
            Ok(v) => v,
            Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
        };
        let ts = match reading.Timestamp() {
            Ok(t) => t.UniversalTime,
            Err(e) => return read_failure_from_hresult(e.code().0).map_or(Ok(None), Err),
        };
        Ok(Some((raw, ts)))
    }

    impl EnvironmentSensors for WindowsEnvironmentSensors {
        fn start(&self, kind: SensorKind) -> Result<(), EnvSensorFailure> {
            ensure_minimize_watcher();
            match kind {
                SensorKind::Pressure => self.start_pressure(),
                SensorKind::Light => self.start_light(),
                SensorKind::Proximity => {
                    let tx = sender();
                    let mut st = lock(&PROX);
                    if st.active {
                        return Ok(());
                    }
                    let epoch = st.begin();
                    // 状態の更新と命令の送信を同じロックの内側で行い、順序を揃える。
                    if tx.send(Cmd::Start(epoch)).is_err() {
                        st.end();
                        return Err(EnvSensorFailure::Failed);
                    }
                    Ok(())
                }
            }
        }

        fn read(&self, kind: SensorKind) -> Result<Option<EnvReading>, EnvSensorFailure> {
            ensure_minimize_watcher();
            match kind {
                SensorKind::Pressure => self.read_pressure(),
                SensorKind::Light => self.read_light(),
                SensorKind::Proximity => self.read_proximity(),
            }
        }

        fn stop(&self, kind: SensorKind) {
            match kind {
                SensorKind::Pressure => {
                    if let Some((s, _)) = lock(&PRESSURE).take() {
                        let _ = s.SetReportInterval(0);
                    }
                }
                SensorKind::Light => {
                    if let Some(s) = lock(&self.light).take() {
                        let _ = s.SetReportInterval(0);
                    }
                }
                SensorKind::Proximity => {
                    let tx = sender();
                    lock(&PROX).end();
                    // 送信に失敗してもワーカーが居ない（保持も無い）ので無視してよい。
                    let _ = tx.send(Cmd::Stop);
                }
            }
        }
    }

    // ---- 最小化ウォッチャー ----

    /// ウォッチャーを 1 回だけ起動する。ウィンドウ未取得なら起動せず次回に再試行する。
    fn ensure_minimize_watcher() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.load(Ordering::Acquire)
            || super::super::windows_share::window().is_none()
            || STARTED.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("fandhe-environment-sensors-minimize".into())
            .spawn(watch_minimize);
        if spawned.is_err() {
            STARTED.store(false, Ordering::Release);
        }
    }

    fn watch_minimize() {
        let mut edge = MinimizeEdge::default();
        loop {
            if let Some(hwnd) = super::super::windows_share::window() {
                match edge.update(is_minimized(hwnd)) {
                    Some(Edge::Minimized) => {
                        drop_pressure_received();
                        super::super::environment_sensors::note_backgrounded();
                    }
                    Some(Edge::Restored) => {
                        super::super::environment_sensors::note_resumed();
                        wake_frame(hwnd);
                    }
                    None => {}
                }
            }
            std::thread::sleep(Duration::from_millis(MINIMIZE_POLL_MS));
        }
    }

    /// 気圧の受信記録を捨てる（背景で保持値を捨てる）。
    fn drop_pressure_received() {
        if let Some((_, fresh)) = lock(&PRESSURE).as_mut() {
            fresh.drop_received();
        }
    }

    #[allow(unsafe_code)]
    fn is_minimized(hwnd: windows::Win32::Foundation::HWND) -> bool {
        // SAFETY: `hwnd` は `remember_window` が控えたメインウィンドウで、プロセス生存中は有効。
        // `IsIconic` は無効な HWND でも FALSE を返すだけで副作用がない。
        unsafe { IsIconic(hwnd) }.as_bool()
    }

    #[allow(unsafe_code)]
    fn wake_frame(hwnd: windows::Win32::Foundation::HWND) {
        // SAFETY: 同上。`InvalidateRect` は無効な HWND なら失敗を返すだけで、矩形に NULL を渡すと
        // クライアント領域全体を無効化する（次の描画を起こすだけ）。
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn hresult_mapping_never_yields_success() {
        assert_eq!(start_failure_from_hresult(0), EnvSensorFailure::NotPresent);
        assert_eq!(
            start_failure_from_hresult(E_ACCESSDENIED),
            EnvSensorFailure::PermissionDenied
        );
        assert_eq!(start_failure_from_hresult(-1), EnvSensorFailure::Failed);
        assert_eq!(read_failure_from_hresult(0), None);
        assert_eq!(
            read_failure_from_hresult(E_ACCESSDENIED),
            Some(EnvSensorFailure::PermissionDenied)
        );
        assert_eq!(
            read_failure_from_hresult(-1),
            Some(EnvSensorFailure::Failed)
        );
    }

    #[test]
    fn enumerate_zero_is_not_present_and_interval_has_floor() {
        assert_eq!(enumerate_outcome(0), Err(EnvSensorFailure::NotPresent));
        assert_eq!(enumerate_outcome(1), Ok(()));
        assert_eq!(report_interval(0), 100);
        assert_eq!(report_interval(50), 100);
        assert_eq!(report_interval(250), 250);
        assert_eq!(proximity_from_detected(true), Proximity::Near);
        assert_eq!(proximity_from_detected(false), Proximity::Far);
    }

    #[test]
    fn async_error_keeps_access_denied() {
        let code = async_failure_code(true, Some(E_ACCESSDENIED));
        assert_eq!(code, E_ACCESSDENIED);
        assert_eq!(
            start_failure_from_hresult(code),
            EnvSensorFailure::PermissionDenied
        );
        assert_eq!(async_failure_code(false, None), -1);
        assert_eq!(async_failure_code(true, None), -1);
    }

    #[test]
    fn pressure_first_reading_is_baseline_only() {
        let t0 = Instant::now();
        let mut f = PressureFreshness::default();
        assert!(!f.observe(10, t0));
        // 同じ Timestamp のままでは受信と見なさない。
        assert!(!f.observe(10, t0 + Duration::from_millis(100)));
        // 変わったら新しい。
        assert!(f.observe(11, t0 + Duration::from_millis(200)));
    }

    #[test]
    fn pressure_goes_stale_after_threshold() {
        let t0 = Instant::now();
        let mut f = PressureFreshness::default();
        f.observe(1, t0);
        assert!(f.observe(2, t0));
        assert!(f.observe(2, t0 + Duration::from_millis(2000)));
        assert!(!f.observe(2, t0 + Duration::from_millis(2001)));
    }

    #[test]
    fn pressure_background_drops_received_until_new_timestamp() {
        let t0 = Instant::now();
        let mut f = PressureFreshness::default();
        f.observe(1, t0);
        assert!(f.observe(2, t0));
        f.drop_received();
        assert!(!f.observe(2, t0 + Duration::from_millis(10)));
        assert!(f.observe(3, t0 + Duration::from_millis(20)));
        // stop 相当（作り直し）は最初から。
        let mut f = PressureFreshness::default();
        assert!(!f.observe(3, t0));
    }

    #[test]
    fn prox_drops_stale_installs_and_stopped_failures() {
        let mut s = ProxState::<u32>::new();
        assert_eq!(s.poll(), Ok(None));
        let e1 = s.begin();
        s.end();
        // 停止後の結果は無視される。
        assert!(!s.install(e1, 1));
        assert!(!s.fail(e1, EnvSensorFailure::Failed));
        assert_eq!(s.poll(), Ok(None));
        // 世代交代後に古い世代の結果は捨てられる。
        let e2 = s.begin();
        let e3 = {
            s.end();
            s.begin()
        };
        assert_ne!(e2, e3);
        assert!(!s.install(e2, 2));
        assert_eq!(s.poll(), Ok(None));
        assert!(s.install(e3, 3));
        assert_eq!(s.poll(), Ok(Some(3)));
        // end は冪等で、以後は値を返さない。
        s.end();
        s.end();
        assert_eq!(s.poll(), Ok(None));
    }

    #[test]
    fn prox_failure_is_per_epoch() {
        let mut s = ProxState::<u32>::new();
        let e = s.begin();
        assert!(s.fail(e, EnvSensorFailure::NotPresent));
        assert_eq!(s.poll(), Err(EnvSensorFailure::NotPresent));
        // 開始し直すと失敗は消える。
        s.begin();
        assert_eq!(s.poll(), Ok(None));
    }

    #[test]
    fn minimize_edge_reports_each_transition_once() {
        let mut m = MinimizeEdge::default();
        assert_eq!(m.update(false), None);
        assert_eq!(m.update(true), Some(Edge::Minimized));
        assert_eq!(m.update(true), None);
        assert_eq!(m.update(false), Some(Edge::Restored));
        assert_eq!(m.update(false), None);
        assert_eq!(MINIMIZE_POLL_MS, 200);
    }
}
