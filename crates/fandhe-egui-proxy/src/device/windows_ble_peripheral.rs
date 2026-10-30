//! Windows 向け BLE の周辺機器役実装（Issue #1101）。`device/ble_peripheral.rs` の Windows 経路
//! （`platform_ble_peripheral`）から呼ばれ、WinRT `GattServiceProvider` で固定のサービス UUID 1 つ・
//! 読み取り専用の特性 1 つを公開する。契約と守る範囲は `fandhe_device::ble_peripheral` のモジュール
//! doc、状態の写像（`BluetoothError` / アドバタイズ status → 事象）はそこの純関数が SSOT。
//!
//! # 流れ（専用ワーカースレッド上で同期的に）
//!
//! 1. `BluetoothAdapter::GetDefaultAsync().get()`: null（code 0）と `IsPeripheralRoleSupported() ==
//!    false` は `Unsupported`、`E_ACCESSDENIED` は `PermissionDenied`、その他は `Failed`
//! 2. `GattServiceProvider::CreateAsync` → `CreateCharacteristicAsync`（Read・Plain）。
//!    結果の `BluetoothError` は `end_reason_from_windows_bluetooth_error` で写す
//! 3. **公開の前に** `ReadRequested` / `AdvertisementStatusChanged` を登録する（ハンドラが無いと
//!    読み取りは `UnlikelyError` で即完了するため）
//! 4. `StartAdvertisingWithParameters(IsConnectable, IsDiscoverable)`。「公開中」は開始呼び出しの
//!    成功ではなく `AdvertisementStatusChanged` の Started で確定する（嘘の成功を作らない）
//!
//! Windows ではローカル名を指定できない（`IsDiscoverable` で OS が端末名を載せる）。中断
//! （`Interrupted`）は出さない。
//!
//! # 並行性（契約表 11）
//!
//! 開始手順の WinRT 待ちは `stop` が `Cancel` で中止するため、開始が未完了でも `stop` は実際の解放結果を短時間で返す。
//! 開始・停止・OS の状態通知は直列キュー（[`Cmd`]）1 本に集約し、状態（[`Running`]）はワーカーの
//! ローカル変数にだけ置く（ロックを持たない）。古い世代の通知は `session` で照合して捨てる。
//! 解放（`Remove*` / `StopAdvertising`）は戻り値を 1 件ずつ見て、失敗したら [`Running`] を
//! 未解放のまま持ち続けて次の Start / Stop で再試行する（要求より長く生きる場所に置く）。
//!
//! 中心役の識別子・読み取りの中身・HRESULT はログにも画面にも出さない（security.md A09）。
//! `unsafe` と `RoInitialize` は使わない。
//!
//! # 既知の天井
//!
//! unpackaged の exe で GATT サーバーが使えるかは未確認（実機確認は #776）。型検査は
//! `x86_64-pc-windows-gnu` のみ。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use windows::core::{Ref, RuntimeType, GUID, HRESULT};
use windows::Devices::Bluetooth::BluetoothAdapter;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattLocalCharacteristic, GattLocalCharacteristicParameters,
    GattProtectionLevel, GattProtocolError, GattReadRequestedEventArgs, GattServiceProvider,
    GattServiceProviderAdvertisementStatusChangedEventArgs,
    GattServiceProviderAdvertisingParameters,
};
use windows::Devices::Radios::RadioState;
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::DataWriter;
use windows_future::IAsyncOperation;

use fandhe_device::ble_peripheral::{
    end_reason_from_windows_bluetooth_error, event_from_windows_advertisement_status,
    BlePeripheral, EndReason, OnEvent, PeripheralEvent, StartOutcome, StopOutcome,
    CHARACTERISTIC_UUID, READ_VALUE, SERVICE_UUID,
};
use fandhe_device::DeviceError;

use super::ble_peripheral::SessionSlot;

/// `E_ACCESSDENIED`。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

/// 現在の公開の session と通知先（OS のハンドラとワーカーが共有する受け口）。
static SESSION: SessionSlot = SessionSlot::new();

/// `stop` が立てる中止要求。開始手順の WinRT 非同期待ち（[`wait`]）を中断させ、`Cmd::Stop` の処理で
/// 下ろす。これにより開始が未完了でも `stop` は短時間で実際の解放結果を返せる（UI を長く止めず、
/// 解放の成否を確かめる前に成功を返さない）。
static CANCEL: AtomicBool = AtomicBool::new(false);

