//! Windows 向け BLE の周辺機器役実装（Issue #1101）。`device/ble_peripheral.rs` の Windows 経路
//! （`platform_ble_peripheral`）から呼ばれ、WinRT `GattServiceProvider` で固定のサービス UUID 1 つ・
//! 読み取り専用の特性 1 つを公開する。契約と守る範囲は `fandhe_device::ble_peripheral` のモジュール
//! doc、状態の写像（`BluetoothError` / アドバタイズ status → 事象）はそこの純関数が SSOT。
//!
//! # 流れ（専用ワーカーの待たない状態機械）
//!
//! 開始手順の各段は WinRT の非同期操作で、完了を `SetCompleted` から [`Cmd`] としてキューへ積む
//! （`windows_pedometer.rs` と同じ形）。ワーカーが待つのは `rx.recv()` だけで、ワーカー上で
//! `.get()` は使わない。
//!
//! 1. `BluetoothAdapter::GetDefaultAsync`: null（code 0）と `IsPeripheralRoleSupported() ==
//!    false` は `Unsupported`、`E_ACCESSDENIED` は `PermissionDenied`、その他は `Failed`
//! 2. `GetRadioAsync`（ラジオ状態の補正用。取れなくても開始は止めない）
//! 3. `GattServiceProvider::CreateAsync` → `CreateCharacteristicAsync`（Read・Plain）。
//!    結果の `BluetoothError` は `end_reason_from_windows_bluetooth_error` で写す
//! 4. **公開の前に** `ReadRequested` / `AdvertisementStatusChanged` を登録する（ハンドラが無いと
//!    読み取りは `UnlikelyError` で即完了するため）
//! 5. `StartAdvertisingWithParameters(IsConnectable, IsDiscoverable)`。「公開中」は開始呼び出しの
//!    成功ではなく `AdvertisementStatusChanged` の Started で確定する（嘘の成功を作らない）
//!
//! Windows ではローカル名を指定できない（`IsDiscoverable` で OS が端末名を載せる）。中断
//! （`Interrupted`）は出さない。
//!
//! # 並行性（契約表 11）
//!
//! 開始・停止・各段の完了・OS の状態通知・読み取りは直列キュー（[`Cmd`]）1 本に集約し、状態
//! （[`State`]）はワーカーのローカル変数にだけ置く（ロックを持たない）。開始手順の途中の `stop` は
//! 保留中の操作を `Cancel` して `Stopped` を即返す（OS を待たない）。取り消しと競合して後から届いた
//! 完了は `session` で照合して捨てる（公開もハンドラ登録もまだなので、参照を捨てれば解放になる）。
//! 解放（`Remove*` / `StopAdvertising`）は戻り値を 1 件ずつ見て、失敗したら [`Running`] を
//! 未解放のまま持ち続けて次の Start / Stop で再試行する。公開の確定（Started）より前に応答した
//! 読み取りは [`Running::early_reads`] に溜め、確定後にまとめて通知する。
//!
//! 中心役の識別子・読み取りの中身・HRESULT はログにも画面にも出さない（security.md A09）。
//! `unsafe` と `RoInitialize` は使わない。
//!
//! # 既知の天井
//!
//! unpackaged の exe で GATT サーバーが使えるかは未確認（実機確認は #776）。型検査は
//! `x86_64-pc-windows-gnu` のみ。

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};

use windows::core::{Interface, Ref, RuntimeType, GUID, HRESULT};
use windows::Devices::Bluetooth::BluetoothAdapter;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattLocalCharacteristic, GattLocalCharacteristicParameters,
    GattLocalCharacteristicResult, GattProtectionLevel, GattProtocolError,
    GattReadRequestedEventArgs, GattServiceProvider,
    GattServiceProviderAdvertisementStatusChangedEventArgs,
    GattServiceProviderAdvertisingParameters, GattServiceProviderResult,
};
use windows::Devices::Radios::{Radio, RadioState};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::DataWriter;
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus, IAsyncInfo, IAsyncOperation};

