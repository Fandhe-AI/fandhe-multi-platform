//! Windows 向け Bluetooth Classic 実装（Issue #660、親 #130。骨格は #659）。
//!
//! 呼び出し元: `device/bluetooth_classic.rs` の Windows 経路が、一覧で [`implementation`]、検索で
//! [`present`] を呼び、終了時（`device::shutdown` → `stop_shared_discovery`）に [`stop_shared`] を呼ぶ。
//! dispatch・`device.rs` の `pub mod`・共通コアの画面には触らない。契約の SSOT は
//! `fandhe_device::bluetooth_classic`（0 件は成功・失敗の区別・部分結果の破棄・`Cancelled` を返さない）。
//!
//! # 流れ
//!
//! 1. 電源・対応の判定: WinRT `BluetoothAdapter`（`IsClassicSupported`）と `Radio.State`
//!    （[`precheck_outcome`]）。アダプタなし・Classic 非対応は `Unsupported`、オフは `Disabled`
//! 2. 一覧: Win32 `BluetoothFindFirstDevice` を `fIssueInquiry = FALSE` で呼び、ペアリング済み
//!    （認証済みまたは記憶済み）だけを返す
//! 3. 検索: 同じ API を `fIssueInquiry = TRUE`・`cTimeoutMultiplier` ≦ 9（1 単位 1.28 秒）で呼ぶ。
//!    電波強度は取れないので `None`（番兵にしない）
//!
//! # 期限と直列経路（契約表 2・3・11）
//!
//! 同期の inquiry は途中で止められない。検索の受付は専用ワーカー（スーパーバイザ 1 本、`mpsc`）へ
//! 集め、開始・置換・停止・完了・期限を [`Scheduler`] の状態機械で順に処理する。inquiry を走らせる
//! スレッドは高々 1 本で、走行中に再要求が来たら旧 inquiry の完了を待ってから、残り時間で走らせる。
//! 期限の起点は `present` の入口で取った `Instant`（単調時計）で、窓に `WINDOWS_DEADLINE_GRACE_MS` を
//! 足した時点で `Internal` を確定する。遅れて届いた結果や古い世代の結果は捨てる。inquiry スレッドは
//! 結果を [`Scheduler`] へ返すだけで、`OBSERVED` へ直接は書かない（期限と世代の判定を 1 箇所に集める）。
//!
//! 一覧の読みは検索の直列経路の外に置く。検索側と共有する状態を持たず、Rust 側の要求状態を触らない
//! ため。WinRT の待機は短命スレッドで行うが、呼び出し元（UI スレッドの `drive_paired_sync`）は結果を
//! 同期で待つ。待ちは [`PAIRED_TIMEOUT_MS`]（2 秒）で打ち切って `Internal` にする（0 件の成功へ畳まない）。
//! 打ち切りまでの最大 2 秒は UI が止まる。打ち切った後も読みスレッドが終わるまでは再試行も `Internal`
//! になる（守らない範囲）。
//!
//! # 守る範囲と守らない範囲
//!
//! `fandhe_device::bluetooth_classic` のモジュール doc を参照。加えて、止められない inquiry は
//! プロセス終了で OS が片付けるため、停止時に同期して待たない。
//!
//! # 既知の天井（【推測】は Windows 実機確認 #650 で確かめる）
//!
//! - 【推測】Bluetooth オフのとき WinRT の `Radio.State` が確実に `Off` を返すか
//! - 【推測】LE だけのペアリング機器が Win32 の列挙に混ざらないか
//! - 【推測】`fRemembered` だけの機器をペアリング済みとして扱ってよいか
//! - 【推測】inquiry の最中に一覧読みがブロックされないか（されても上限で `Internal` に倒れる）
//! - 【推測】パッケージなしの exe で同意が要らないか
//!
//! 機器名・アドレス・HRESULT をログ・エラー文言へ出さない（security.md A09）。
//! 型検査は `x86_64-pc-windows-gnu` のみで、実機確認は #650。
//!
//! 純関数と [`Scheduler`] は OS 非依存で、ホストの `cargo test` でも検証する。`windows` クレートを
//! 使う部分は `cfg(target_os = "windows")` の `os` に閉じる。

use std::time::{Duration, Instant};

use fandhe_device::bluetooth_classic::{
    clamp_name, major_from_cod, ClassicDevice, DeviceKey, DISCOVERY_WINDOW_MS,
    WINDOWS_DEADLINE_GRACE_MS,
};
use fandhe_device::DeviceError;