/// いま待っている開始手順の非同期操作（`stop` が `Cancel` する先）。
static CURRENT_OP: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

/// 非同期操作の完了を待つ。中止要求があれば待たずに、待っている最中なら `Cancel` で失敗にする。
fn wait<T: RuntimeType + 'static>(op: IAsyncOperation<T>) -> windows::core::Result<T> {
    {
        let mut cur = CURRENT_OP.lock().unwrap_or_else(|e| e.into_inner());
        if CANCEL.load(Ordering::SeqCst) {
            return Err(HRESULT(0x8000_4004_u32 as i32).into()); // E_ABORT
        }
        let handle = op.clone();
        *cur = Some(Box::new(move || {
            let _ = handle.Cancel();
        }));
    }
    let r = op.get();
    *CURRENT_OP.lock().unwrap_or_else(|e| e.into_inner()) = None;
    r
}

/// 開始手順の待ちを中止する。
fn cancel_pending() {
    let cur = CURRENT_OP.lock().unwrap_or_else(|e| e.into_inner());
    CANCEL.store(true, Ordering::SeqCst);
    if let Some(cancel) = cur.as_ref() {
        cancel();
    }
}

/// ワーカーへの命令。開始・停止・OS の状態通知はすべてここを順に通る。
enum Cmd {
    Start(u64),
    Stop(Sender<Result<StopOutcome, DeviceError>>),
    Status {
        session: u64,
        status: i32,
        error: i32,
    },
}

/// ワーカーのローカル状態。解放に失敗した分は残り、次の機会に再試行する。
struct Running {
    session: u64,
    provider: GattServiceProvider,
    adapter: BluetoothAdapter,
    // 解放済みの項目は None / true にして、再試行で二重に外さない。
    read_token: Option<i64>,
    status_token: Option<i64>,
    char: GattLocalCharacteristic,
    adv_stopped: bool,
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

/// Windows 実装。開始手順の保留中の `stop` は待ちを中止（[`cancel_pending`]）してから解放結果を待つ。
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
        // 先に中止要求を立ててから Stop を積む。要求を下ろすのは Stop の処理（ワーカー側）で、必ずその後になる。
        // 下ろした後に要求が残って次の Start を E_ABORT にすることがない（キュー順で Stop が最後に効く）。
        cancel_pending();
        if tx.send(Cmd::Stop(reply_tx)).is_err() {
            CANCEL.store(false, Ordering::SeqCst);
            return Err(DeviceError::Internal("ble_peripheral stop"));
        }
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
    let mut running: Option<Running> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Start(session) => {
                // 前の公開は完了通知なしで置き換える。解放に失敗したら開始しない（契約表 9）。
                if let Some(old) = running.as_mut() {
                    if old.release().is_err() {
                        SESSION.deliver(session, PeripheralEvent::Ended(EndReason::Failed));
                        continue;
                    }
                    running = None;
                }
                let started = begin(session, &tx);
                match started {
                    Ok(r) => running = Some(r),
                    Err((reason, partial)) => {
                        // 部分的に確保した分を解放する。失敗したら未解放として持ち越す。
                        if let Some(mut p) = partial {
                            if p.release().is_err() {
                                running = Some(p);
                            }
                        }
                        SESSION.deliver(session, PeripheralEvent::Ended(reason));
                    }
                }
            }
            Cmd::Stop(reply) => {
                // 中止された開始は終わっている。以後の Start を巻き込まないよう下ろす。
                CANCEL.store(false, Ordering::SeqCst);
                let result = match running.as_mut() {
                    None => Ok(StopOutcome::NotRunning),
                    Some(r) => match r.release() {
                        Ok(()) => {
                            running = None;
                            Ok(StopOutcome::Stopped)
                        }
                        Err(()) => Err(DeviceError::Internal("ble_peripheral stop")),
                    },
                };
                let _ = reply.send(result);
            }
            Cmd::Status {
                session,
                status,
                error,
            } => {
                // 古い世代の通知は捨てる。
                let Some(r) = running.as_mut().filter(|r| r.session == session) else {
                    continue;
                };
                match event_from_windows_advertisement_status(status, error) {
                    None => {}
                    Some(PeripheralEvent::Ended(mut reason)) => {
                        // Success と一緒に届いた Aborted 等は、確定時点でラジオを 1 回観測して直す。
                        if reason == EndReason::Failed && radio_is_off(&r.adapter) {
                            reason = EndReason::BluetoothOff;
                        }
                        // 確定した reason は保つ。解放に失敗した分は未解放のまま持ち越す。
                        if r.release().is_ok() {
                            running = None;
                        }
                        SESSION.deliver(session, PeripheralEvent::Ended(reason));
                    }
                    Some(ev) => SESSION.deliver(session, ev),
                }
            }
        }
    }
}

