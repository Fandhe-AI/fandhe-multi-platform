//! Android 向け Bluetooth Classic の骨格（Issue #659 が置いた `Unsupported` のスタブ。実装は #661 が
//! このファイルの中身と Java 側 `BluetoothClassicHelper.java` を埋める）。
//!
//! 呼び出し元: `device/bluetooth_classic.rs` の Android 経路が一覧で [`implementation`]、検索で
//! [`present`] を呼ぶ。Activity 再生成時の保留破棄は `android.rs` が
//! `bluetooth_classic::reset_pending_tracking` で行う（配線済み）。dispatch・fallback・`device.rs` の
//! `pub mod`・共通コアの画面には触らない。契約の SSOT は `fandhe_device::bluetooth_classic`。
//!
//! # #661 が埋めるときの約束
//!
//! - 一覧は `BluetoothAdapter.getBondedDevices`、検索は `startDiscovery`（`ACTION_FOUND` /
//!   `ACTION_DISCOVERY_FINISHED`）。`getType` で LE だけの機器は除く
//! - Bluetooth がオフなら先に `Disabled`（`getBondedDevices` はオフのとき空を返すため）。API 30 以下で
//!   位置情報がオフの場合は `Internal`
//! - 窓は main Handler が閉じ、`cancelDiscovery` を呼んでから完了コードを渡す（コードの写像は
//!   `fandhe_device::bluetooth_classic::decode_code`）。受付は main Handler の 1 本に集める（契約表 11）
//! - 結果は `OBSERVED.observe` へ足し、完了は `finish` へ 1 回だけ渡す。名前・アドレスはログに出さない
//!
//! このスタブはダミーの成功（0 件の `Ok`）を返さない。

use std::time::Duration;

use fandhe_device::bluetooth_classic::{BluetoothClassic, UnsupportedBluetoothClassic};
use fandhe_device::DeviceError;

/// Android の `BluetoothClassic` 実装を返す（スタブ。#661 で置き換える）。
pub(crate) fn implementation() -> Box<dyn BluetoothClassic> {
    Box::new(UnsupportedBluetoothClassic)
}

/// 検索の開始を依頼する。スタブは常に `Unsupported`（コールバックは来ない）。
pub(crate) fn present(_request_id: u64, _duration: Duration) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("bluetooth_classic"))
}