/// 1 回の列挙で受け取る機器数の上限（応答の洪水でメモリが膨らむのを防ぐ）。
const MAX_ENUMERATED: usize = 256;
/// `cTimeoutMultiplier` 1 単位の長さ（ミリ秒。公式: 1.28 秒）。
const MULTIPLIER_UNIT_MS: u64 = 1_280;
/// ペアリング済み一覧の読みを UI 側が待つ上限（ミリ秒）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const PAIRED_TIMEOUT_MS: u64 = 2_000;

/// `ERROR_NO_MORE_ITEMS` の HRESULT（列挙の正常な終端）。
const HR_NO_MORE_ITEMS: i32 = 0x8007_0103_u32 as i32;
/// `E_ACCESSDENIED`。
const HR_ACCESS_DENIED: i32 = 0x8007_0005_u32 as i32;

type Found = Vec<(DeviceKey, ClassicDevice)>;

/// 48 bit アドレスを `AA:BB:CC:DD:EE:FF` のキーにする。0 は `None`（機器を捨てる）。
/// キーはログにも画面にも出さない。
fn format_key(addr: u64) -> Option<DeviceKey> {
    let addr = addr & 0xFFFF_FFFF_FFFF;
    if addr == 0 {
        return None;
    }
    let s = (0..6)
        .rev()
        .map(|i| format!("{:02X}", (addr >> (i * 8)) & 0xFF))
        .collect::<Vec<_>>()
        .join(":");
    DeviceKey::new(s)
}

/// 固定長 UTF-16 バッファの機器名を読む。NUL まで（無ければ配列長まで）。
/// UTF-16 として不正なら `None`（lossy にしない）。
fn name_from_utf16(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let s = String::from_utf16(&units[..end]).ok()?;
    clamp_name(s)
}

/// OS の生値を契約の機器 1 件へ写す。アドレスが 0 の機器は捨てる。電波強度は取れないので `None`。
fn device_from_parts(
    addr: u64,
    cod: u32,
    name_units: &[u16],
    authenticated: bool,
    remembered: bool,
) -> Option<(DeviceKey, ClassicDevice)> {
    let key = format_key(addr)?;
    Some((
        key,
        ClassicDevice {
            name: name_from_utf16(name_units),
            kind: major_from_cod(cod),
            paired: authenticated || remembered,
            rssi: None,
        },
    ))
}

/// 残り時間から `cTimeoutMultiplier` を決める。1.28 秒未満は `None`、上限は窓に収まる値（9）。
fn timeout_multiplier(remaining: Duration) -> Option<u8> {
    let units = remaining.as_millis() / u128::from(MULTIPLIER_UNIT_MS);
    if units == 0 {
        return None;
    }
    let max = u128::from(DISCOVERY_WINDOW_MS / MULTIPLIER_UNIT_MS);
    u8::try_from(units.min(max)).ok()
}

/// Win32 列挙関数の失敗の写像結果。
#[derive(Debug)]
enum FindOutcome {
    /// 列挙の正常な終端（0 件の成功、または最後の機器の後）。
    End,
    /// 失敗（部分結果は捨てる）。
    Fail(DeviceError),
}

/// 列挙関数（`BluetoothFindFirstDevice` / `BluetoothFindNextDevice`）の HRESULT を契約へ写す。
/// 全呼び出し箇所でこの 1 関数を使う（契約表 5）。
fn classify_find_error(code: i32) -> FindOutcome {
    match code {
        HR_NO_MORE_ITEMS => FindOutcome::End,
        HR_ACCESS_DENIED => FindOutcome::Fail(DeviceError::PermissionDenied("bluetooth_classic")),
        _ => FindOutcome::Fail(DeviceError::Internal("bluetooth_classic enumerate")),
    }
}

/// ラジオ状態（WinRT `RadioState` を写したもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum RadioKind {
    On,
    Off,
    Disabled,
    Unknown,
}

/// アダプタ判定の入力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum AdapterProbe {
    /// 既定アダプタが無い。
    NoAdapter,
    /// Classic（BR/EDR）に非対応。
    ClassicUnsupported,
    /// 対応あり。ラジオの状態。
    Radio(RadioKind),
}

