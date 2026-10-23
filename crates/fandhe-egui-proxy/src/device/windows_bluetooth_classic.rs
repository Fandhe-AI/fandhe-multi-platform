//! Windows 向け Bluetooth Classic の骨格（Issue #659 が置いた `Unsupported` のスタブ。実装は #660 が
//! このファイルの中身だけを埋める）。
//!
//! 呼び出し元: `device/bluetooth_classic.rs` の Windows 経路が一覧で [`implementation`]、検索で
//! [`present`] を呼び、終了時に [`stop_shared`] を呼ぶ。dispatch・fallback・`device.rs` の `pub mod`・
//! 共通コアの画面には触らない。契約の SSOT は `fandhe_device::bluetooth_classic`。
//!
//! # #660 が埋めるときの約束
//!
//! - 検索は Win32 `BluetoothFindFirstDevice`（`cTimeoutMultiplier` 9 ≒ 11.5 秒）。同期の inquiry は
//!   途中で止められないため、専用ワーカーで走らせる
//! - 期限は呼び出しの開始を起点に単調時計で測り、窓に `WINDOWS_DEADLINE_GRACE_MS` を足した時点で
//!   `Internal` を確定する。遅れて届いた結果は request_id の照合で捨てる
//! - 結果は `OBSERVED.observe` へ足し、完了は `finish` へ 1 回だけ渡す。電波強度は取れないので `None`
//! - 開始・置換・停止は 1 本の専用ワーカー（`mpsc` + 専用スレッド）へ順に流す（契約表 11）
//!
//! このスタブはダミーの成功（0 件の `Ok`）を返さない。

use std::time::Duration;

use fandhe_device::bluetooth_classic::{BluetoothClassic, UnsupportedBluetoothClassic};
use fandhe_device::DeviceError;

/// Windows の `BluetoothClassic` 実装を返す（スタブ。#660 で置き換える）。
pub(crate) fn implementation() -> Box<dyn BluetoothClassic> {
    Box::new(UnsupportedBluetoothClassic)
}

/// 検索の開始を依頼する。スタブは常に `Unsupported`（コールバックは来ない）。
pub(crate) fn present(_request_id: u64, _duration: Duration) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("bluetooth_classic"))
}

/// 進行中の検索を止める（終了時専用。スタブは何もしない）。
pub(crate) fn stop_shared() {}
