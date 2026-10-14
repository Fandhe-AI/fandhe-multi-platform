//! Windows 向け BLE スキャン実装（Issue #1060）。`device/bluetooth_le.rs` の Windows
//! 経路から呼ばれ、WinRT `BluetoothLEAdvertisementWatcher` を [`present`] /
//! [`stop_shared`] のセッション管理（ワーカースレッド・期限タイマ・停止合図）で駆動する
//! （`BluetoothLe` トレイトは通さない。`windows_speech_recognition.rs` と同型）。
//!
//! # 流れ（ワーカースレッド上で同期的に）
//!
//! 1. `BluetoothAdapter::GetDefaultAsync().get()`: アダプタ無し（null）のみ `Unsupported`、
//!    アクセス拒否は `PermissionDenied`、その他は `Internal`。
//!    `IsLowEnergySupported() == false` も `Unsupported`
//! 2. ラジオ状態が `Off` / `Disabled` なら `Disabled`（成功・0 台と区別する）
//! 3. watcher を `Active` で作る（名前はスキャン応答で届くため）。`Received` は広告ごとに
//!    `bluetooth_le::OBSERVED` へ足し、`Stopped` は OS 側の中断（`BluetoothError`）を
//!    記録する
//! 4. `Start()` → 期限（`SCAN_DURATION_MS`、呼び出し元が渡す）か停止合図まで待つ →
//!    `Stop()` → **`Stopped` 通知の到着を待つ**（期限なし・専用ワーカー上）→ ハンドラ解除。
//!    **全経路で**ハンドラを解除する（契約表 9 節）
//! 5. `Stopped` の `Error` が Success のときだけ成功。期限前に OS 起点で停止していたら
//!    （Success でも）`error_from_bluetooth_error` で失敗にする（部分結果を成功にしない）
//!
//! 周辺機器の名前・アドレスをログへ出さない（security.md A09）。`unsafe` と
//! `RoInitialize` は使わない（既存の Windows 実装群と同じ前提）。
//!
//! # 並行性・世代
//!
//! セッションは [`SESSION`]。新しい要求は古いセッションへ停止を送って置き換える。
//! 完了は `bluetooth_le::finish`（request_id 照合つき）へ渡し、古い世代の結果は捨てられる。
//! `SESSION` は結果を届ける前に解除する（届けた後に次の要求が来ても busy にならない）。
//!
//! # 既知の天井
//!
//! パッケージ無しの exe で動く前提（マニフェスト capability は宣言しない）。許可が必要な
//! 環境では `ConsentRequired` / `DisabledByUser` を `PermissionDenied` として返す。
//! 実機確認は未実施（ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ）。

use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use windows::core::Ref;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEAdvertisementWatcherStoppedEventArgs, BluetoothLEScanningMode,
};
use windows::Devices::Bluetooth::BluetoothAdapter;
use windows::Devices::Radios::RadioState;
use windows::Foundation::TypedEventHandler;

use fandhe_device::bluetooth_le::BleDevice;
use fandhe_device::DeviceError;

use super::bluetooth_le::{error_from_bluetooth_error, finish, OBSERVED};

/// `RawSignalStrengthInDBm` の「取得不能」番兵（実際の値域と衝突しないため `None` に写す）。
const RSSI_UNAVAILABLE: i16 = -127;
/// `E_ACCESSDENIED`（ラジオへのアクセス拒否）。
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

struct Session {
    request_id: u64,
    stop_tx: mpsc::Sender<()>,
}

/// 現在進行中のセッション（プロセス static、1 度に 1 つ）。
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn lock_session() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

fn clear_if_current(request_id: u64) {
    let mut session = lock_session();
    if session.as_ref().is_some_and(|s| s.request_id == request_id) {
        *session = None;
    }
}

/// `windows::core::Error` → 契約のエラー。アクセス拒否だけ `PermissionDenied`、
/// それ以外は内部エラー（HRESULT は画面へ出さない）。
fn map_win_error(e: &windows::core::Error, what: &'static str) -> DeviceError {
    if e.code().0 == E_ACCESSDENIED {
        DeviceError::PermissionDenied("bluetooth_le")
    } else {
        DeviceError::Internal(what)
    }
}