/// アダプタ判定を契約へ写す。`Unknown` のラジオは列挙の実結果に任せる（空の成功は返さない）。
fn precheck_outcome(probe: AdapterProbe) -> Result<(), DeviceError> {
    match probe {
        AdapterProbe::NoAdapter | AdapterProbe::ClassicUnsupported => {
            Err(DeviceError::Unsupported("bluetooth_classic"))
        }
        AdapterProbe::Radio(RadioKind::Off | RadioKind::Disabled) => {
            Err(DeviceError::Disabled("bluetooth_classic"))
        }
        AdapterProbe::Radio(RadioKind::On | RadioKind::Unknown) => Ok(()),
    }
}

/// スケジューラが実行側へ依頼する動作。
#[derive(Debug)]
enum Action {
    /// inquiry スレッドを起動する（`multiplier` は `cTimeoutMultiplier`）。
    Spawn { request_id: u64, multiplier: u8 },
    /// 要求を確定する。`Ok` は観測バッファへ足して完了、`Err` は部分結果を捨てて失敗で完了。
    Finish {
        request_id: u64,
        result: Result<Found, DeviceError>,
    },
}

#[derive(Debug)]
struct Req {
    id: u64,
    /// 窓の終わり（`started_at + duration`）。inquiry の残り時間の基準。
    window_end: Instant,
    /// 失敗確定の期限（窓 + 猶予）。
    deadline: Instant,
    /// inquiry の起動待ち（走行中の旧 inquiry の完了を待っている）。
    waiting: bool,
}

/// 検索の状態機械（スーパーバイザが 1 本で回す。時刻は引数で受け、OS を触らない）。
///
/// 守ること: 同じ要求に `Finish` は高々 1 回、走行中の inquiry は高々 1 本、期限後・古い世代・
/// 停止後の結果は成功として届けない。
#[derive(Debug, Default)]
struct Scheduler {
    current: Option<Req>,
    inflight: Option<u64>,
}

impl Scheduler {
    /// 次の期限までの待ち時間。要求が無ければ `None`（メッセージ待ちだけ）。
    fn next_timeout(&self, now: Instant) -> Option<Duration> {
        self.current
            .as_ref()
            .map(|r| r.deadline.saturating_duration_since(now))
    }

    /// 期限を過ぎた要求を `Internal` で確定する。`inflight` は止められないので残す。
    fn expire(&mut self, now: Instant, out: &mut Vec<Action>) {
        if self.current.as_ref().is_some_and(|r| now >= r.deadline) {
            if let Some(r) = self.current.take() {
                out.push(Action::Finish {
                    request_id: r.id,
                    result: Err(DeviceError::Internal("bluetooth_classic deadline")),
                });
            }
        }
    }

    /// 起動待ちの要求があり inquiry が空いていれば、残り時間で起動する。残りが足りなければ `Internal`。
    fn try_spawn(&mut self, now: Instant, out: &mut Vec<Action>) {
        if self.inflight.is_some() {
            return;
        }
        let Some(req) = self.current.as_mut().filter(|r| r.waiting) else {
            return;
        };
        let remaining = req.window_end.saturating_duration_since(now);
        match timeout_multiplier(remaining) {
            Some(multiplier) => {
                req.waiting = false;
                self.inflight = Some(req.id);
                out.push(Action::Spawn {
                    request_id: req.id,
                    multiplier,
                });
            }
            None => {
                let id = req.id;
                self.current = None;
                out.push(Action::Finish {
                    request_id: id,
                    result: Err(DeviceError::Internal("bluetooth_classic window")),
                });
            }
        }
    }

    /// 検索の開始。前の要求は捨てる（Rust 側で置換済みで、旧要求の完了は照合で落ちる）。
    fn on_start(
        &mut self,
        id: u64,
        started_at: Instant,
        duration: Duration,
        now: Instant,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        self.expire(now, &mut out);
        let window_end = started_at + duration;
        self.current = Some(Req {
            id,
            window_end,
            deadline: window_end + Duration::from_millis(WINDOWS_DEADLINE_GRACE_MS),
            waiting: true,
        });
        self.try_spawn(now, &mut out);
        out
    }

    /// inquiry の完了。現在の要求かつ期限内のときだけ結果を確定する（確定してから届ける）。
    fn on_done(
        &mut self,
        id: u64,
        result: Result<Found, DeviceError>,
        now: Instant,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        self.expire(now, &mut out);
        if self.inflight == Some(id) {
            self.inflight = None;
        }
        if self
            .current
            .as_ref()
            .is_some_and(|r| r.id == id && !r.waiting)
        {
            self.current = None;
            out.push(Action::Finish {
                request_id: id,
                result,
            });
        }
        self.try_spawn(now, &mut out);
        out
    }