fn radio_is_off(adapter: &BluetoothAdapter) -> bool {
    adapter
        .GetRadioAsync()
        .and_then(|op| op.get())
        .and_then(|radio| radio.State())
        .is_ok_and(|s| s == RadioState::Off || s == RadioState::Disabled)
}

/// 公開を始める。失敗時は理由と、部分的に確保した分（解放が要るもの）を返す。
fn begin(session: u64, tx: &Sender<Cmd>) -> Result<Running, (EndReason, Option<Running>)> {
    let fail = |r: EndReason| (r, None);
    let adapter = BluetoothAdapter::GetDefaultAsync()
        .and_then(wait)
        .map_err(|e| {
            // アダプタ無し（null）は windows クレートが空エラー（code 0）にして返す。
            if e.code().0 == 0 {
                fail(EndReason::Unsupported)
            } else {
                fail(reason_of(&e))
            }
        })?;
    if !adapter
        .IsPeripheralRoleSupported()
        .map_err(|e| fail(reason_of(&e)))?
    {
        return Err(fail(EndReason::Unsupported));
    }

    let created = GattServiceProvider::CreateAsync(guid(SERVICE_UUID).map_err(fail)?)
        .and_then(wait)
        .map_err(|e| fail(reason_of(&e)))?;
    let code = created.Error().map_err(|e| fail(reason_of(&e)))?.0;
    if code != 0 {
        return Err(fail(end_reason_from_windows_bluetooth_error(code)));
    }
    let provider = created.ServiceProvider().map_err(|e| fail(reason_of(&e)))?;

    let params = GattLocalCharacteristicParameters::new().map_err(|e| fail(reason_of(&e)))?;
    params
        .SetCharacteristicProperties(GattCharacteristicProperties::Read)
        .and_then(|_| params.SetReadProtectionLevel(GattProtectionLevel::Plain))
        .map_err(|e| fail(reason_of(&e)))?;
    let char_uuid = guid(CHARACTERISTIC_UUID).map_err(fail)?;
    let char_result = provider
        .Service()
        .and_then(|s| s.CreateCharacteristicAsync(char_uuid, &params))
        .and_then(wait)
        .map_err(|e| fail(reason_of(&e)))?;
    let code = char_result.Error().map_err(|e| fail(reason_of(&e)))?.0;
    if code != 0 {
        return Err(fail(end_reason_from_windows_bluetooth_error(code)));
    }
    let char = char_result
        .Characteristic()
        .map_err(|e| fail(reason_of(&e)))?;

    // 公開の前にハンドラを登録する。
    let read_token = char
        .ReadRequested(&TypedEventHandler::new(
            move |_: Ref<'_, GattLocalCharacteristic>,
                  args: Ref<'_, GattReadRequestedEventArgs>| {
                handle_read(session, args.ok()?)
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
                        adapter,
                        read_token: Some(read_token),
                        status_token: None,
                        char,
                        adv_stopped: true,
                    }),
                )),
            };
        }
    };

    let running = Running {
        session,
        provider,
        adapter,
        read_token: Some(read_token),
        status_token: Some(status_token),
        char,
        adv_stopped: false,
    };
    let adv = GattServiceProviderAdvertisingParameters::new().and_then(|p| {
        p.SetIsConnectable(true)?;
        p.SetIsDiscoverable(true)?;
        running.provider.StartAdvertisingWithParameters(&p)
    });
    match adv {
        Ok(()) => Ok(running),
        Err(e) => Err((reason_of(&e), Some(running))),
    }
}

/// 読み取り要求に応答する（WinRT のスレッドプールで動く）。成功した応答だけを数える。
fn handle_read(session: u64, args: &GattReadRequestedEventArgs) -> windows::core::Result<()> {
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
        Ok(true)
    })();
    // 全経路で完了させ、その失敗も見る。
    let completed = deferral.Complete();
    if matches!(responded, Ok(true)) && completed.is_ok() {
        SESSION.deliver(session, PeripheralEvent::Read);
    }
    responded.and(completed)
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
}