/// `device/bluetooth_le.rs::drive_callback` から呼ばれる。ワーカーを起動して即座に返る。
/// 進行中のセッションがあれば停止を送って置き換える（古い結果は request_id 照合で捨てる）。
pub(crate) fn present(request_id: u64, duration: Duration) -> Result<(), DeviceError> {
    let (stop_tx, stop_rx) = mpsc::channel();
    {
        let mut session = lock_session();
        if let Some(old) = session.replace(Session {
            request_id,
            stop_tx,
        }) {
            let _ = old.stop_tx.send(());
        }
    }

    let spawned = std::thread::Builder::new().spawn(move || {
        let result = run_scan(request_id, duration, &stop_rx);
        // 届ける前に状態を確定する（自分の request_id のときだけ解除）。
        clear_if_current(request_id);
        finish(request_id, result);
    });
    if spawned.is_err() {
        clear_if_current(request_id);
        return Err(DeviceError::Internal("bluetooth_le present"));
    }
    Ok(())
}

/// アプリ終了時に進行中のセッションへ停止を送る（`stop_shared_scan` から）。
pub(crate) fn stop_shared() {
    let session = lock_session().take();
    if let Some(s) = session {
        let _ = s.stop_tx.send(());
    }
}

/// 48bit アドレスを `AA:BB:CC:DD:EE:FF` に整形する。
fn format_address(addr: u64) -> String {
    (0..6)
        .rev()
        .map(|i| format!("{:02X}", (addr >> (i * 8)) & 0xFF))
        .collect::<Vec<_>>()
        .join(":")
}