use fandhe_device::ble_peripheral::{
    end_reason_from_windows_bluetooth_error, event_from_windows_advertisement_status,
    BlePeripheral, EndReason, OnEvent, PeripheralEvent, StartOutcome, StopOutcome,
    CHARACTERISTIC_UUID, READ_VALUE, SERVICE_UUID,
};
use fandhe_device::DeviceError;

use super::ble_peripheral::SessionSlot;

/// `E_ACCESSDENIED`。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
/// `E_FAIL`（完了が成功以外で、OS が理由を返さないときの代用）。
const E_FAIL: i32 = 0x8000_4005_u32 as i32;

/// 現在の公開の session と通知先（OS のハンドラとワーカーが共有する受け口）。
static SESSION: SessionSlot = SessionSlot::new();

/// ワーカーへの命令。開始・停止・各段の完了・OS の通知はすべてここを順に通る。
enum Cmd {
    Start(u64),
    Stop(Sender<Result<StopOutcome, DeviceError>>),
    Adapter(u64, windows::core::Result<BluetoothAdapter>),
    Radio(u64, windows::core::Result<Radio>),
    Provider(u64, windows::core::Result<GattServiceProviderResult>),
    Char(u64, windows::core::Result<GattLocalCharacteristicResult>),
    Status {
        session: u64,
        status: i32,
        error: i32,
    },
    /// offset 0 の読み取り成功 1 件。
    Read(u64),
}

/// 開始手順の途中で確保済みの部品。公開もハンドラ登録もまだなので、捨てれば解放になる。
#[derive(Default)]
struct Held {
    radio: Option<Radio>,
    provider: Option<GattServiceProvider>,
}

enum State {
    Idle,
    /// 開始手順の途中。`op` は保留中の非同期操作（`stop` / 再 Start が `Cancel` する先）。
    Opening {
        session: u64,
        op: IAsyncInfo,
        held: Held,
    },
    Running(Running),
}

/// 公開済み（または解放待ち）の状態。解放に失敗した分は残り、次の機会に再試行する。
struct Running {
    session: u64,
    provider: GattServiceProvider,
    radio: Option<Radio>,
    // 解放済みの項目は None / true にして、再試行で二重に外さない。
    read_token: Option<i64>,
    status_token: Option<i64>,
    char: GattLocalCharacteristic,
    adv_stopped: bool,
    /// Started が届いたか。
    advertising: bool,
    /// Started より前に応答した読み取りの件数（確定後にまとめて通知する）。
    early_reads: u64,
}

impl Running {
    /// 全項目を解放する。1 件でも失敗したら `Err` で、残りは次回に再試行する。
    fn release(&mut self) -> Result<(), ()> {
        let mut ok = true;
        // 先に状態通知を外す（自分で止めたときの Stopped を届かせない）。
        if let Some(t) = self.status_token {
            if self.provider.RemoveAdvertisementStatusChanged(t).is_ok() {
                self.status_token = None;
            } else {
                ok = false;
            }
        }
        if let Some(t) = self.read_token {
            if self.char.RemoveReadRequested(t).is_ok() {
                self.read_token = None;
            } else {
                ok = false;
            }
        }
        if !self.adv_stopped {
            if self.provider.StopAdvertising().is_ok() {
                self.adv_stopped = true;
            } else {
                ok = false;
            }
        }
        if ok {
            Ok(())
        } else {
            Err(())
        }
    }

    /// 保持しているラジオが切れているか（同期プロパティの読み取りで、待たない）。
    fn radio_is_off(&self) -> bool {
        self.radio
            .as_ref()
            .and_then(|r| r.State().ok())
            .is_some_and(|s| s == RadioState::Off || s == RadioState::Disabled)
    }
}

fn guid(s: &str) -> Result<GUID, EndReason> {
    GUID::try_from(s).map_err(|_| EndReason::Failed)
}

fn reason_of(e: &windows::core::Error) -> EndReason {
    if e.code().0 == E_ACCESSDENIED {
        EndReason::PermissionDenied
    } else {
        EndReason::Failed
    }
}