    /// 待ちの期限切れ。
    fn on_timeout(&mut self, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        self.expire(now, &mut out);
        out
    }

    /// 終了時の停止。走行中の inquiry は止められないので放置し、完了は捨てる。
    fn on_stop(&mut self) {
        self.current = None;
    }
}

#[cfg(target_os = "windows")]
pub(crate) use os::{implementation, present, stop_shared};

/// `windows` クレート・Win32 に触れる部分（Windows ターゲット専用）。
#[cfg(target_os = "windows")]
mod os {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use fandhe_device::bluetooth_classic::{normalize_devices, BluetoothClassic, ClassicDevice};
    use fandhe_device::DeviceError;
    use windows::Devices::Bluetooth::BluetoothAdapter;
    use windows::Devices::Radios::RadioState;
    use windows::Win32::Devices::Bluetooth::{
        BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice,
        BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS, HBLUETOOTH_DEVICE_FIND,
    };
    use windows::Win32::Foundation::HANDLE;

    use super::{
        classify_find_error, device_from_parts, precheck_outcome, Action, AdapterProbe,
        FindOutcome, Found, RadioKind, Scheduler, HR_ACCESS_DENIED, MAX_ENUMERATED,
        PAIRED_TIMEOUT_MS,
    };
    use crate::device::bluetooth_classic::{finish, OBSERVED};

    enum Msg {
        Start {
            request_id: u64,
            started_at: Instant,
            duration: Duration,
        },
        Stop,
        Done {
            request_id: u64,
            result: Result<Found, DeviceError>,
        },
    }

    /// スーパーバイザへの送信口（遅延起動。ロックは送信の間だけ持つ）。
    static WORKER: Mutex<Option<mpsc::Sender<Msg>>> = Mutex::new(None);
    /// 一覧読みスレッドが走行中か（無制限にスレッドが増えるのを防ぐ）。
    static PAIRED_BUSY: AtomicBool = AtomicBool::new(false);

    fn lock_worker() -> std::sync::MutexGuard<'static, Option<mpsc::Sender<Msg>>> {
        WORKER.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `windows::core::Error` → 契約のエラー（アクセス拒否だけ `PermissionDenied`）。
    fn map_win_error(e: &windows::core::Error, what: &'static str) -> DeviceError {
        if e.code().0 == HR_ACCESS_DENIED {
            DeviceError::PermissionDenied("bluetooth_classic")
        } else {
            DeviceError::Internal(what)
        }
    }

    /// WinRT でアダプタの有無・Classic 対応・ラジオ状態を調べて契約へ写す。
    fn precheck() -> Result<(), DeviceError> {
        // アダプタ無し（null）は windows クレートが code 0 の `Err` にする。
        let adapter = match BluetoothAdapter::GetDefaultAsync()
            .map_err(|e| map_win_error(&e, "bluetooth_classic init"))?
            .get()
        {
            Ok(a) => a,
            Err(e) if e.code().0 == 0 => return precheck_outcome(AdapterProbe::NoAdapter),
            Err(e) => return Err(map_win_error(&e, "bluetooth_classic init")),
        };
        let supported = adapter
            .IsClassicSupported()
            .map_err(|e| map_win_error(&e, "bluetooth_classic init"))?;
        if !supported {
            return precheck_outcome(AdapterProbe::ClassicUnsupported);
        }
        let radio = adapter
            .GetRadioAsync()
            .map_err(|e| map_win_error(&e, "bluetooth_classic radio"))?
            .get()
            .map_err(|e| map_win_error(&e, "bluetooth_classic radio"))?;
        let state = radio
            .State()
            .map_err(|e| map_win_error(&e, "bluetooth_classic radio"))?;
        let kind = if state == RadioState::On {
            RadioKind::On
        } else if state == RadioState::Off {
            RadioKind::Off
        } else if state == RadioState::Disabled {
            RadioKind::Disabled
        } else {
            RadioKind::Unknown
        };
        precheck_outcome(AdapterProbe::Radio(kind))
    }

    /// 検索ハンドル（`Drop` で必ず閉じる。契約表 9）。
    struct FindHandle(HBLUETOOTH_DEVICE_FIND);