fn run_scan(
    request_id: u64,
    duration: Duration,
    stop_rx: &mpsc::Receiver<()>,
) -> Result<(), DeviceError> {
    // アダプタ無し（null）は windows クレートが空エラー（code 0）の `Err` にして返す。
    // その場合だけ Unsupported。アクセス拒否は PermissionDenied、それ以外は Internal。
    let adapter = BluetoothAdapter::GetDefaultAsync()
        .map_err(|e| map_win_error(&e, "bluetooth_le init"))?
        .get()
        .map_err(|e| {
            if e.code().0 == 0 {
                DeviceError::Unsupported("bluetooth_le")
            } else {
                map_win_error(&e, "bluetooth_le init")
            }
        })?;
    if !adapter
        .IsLowEnergySupported()
        .map_err(|e| map_win_error(&e, "bluetooth_le init"))?
    {
        return Err(DeviceError::Unsupported("bluetooth_le"));
    }
    let radio = adapter
        .GetRadioAsync()
        .map_err(|e| map_win_error(&e, "bluetooth_le radio"))?
        .get()
        .map_err(|e| map_win_error(&e, "bluetooth_le radio"))?;
    let state = radio
        .State()
        .map_err(|e| map_win_error(&e, "bluetooth_le radio"))?;
    if state == RadioState::Off || state == RadioState::Disabled {
        return Err(DeviceError::Disabled("bluetooth_le"));
    }

    let watcher = BluetoothLEAdvertisementWatcher::new()
        .map_err(|e| map_win_error(&e, "bluetooth_le init"))?;
    watcher
        .SetScanningMode(BluetoothLEScanningMode::Active)
        .map_err(|e| map_win_error(&e, "bluetooth_le init"))?;

    let received = watcher
        .Received(&TypedEventHandler::new(
            move |_: Ref<'_, BluetoothLEAdvertisementWatcher>,
                  args: Ref<'_, BluetoothLEAdvertisementReceivedEventArgs>| {
                let args = args.ok()?;
                let name = args
                    .Advertisement()
                    .and_then(|a| a.LocalName())
                    .map(|n| n.to_string_lossy())
                    .ok()
                    .filter(|n| !n.is_empty());
                let rssi = args
                    .RawSignalStrengthInDBm()
                    .ok()
                    .filter(|r| *r != RSSI_UNAVAILABLE);
                OBSERVED.observe(
                    request_id,
                    BleDevice {
                        id: format_address(args.BluetoothAddress()?),
                        name,
                        rssi_dbm: rssi,
                    },
                );
                Ok(())
            },
        ))
        .map_err(|e| map_win_error(&e, "bluetooth_le start"))?;

    // `Stopped` の到着と `BluetoothError`（0 = Success）を 1 本のチャネルで受ける。
    let (stopped_tx, stopped_rx) = mpsc::channel::<i32>();
    let stopped = match watcher.Stopped(&TypedEventHandler::new(
        move |_: Ref<'_, BluetoothLEAdvertisementWatcher>,
              args: Ref<'_, BluetoothLEAdvertisementWatcherStoppedEventArgs>| {
            // Error を読めなくても停止自体は通知する（読めない場合は Internal 扱いにする）。
            let code = args.ok()?.Error().map_or(-1, |e| e.0);
            let _ = stopped_tx.send(code);
            Ok(())
        },
    )) {
        Ok(token) => token,
        Err(e) => {
            let _ = watcher.RemoveReceived(received);
            return Err(map_win_error(&e, "bluetooth_le start"));
        }
    };

    // 開始から停止確定までの結果。成否に関わらず下で必ずハンドラを解除する。
    let outcome = (|| {
        watcher
            .Start()
            .map_err(|e| map_win_error(&e, "bluetooth_le start"))?;
        const POLL: Duration = Duration::from_millis(100);
        let deadline = Instant::now() + duration;
        let mut early = None;
        loop {
            match stop_rx.recv_timeout(POLL) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    early = stopped_rx.try_recv().ok();
                    if early.is_some() || Instant::now() >= deadline {
                        break;
                    }
                }
            }
        }
        // 自分の `Stop()` 前に届いた停止は OS 起点（Bluetooth オフ等）。Success でも期限前の
        // 停止は部分結果なので失敗にする。
        if let Some(code) = early.or_else(|| stopped_rx.try_recv().ok()) {
            return Err(stopped_error(code, true));
        }
        watcher
            .Stop()
            .map_err(|e| map_win_error(&e, "bluetooth_le stop"))?;
        // 自分の `Stop()` に対する `Stopped` を待ち、Error を確定してから成功にする。
        // 待機は専用ワーカー上で行い UI スレッドを塞がない。期限は設けない（巻き戻さない）。
        match stopped_rx.recv() {
            Ok(0) => Ok(()),
            Ok(code) => Err(stopped_error(code, false)),
            Err(_) => Err(DeviceError::Internal("bluetooth_le stop")),
        }
    })();

    // 失敗経路でも動作中の watcher を止める（成功経路では停止済みで無害）。
    let _ = watcher.Stop();
    let _ = watcher.RemoveReceived(received);
    let _ = watcher.RemoveStopped(stopped);
    outcome
}

/// `Stopped` の `BluetoothError` コードを契約のエラーへ写す。`early` は期限前の OS 起点停止
/// （Success でも失敗）。Error を読めなかった（-1）場合は `Internal`。
fn stopped_error(code: i32, early: bool) -> DeviceError {
    match code {
        0 if early => DeviceError::Internal("bluetooth_le stopped early"),
        -1 => DeviceError::Internal("bluetooth_le stopped"),
        c => error_from_bluetooth_error(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_is_formatted_as_six_hex_octets() {
        assert_eq!(format_address(0x0011_2233_4455), "00:11:22:33:44:55");
        assert_eq!(format_address(0xAABB_CCDD_EEFF), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn stopped_error_maps_early_success_and_radio_off() {
        assert!(matches!(stopped_error(0, true), DeviceError::Internal(_)));
        assert!(matches!(stopped_error(1, false), DeviceError::Disabled(_)));
        assert!(matches!(stopped_error(-1, false), DeviceError::Internal(_)));
    }

    #[test]
    fn stop_shared_without_session_is_noop() {
        stop_shared();
    }
}