/// アダプター取得の失敗。アダプター無し（null）は windows クレートが空エラー（code 0）にして返す。
fn adapter_reason(e: &windows::core::Error) -> EndReason {
    if e.code().0 == 0 {
        EndReason::Unsupported
    } else {
        reason_of(e)
    }
}

/// 非同期操作の完了を `wrap` で [`Cmd`] にしてキューへ積む。待たない。保留操作を返す（`Cancel` 用）。
fn spawn<T: RuntimeType + 'static>(
    op: windows::core::Result<IAsyncOperation<T>>,
    tx: &Sender<Cmd>,
    wrap: impl Fn(windows::core::Result<T>) -> Cmd + Send + 'static,
) -> windows::core::Result<IAsyncInfo> {
    let op = op?;
    let tx = tx.clone();
    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
        let result = match (op.ok(), status) {
            (Ok(op), AsyncStatus::Completed) => op.GetResults(),
            // 失敗・取り消しは成功にしない（理由が取れなければ E_FAIL）。
            (Ok(op), _) => Err(op
                .GetResults()
                .err()
                .unwrap_or_else(|| HRESULT(E_FAIL).into())),
            _ => Err(HRESULT(E_FAIL).into()),
        };
        // 受信側（ワーカー）が終わっていても無視してよい。
        let _ = tx.send(wrap(result));
        Ok(())
    }))?;
    op.cast()
}

/// Windows 実装。開始手順の途中の `stop` は保留操作を `Cancel` して即 `Stopped` を返す。
struct WindowsBlePeripheral;

static WORKER: OnceLock<Option<Sender<Cmd>>> = OnceLock::new();

fn worker() -> Option<&'static Sender<Cmd>> {
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let own = tx.clone();
            std::thread::Builder::new()
                .name("ble-peripheral".into())
                .spawn(move || run_worker(rx, own))
                .ok()
                .map(|_| tx)
        })
        .as_ref()
}

impl BlePeripheral for WindowsBlePeripheral {
    fn start(&self, session: u64, on_event: OnEvent) -> Result<StartOutcome, DeviceError> {
        SESSION.set(session, on_event);
        let sent = worker().is_some_and(|tx| tx.send(Cmd::Start(session)).is_ok());
        if !sent {
            SESSION.clear();
            return Err(DeviceError::Internal("ble_peripheral start"));
        }
        Ok(StartOutcome::Accepted)
    }