    impl Drop for FindHandle {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: `FindHandle` は `BluetoothFindFirstDevice` が返した有効なハンドルをちょうど 1 つ
            // 持ち、ここで 1 回だけ閉じる（コピーを外へ出さない）。
            let _ = unsafe { BluetoothFindDeviceClose(self.0) };
        }
    }

    fn new_info() -> BLUETOOTH_DEVICE_INFO {
        BLUETOOTH_DEVICE_INFO {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
            ..Default::default()
        }
    }

    /// 検索の種類。
    #[derive(Clone, Copy)]
    enum Mode {
        /// inquiry なし（記憶済み・認証済みのみ）。
        Paired,
        /// inquiry あり（`cTimeoutMultiplier` を渡す）。
        Inquiry(u8),
    }

    /// `BLUETOOTH_DEVICE_INFO` 1 件を契約の値へ写す。
    #[allow(unsafe_code)]
    fn device_from_info(info: &BLUETOOTH_DEVICE_INFO) -> Option<(super::DeviceKey, ClassicDevice)> {
        // SAFETY: `BLUETOOTH_ADDRESS` の union は `u64` と `[u8; 6]` で、どのビット列も有効。
        let addr = unsafe { info.Address.Anonymous.ullLong };
        device_from_parts(
            addr,
            info.ulClassofDevice,
            &info.szName,
            info.fAuthenticated.as_bool(),
            info.fRemembered.as_bool(),
        )
    }

    /// Win32 の列挙。失敗は部分結果を捨てて `Err`。
    #[allow(unsafe_code)]
    fn enumerate(mode: Mode) -> Result<Found, DeviceError> {
        let (inquiry, multiplier) = match mode {
            Mode::Paired => (false, 0),
            Mode::Inquiry(m) => (true, m),
        };
        let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnAuthenticated: true.into(),
            fReturnRemembered: true.into(),
            fReturnUnknown: inquiry.into(),
            fReturnConnected: inquiry.into(),
            fIssueInquiry: inquiry.into(),
            cTimeoutMultiplier: multiplier,
            hRadio: HANDLE::default(),
        };
        let mut info = new_info();
        // SAFETY: `params` と `info` は呼び出しの間有効なスタック上の値で、`dwSize` を設定済み。
        // OS はポインタを保持しない。
        let first = unsafe { BluetoothFindFirstDevice(&params, &mut info) };
        let handle = match first {
            Ok(h) => FindHandle(h),
            Err(e) => {
                return match classify_find_error(e.code().0) {
                    FindOutcome::End => Ok(Vec::new()),
                    FindOutcome::Fail(err) => Err(err),
                }
            }
        };
        let mut out: Found = Vec::new();
        loop {
            let paired_only = matches!(mode, Mode::Paired);
            if let Some((key, dev)) = device_from_info(&info) {
                // 一覧は防御的にペアリング済みだけに絞る。
                if (!paired_only || dev.paired) && out.len() < MAX_ENUMERATED {
                    out.push((key, dev));
                }
            }
            if out.len() >= MAX_ENUMERATED {
                break;
            }
            info = new_info();
            // SAFETY: `handle.0` は有効な検索ハンドル、`info` は `dwSize` 設定済みの有効なスタック値。
            match unsafe { BluetoothFindNextDevice(handle.0, &mut info) } {
                Ok(()) => {}
                Err(e) => match classify_find_error(e.code().0) {
                    FindOutcome::End => break,
                    FindOutcome::Fail(err) => return Err(err),
                },
            }
        }
        Ok(out)
    }

    /// `BluetoothClassic` の Windows 実装。
    struct WindowsBluetoothClassic;

    impl BluetoothClassic for WindowsBluetoothClassic {
        fn paired(&self) -> Result<Vec<ClassicDevice>, DeviceError> {
            paired_list()
        }

        /// 検索は pull 型の `present` 経路でだけ駆動する。成功値へは倒さない。
        fn discover(&self) -> Result<Vec<ClassicDevice>, DeviceError> {
            Err(DeviceError::Unsupported("bluetooth_classic"))
        }
    }

    pub(crate) fn implementation() -> Box<dyn BluetoothClassic> {
        Box::new(WindowsBluetoothClassic)
    }

    /// 一覧読みスレッドの走行フラグを解除する RAII。
    struct BusyGuard;
    impl Drop for BusyGuard {
        fn drop(&mut self) {
            PAIRED_BUSY.store(false, Ordering::Release);
        }
    }

    /// ペアリング済み一覧を短命スレッドで読み、UI 側は上限付きで待つ（UI スレッドで WinRT を待たない）。
    fn paired_list() -> Result<Vec<ClassicDevice>, DeviceError> {
        let started = Instant::now();
        if PAIRED_BUSY.swap(true, Ordering::AcqRel) {
            return Err(DeviceError::Internal("bluetooth_classic busy"));
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().spawn(move || {
            let _busy = BusyGuard;
            let result = precheck().and_then(|()| enumerate(Mode::Paired));
            let _ = tx.send(result);
        });
        if spawned.is_err() {
            PAIRED_BUSY.store(false, Ordering::Release);
            return Err(DeviceError::Internal("bluetooth_classic paired"));
        }
        let limit = Duration::from_millis(PAIRED_TIMEOUT_MS).saturating_sub(started.elapsed());
        match rx.recv_timeout(limit) {
            Ok(result) => result.map(normalize_devices),
            Err(RecvTimeoutError::Timeout) => {
                Err(DeviceError::Internal("bluetooth_classic paired timeout"))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(DeviceError::Internal("bluetooth_classic paired"))
            }
        }
    }

    /// 検索の開始を依頼する。期限の起点は入口の `Instant`。スーパーバイザへ送って即座に戻る。
    pub(crate) fn present(request_id: u64, duration: Duration) -> Result<(), DeviceError> {
        let started_at = Instant::now();
        let mut guard = lock_worker();
        if guard.is_none() {
            let (tx, rx) = mpsc::channel();
            let own_tx = tx.clone();
            let spawned = std::thread::Builder::new()
                .name("fandhe-bt-classic".into())
                .spawn(move || supervisor(rx, own_tx));
            if spawned.is_err() {
                return Err(DeviceError::Internal("bluetooth_classic present"));
            }
            *guard = Some(tx);
        }
        let sent = guard.as_ref().is_some_and(|tx| {
            tx.send(Msg::Start {
                request_id,
                started_at,
                duration,
            })
            .is_ok()
        });
        if !sent {
            *guard = None;
            return Err(DeviceError::Internal("bluetooth_classic present"));
        }
        Ok(())
    }

    /// 終了時の停止。起動済みのときだけ `Stop` を送る（終了時に新しく起動しない）。
    pub(crate) fn stop_shared() {
        if let Some(tx) = lock_worker().as_ref() {
            let _ = tx.send(Msg::Stop);
        }
    }

    /// 検索の直列経路。スーパーバイザ 1 本が [`Scheduler`] を回し、結果を `OBSERVED` / `finish` へ流す。
    /// プロセスの寿命と同じだけ生きる（自分の送信口を持つため受信側は切断されない）。
    fn supervisor(rx: mpsc::Receiver<Msg>, tx: mpsc::Sender<Msg>) {
        let mut sched = Scheduler::default();
        loop {
            let msg = match sched.next_timeout(Instant::now()) {
                Some(t) => rx.recv_timeout(t),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let now = Instant::now();
            let actions = match msg {
                Ok(Msg::Start {
                    request_id,
                    started_at,
                    duration,
                }) => sched.on_start(request_id, started_at, duration, now),
                Ok(Msg::Stop) => {
                    sched.on_stop();
                    Vec::new()
                }
                Ok(Msg::Done { request_id, result }) => sched.on_done(request_id, result, now),
                Err(RecvTimeoutError::Timeout) => sched.on_timeout(now),
                Err(RecvTimeoutError::Disconnected) => return,
            };
            run_actions(&mut sched, actions, &tx);
        }
    }

    fn run_actions(sched: &mut Scheduler, actions: Vec<Action>, tx: &mpsc::Sender<Msg>) {
        let mut queue: VecDeque<Action> = actions.into();
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Spawn {
                    request_id,
                    multiplier,
                } => {
                    let done_tx = tx.clone();
                    let spawned = std::thread::Builder::new()
                        .name("fandhe-bt-inquiry".into())
                        .spawn(move || {
                            let result =
                                precheck().and_then(|()| enumerate(Mode::Inquiry(multiplier)));
                            let _ = done_tx.send(Msg::Done { request_id, result });
                        });
                    if spawned.is_err() {
                        queue.extend(sched.on_done(
                            request_id,
                            Err(DeviceError::Internal("bluetooth_classic spawn")),
                            Instant::now(),
                        ));
                    }
                }
                Action::Finish { request_id, result } => match result {
                    Ok(list) => {
                        for (key, dev) in list {
                            OBSERVED.observe(request_id, key, dev);
                        }
                        finish(request_id, Ok(()));
                    }
                    Err(e) => finish(request_id, Err(e)),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn is_internal<T>(r: &Result<T, DeviceError>) -> bool {
        matches!(r, Err(DeviceError::Internal(_)))
    }

    #[test]
    fn format_key_rejects_zero_and_ignores_upper_bits() {
        assert!(format_key(0).is_none());
        assert!(format_key(0xFFFF_0000_0000_0000).is_none());
        let a = format_key(0x0000_AABB_CCDD_EEFF).unwrap();
        let b = format_key(0xFFFF_AABB_CCDD_EEFF).unwrap();
        assert_eq!(a, b);
        assert_eq!(format!("{a:?}"), "DeviceKey(<redacted>)");
    }

    #[test]
    fn name_from_utf16_handles_nul_unpaired_and_empty() {
        assert_eq!(name_from_utf16(&utf16("abc")).as_deref(), Some("abc"));
        let mut with_nul = utf16("abc");
        with_nul.extend([0, b'x'.into()]);
        assert_eq!(name_from_utf16(&with_nul).as_deref(), Some("abc"));
        assert_eq!(name_from_utf16(&[0xD800, 0]), None);
        assert_eq!(name_from_utf16(&[0xD800]), None);
        assert_eq!(name_from_utf16(&[0, 0]), None);
        assert_eq!(name_from_utf16(&[]), None);
        assert_eq!(name_from_utf16(&utf16("\u{1}\t")), None);
        let long = utf16(&"あ".repeat(100));
        assert_eq!(name_from_utf16(&long).unwrap().chars().count(), 82);
    }

    #[test]
    fn device_from_parts_maps_paired_kind_and_rssi() {
        let name = utf16("Dev");
        for (auth, rem, paired) in [
            (true, false, true),
            (false, true, true),
            (true, true, true),
            (false, false, false),
        ] {
            let (_, d) = device_from_parts(1, 0x0000_0204, &name, auth, rem).unwrap();
            assert_eq!(d.paired, paired);
            assert_eq!(d.rssi, None);
            assert_eq!(d.kind, fandhe_device::bluetooth_classic::DeviceKind::Phone);
            assert_eq!(d.name.as_deref(), Some("Dev"));
        }
        assert!(device_from_parts(0, 0, &name, true, true).is_none());
    }

    #[test]
    fn timeout_multiplier_bounds() {
        assert_eq!(timeout_multiplier(Duration::from_millis(12_000)), Some(9));
        assert_eq!(timeout_multiplier(Duration::from_millis(1_279)), None);
        assert_eq!(timeout_multiplier(Duration::from_millis(1_280)), Some(1));
        assert_eq!(timeout_multiplier(Duration::from_millis(2_559)), Some(1));
        assert_eq!(
            timeout_multiplier(Duration::from_secs(u64::MAX / 2000)),
            Some(9)
        );
        assert_eq!(timeout_multiplier(Duration::ZERO), None);
    }

    #[test]
    fn classify_find_error_table() {
        assert!(matches!(
            classify_find_error(HR_NO_MORE_ITEMS),
            FindOutcome::End
        ));
        assert!(matches!(
            classify_find_error(HR_ACCESS_DENIED),
            FindOutcome::Fail(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(
            classify_find_error(-1),
            FindOutcome::Fail(DeviceError::Internal(_))
        ));
        assert!(matches!(
            classify_find_error(0),
            FindOutcome::Fail(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn precheck_outcome_never_returns_empty_success_for_failures() {
        assert!(matches!(
            precheck_outcome(AdapterProbe::NoAdapter),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            precheck_outcome(AdapterProbe::ClassicUnsupported),
            Err(DeviceError::Unsupported(_))
        ));
        for k in [RadioKind::Off, RadioKind::Disabled] {
            assert!(matches!(
                precheck_outcome(AdapterProbe::Radio(k)),
                Err(DeviceError::Disabled(_))
            ));
        }
        assert!(precheck_outcome(AdapterProbe::Radio(RadioKind::On)).is_ok());
        assert!(precheck_outcome(AdapterProbe::Radio(RadioKind::Unknown)).is_ok());
    }

    const WIN: Duration = Duration::from_millis(DISCOVERY_WINDOW_MS);

    fn grace() -> Duration {
        Duration::from_millis(WINDOWS_DEADLINE_GRACE_MS)
    }

    fn finishes(actions: &[Action]) -> Vec<(u64, bool)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Finish { request_id, result } => Some((*request_id, result.is_ok())),
                _ => None,
            })
            .collect()
    }

    fn spawns(actions: &[Action]) -> Vec<(u64, u8)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Spawn {
                    request_id,
                    multiplier,
                } => Some((*request_id, *multiplier)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn scheduler_normal_flow_finishes_once() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        let a = s.on_start(1, t0, WIN, t0);
        assert_eq!(spawns(&a), vec![(1, 9)]);
        let a = s.on_done(1, Ok(Vec::new()), t0 + Duration::from_secs(11));
        assert_eq!(finishes(&a), vec![(1, true)]);
        // 同じ要求の二重完了は出ない。
        let a = s.on_done(1, Ok(Vec::new()), t0 + Duration::from_secs(11));
        assert!(finishes(&a).is_empty());
        assert!(s.next_timeout(t0).is_none());
    }

    #[test]
    fn scheduler_drops_late_success_and_times_out_once() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.on_start(1, t0, WIN, t0);
        let late = t0 + WIN + grace();
        let a = s.on_timeout(late);
        assert_eq!(finishes(&a), vec![(1, false)]);
        assert!(is_internal(match &a[0] {
            Action::Finish { result, .. } => result,
            _ => unreachable!(),
        }));
        assert!(finishes(&s.on_timeout(late)).is_empty());
        // 遅れて届く成功は捨てる。
        let a = s.on_done(1, Ok(Vec::new()), late + Duration::from_secs(1));
        assert!(finishes(&a).is_empty());
    }

    #[test]
    fn scheduler_done_after_deadline_is_not_success() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.on_start(1, t0, WIN, t0);
        let late = t0 + WIN + grace();
        let a = s.on_done(1, Ok(Vec::new()), late);
        assert_eq!(finishes(&a), vec![(1, false)]);
    }

    #[test]
    fn scheduler_drops_stale_generation_done() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.on_start(1, t0, WIN, t0);
        // inquiry 1 の走行中に要求 2 が来る: 起動は保留。
        let a = s.on_start(2, t0, WIN, t0 + Duration::from_secs(3));
        assert!(spawns(&a).is_empty());
        // 旧 inquiry の完了は捨て、要求 2 を残り時間で起動する。
        let a = s.on_done(1, Ok(Vec::new()), t0 + Duration::from_secs(5));
        assert!(finishes(&a).is_empty());
        // 残り 7 秒 = 5 単位。
        assert_eq!(spawns(&a), vec![(2, 5)]);
    }

    #[test]
    fn scheduler_pending_request_fails_when_window_exhausted() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.on_start(1, t0, WIN, t0);
        s.on_start(2, t0, WIN, t0 + Duration::from_secs(1));
        let a = s.on_done(
            1,
            Ok(Vec::new()),
            t0 + Duration::from_millis(11_000) + Duration::from_millis(500),
        );
        assert!(spawns(&a).is_empty());
        assert_eq!(finishes(&a), vec![(2, false)]);
    }

    #[test]
    fn scheduler_stop_discards_done_and_keeps_single_inflight() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.on_start(1, t0, WIN, t0);
        s.on_stop();
        assert!(s.next_timeout(t0).is_none());
        // 停止後に再要求しても、旧 inquiry が走行中のうちは起動しない（高々 1 本）。
        let a = s.on_start(2, t0, WIN, t0 + Duration::from_secs(1));
        assert!(spawns(&a).is_empty());
        let a = s.on_done(1, Ok(Vec::new()), t0 + Duration::from_secs(2));
        assert!(finishes(&a).is_empty());
        assert_eq!(spawns(&a).len(), 1);
    }

    #[test]
    fn scheduler_done_err_finishes_as_err() {
        let t0 = Instant::now();
        let mut s = Scheduler::default();
        s.on_start(1, t0, WIN, t0);
        let a = s.on_done(
            1,
            Err(DeviceError::Disabled("bluetooth_classic")),
            t0 + Duration::from_secs(1),
        );
        assert_eq!(finishes(&a), vec![(1, false)]);
    }
}