    fn stop(&self) -> Result<StopOutcome, DeviceError> {
        SESSION.clear();
        // ワーカー未起動なら動いていない。
        let Some(Some(tx)) = WORKER.get() else {
            return Ok(StopOutcome::NotRunning);
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.send(Cmd::Stop(reply_tx)).is_err() {
            return Err(DeviceError::Internal("ble_peripheral stop"));
        }
        // ワーカーは OS を待たないので、Stop には即座に返事が返る。
        reply_rx
            .recv()
            .unwrap_or(Err(DeviceError::Internal("ble_peripheral stop")))
    }
}

/// `device/ble_peripheral.rs` から呼ばれる、プロセスに 1 つの実装。
pub(crate) fn implementation() -> Arc<dyn BlePeripheral> {
    Arc::new(WindowsBlePeripheral)
}

fn run_worker(rx: mpsc::Receiver<Cmd>, tx: Sender<Cmd>) {
    let mut state = State::Idle;
    while let Ok(cmd) = rx.recv() {
        state = step(state, cmd, &tx);
    }
}

/// 開始手順の 1 段。現在の Opening の session と一致するときだけ `f` で次の操作を出し、
/// 失敗は `Ended` で確定して Idle へ戻る。一致しない（古い）完了は捨てる。
fn stage(
    state: State,
    session: u64,
    f: impl FnOnce(&mut Held) -> Result<IAsyncInfo, EndReason>,
) -> State {
    match state {
        State::Opening {
            session: current,
            mut held,
            ..
        } if current == session => match f(&mut held) {
            Ok(op) => State::Opening { session, op, held },
            Err(reason) => {
                SESSION.deliver(session, PeripheralEvent::Ended(reason));
                State::Idle
            }
        },
        other => other,
    }
}

fn step(state: State, cmd: Cmd, tx: &Sender<Cmd>) -> State {
    match cmd {
        Cmd::Start(session) => {
            // 前の状態を片付けてから開始する（完了通知なしで置き換える）。
            match state {
                State::Idle => {}
                State::Opening { op, .. } => {
                    let _ = op.Cancel();
                }
                State::Running(mut old) => {
                    if old.release().is_err() {
                        // 解放できないまま開始しない（契約表 9）。未解放は持ち越す。
                        SESSION.deliver(session, PeripheralEvent::Ended(EndReason::Failed));
                        return State::Running(old);
                    }
                }
            }
            match spawn(BluetoothAdapter::GetDefaultAsync(), tx, move |r| {
                Cmd::Adapter(session, r)
            }) {
                Ok(op) => State::Opening {
                    session,
                    op,
                    held: Held::default(),
                },
                Err(e) => {
                    SESSION.deliver(session, PeripheralEvent::Ended(adapter_reason(&e)));
                    State::Idle
                }
            }
        }
        Cmd::Adapter(session, result) => stage(state, session, |_| {
            let adapter = result.map_err(|e| adapter_reason(&e))?;
            if !adapter
                .IsPeripheralRoleSupported()
                .map_err(|e| reason_of(&e))?
            {
                return Err(EndReason::Unsupported);
            }
            spawn(adapter.GetRadioAsync(), tx, move |r| Cmd::Radio(session, r))
                .map_err(|e| reason_of(&e))
        }),
        Cmd::Radio(session, result) => stage(state, session, |held| {
            // ラジオが取れなくても開始は止めない（状態補正ができないだけ）。
            held.radio = result.ok();
            let uuid = guid(SERVICE_UUID)?;
            spawn(GattServiceProvider::CreateAsync(uuid), tx, move |r| {
                Cmd::Provider(session, r)
            })
            .map_err(|e| reason_of(&e))
        }),
        Cmd::Provider(session, result) => stage(state, session, |held| {
            let created = result.map_err(|e| reason_of(&e))?;
            let code = created.Error().map_err(|e| reason_of(&e))?.0;
            if code != 0 {
                return Err(end_reason_from_windows_bluetooth_error(code));
            }
            let provider = created.ServiceProvider().map_err(|e| reason_of(&e))?;
            let params = GattLocalCharacteristicParameters::new().map_err(|e| reason_of(&e))?;
            params
                .SetCharacteristicProperties(GattCharacteristicProperties::Read)
                .and_then(|_| params.SetReadProtectionLevel(GattProtectionLevel::Plain))
                .map_err(|e| reason_of(&e))?;
            let char_uuid = guid(CHARACTERISTIC_UUID)?;
            let op = provider
                .Service()
                .and_then(|s| s.CreateCharacteristicAsync(char_uuid, &params));
            held.provider = Some(provider);
            spawn(op, tx, move |r| Cmd::Char(session, r)).map_err(|e| reason_of(&e))
        }),
        Cmd::Char(session, result) => match state {
            State::Opening {
                session: current,
                mut held,
                ..
            } if current == session => match publish(session, &mut held, result, tx) {
                Ok(r) => State::Running(r),
                Err((reason, partial)) => {
                    SESSION.deliver(session, PeripheralEvent::Ended(reason));
                    // 部分的に確保した分の解放に失敗したら未解放として持ち越す。
                    match partial {
                        Some(mut p) => {
                            if p.release().is_err() {
                                State::Running(p)
                            } else {
                                State::Idle
                            }
                        }
                        None => State::Idle,
                    }
                }
            },
            other => other,
        },
        Cmd::Stop(reply) => match state {
            State::Idle => {
                let _ = reply.send(Ok(StopOutcome::NotRunning));
                State::Idle
            }
            State::Opening { op, .. } => {
                // Cancel の結果は見ない。競合して後から届く完了は session 照合で捨てる。
                let _ = op.Cancel();
                let _ = reply.send(Ok(StopOutcome::Stopped));
                State::Idle
            }
            State::Running(mut r) => match r.release() {
                Ok(()) => {
                    let _ = reply.send(Ok(StopOutcome::Stopped));
                    State::Idle
                }
                Err(()) => {
                    let _ = reply.send(Err(DeviceError::Internal("ble_peripheral stop")));
                    State::Running(r)
                }
            },
        },
        Cmd::Read(session) => {
            let mut state = state;
            if let State::Running(r) = &mut state {
                if r.session == session {
                    if r.advertising {
                        SESSION.deliver(session, PeripheralEvent::Read);
                    } else {
                        r.early_reads += 1;
                    }
                }
            }
            state
        }
        Cmd::Status {
            session,
            status,
            error,
        } => {
            let mut state = state;
            let mut released = false;
            if let State::Running(r) = &mut state {
                // 古い世代の通知は捨てる。
                if r.session == session {
                    match event_from_windows_advertisement_status(status, error) {
                        None => {}
                        Some(PeripheralEvent::Ended(mut reason)) => {
                            // Success と一緒に届いた Aborted 等は、保持ラジオを 1 回読んで直す。
                            if reason == EndReason::Failed && r.radio_is_off() {
                                reason = EndReason::BluetoothOff;
                            }
                            // 確定した reason は保つ。解放に失敗した分は未解放のまま持ち越す。
                            released = r.release().is_ok();
                            SESSION.deliver(session, PeripheralEvent::Ended(reason));
                        }
                        Some(PeripheralEvent::Advertising) => {
                            r.advertising = true;
                            SESSION.deliver(session, PeripheralEvent::Advertising);
                            for _ in 0..std::mem::take(&mut r.early_reads) {
                                SESSION.deliver(session, PeripheralEvent::Read);
                            }
                        }
                        Some(ev) => SESSION.deliver(session, ev),
                    }
                }
            }
            if released {
                State::Idle
            } else {
                state
            }
        }
    }
}

/// 特性の生成結果から、ハンドラ登録と広告開始までを同期で行う（どれも待たない呼び出し）。
/// 失敗時は理由と、部分的に確保した分（解放が要るもの）を返す。
fn publish(
    session: u64,
    held: &mut Held,
    result: windows::core::Result<GattLocalCharacteristicResult>,
    tx: &Sender<Cmd>,
) -> Result<Running, (EndReason, Option<Running>)> {
    let fail = |r: EndReason| (r, None);
    let char_result = result.map_err(|e| fail(reason_of(&e)))?;
    let code = char_result.Error().map_err(|e| fail(reason_of(&e)))?.0;
    if code != 0 {
        return Err(fail(end_reason_from_windows_bluetooth_error(code)));
    }
    let char = char_result
        .Characteristic()
        .map_err(|e| fail(reason_of(&e)))?;
    let provider = held.provider.take().ok_or(fail(EndReason::Failed))?;
    let radio = held.radio.take();

    // 公開の前にハンドラを登録する（無いと読み取りは UnlikelyError で即完了する）。
    let read_tx = tx.clone();
    let read_token = char
        .ReadRequested(&TypedEventHandler::new(
            move |_: Ref<'_, GattLocalCharacteristic>,
                  args: Ref<'_, GattReadRequestedEventArgs>| {
                handle_read(session, &read_tx, args.ok()?)
            },
        ))
        .map_err(|e| fail(reason_of(&e)))?;
    let status_tx = tx.clone();
    let status_token = match provider.AdvertisementStatusChanged(&TypedEventHandler::new(
        move |_: Ref<'_, GattServiceProvider>,
              args: Ref<'_, GattServiceProviderAdvertisementStatusChangedEventArgs>| {
            let args = args.ok()?;
            let status = args.Status().map_or(-1, |s| s.0);
            let error = args.Error().map_or(-1, |e| e.0);
            let _ = status_tx.send(Cmd::Status {
                session,
                status,
                error,
            });
            Ok(())
        },
    )) {
        Ok(t) => t,
        Err(e) => {
            // 1 つ目の登録を外してから失敗にする。外せなければ未解放として返す。
            let reason = reason_of(&e);
            return match char.RemoveReadRequested(read_token) {
                Ok(()) => Err((reason, None)),
                Err(_) => Err((
                    reason,
                    Some(Running {
                        session,
                        provider,
                        radio,
                        read_token: Some(read_token),
                        status_token: None,
                        char,
                        adv_stopped: true,
                        advertising: false,
                        early_reads: 0,
                    }),
                )),
            };
        }
    };

    let running = Running {
        session,
        provider,
        radio,
        read_token: Some(read_token),
        status_token: Some(status_token),
        char,
        adv_stopped: false,
        advertising: false,
        early_reads: 0,
    };
    let adv = GattServiceProviderAdvertisingParameters::new().and_then(|p| {
        p.SetIsConnectable(true)?;
        p.SetIsDiscoverable(true)?;
        running.provider.StartAdvertisingWithParameters(&p)
    });
    match adv {
        Ok(()) => Ok(running),
        Err(e) => {
            // 広告は開始していない（パラメータ生成・開始呼び出しの失敗）。解放で StopAdvertising を
            // 呼んで失敗し、Running が未解放のまま残るのを避ける。
            let mut running = running;
            running.adv_stopped = adv_stopped_after_start_failure();
            Err((reason_of(&e), Some(running)))
        }
    }
}

/// 広告開始の失敗経路で `adv_stopped` に入れる値。開始していないので停止済み扱い。
fn adv_stopped_after_start_failure() -> bool {
    true
}

/// 読み取り要求に応答する（WinRT のスレッドプールで動く。ワーカーではないので deferral 内の
/// `.get()` は許す）。成功した応答のうち offset 0 だけをキューへ積んで数える（macOS と同じ）。
fn handle_read(
    session: u64,
    tx: &Sender<Cmd>,
    args: &GattReadRequestedEventArgs,
) -> windows::core::Result<()> {
    let deferral = args.GetDeferral()?;
    let responded = (|| -> windows::core::Result<bool> {
        let request = args.GetRequestAsync()?.get()?;
        let value = READ_VALUE.as_bytes();
        let offset = request.Offset()? as usize;
        // 範囲外の offset は InvalidOffset で返し、数えない。
        if offset > value.len() {
            request.RespondWithProtocolError(GattProtocolError::InvalidOffset()?)?;
            return Ok(false);
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&value[offset..])?;
        request.RespondWithValue(&writer.DetachBuffer()?)?;
        Ok(offset == 0)
    })();
    // 全経路で完了させ、その失敗も見る。
    let completed = deferral.Complete();
    if matches!(responded, Ok(true)) && completed.is_ok() {
        let _ = tx.send(Cmd::Read(session));
    }
    responded.map(|_| ()).and(completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_uuids_parse() {
        assert!(guid(SERVICE_UUID).is_ok());
        assert!(guid(CHARACTERISTIC_UUID).is_ok());
        assert!(guid("not-a-guid").is_err());
    }

    #[test]
    fn adapter_failures_map_without_success() {
        // アダプター無し（null → code 0）は非対応、権限拒否は権限拒否、その他は失敗。
        let e = |c: i32| windows::core::Error::from(HRESULT(c));
        assert_eq!(adapter_reason(&e(0)), EndReason::Unsupported);
        assert_eq!(
            adapter_reason(&e(E_ACCESSDENIED)),
            EndReason::PermissionDenied
        );
        assert_eq!(adapter_reason(&e(E_FAIL)), EndReason::Failed);
    }

    #[test]
    fn start_failure_needs_no_stop_advertising() {
        // 広告開始に失敗した部分 Running は StopAdvertising を呼ばせない（解放が詰まらない）。
        assert!(adv_stopped_after_start_failure());
    }

    #[test]
    fn stale_completion_is_dropped() {
        // Idle へ届いた古い完了は状態を変えず、次の操作も出さない（成功を作らない）。
        let st = stage(State::Idle, 1, |_| panic!("次の段を出してはいけない"));
        assert!(matches!(st, State::Idle));
    }
}
