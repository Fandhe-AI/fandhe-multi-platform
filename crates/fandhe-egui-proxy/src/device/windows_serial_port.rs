//! Windows 向けシリアルポートの骨格（Issue #670 が置いた `Unsupported` のスタブ。実装は #671 が
//! このファイルの中身だけを埋める）。
//!
//! 呼び出し元: `device/serial_port.rs` の Windows 経路が [`implementation`] を呼ぶ。dispatch・
//! fallback・`device.rs` の `pub mod`・共通コアの画面・`fandhe-device` には触らない。契約の SSOT は
//! `fandhe_device::serial_port`。
//!
//! # #671 が埋めるときの約束
//!
//! - 一覧は Win32 `GetCommPorts`。`ERROR_FILE_NOT_FOUND` は 0 件の成功にする。結果は
//!   `normalize_ports(.., is_windows_com_name)` を通す
//! - 開くのは `CreateFileW`（名前は `is_windows_com_name` で検証してから）→ DCB（9600 bps・8N1・
//!   フロー制御なし）→ `COMMTIMEOUTS`（`MAXDWORD` / `MAXDWORD` / 100 ms の読み取り）→ `PurgeComm`
//! - エラーは `failure_from_win32` に通す。`HANDLE` は `Drop` で閉じる
//! - 往復ループは自前で書かず `fandhe_device::serial_port::round_trip` を使う
//! - `windows` クレートの feature `Win32_Devices_Communication` を足す
//!
//! このスタブはダミーの成功（0 件の `Ok`）を返さない。

use std::sync::Arc;

use fandhe_device::serial_port::{SerialPorts, UnsupportedSerialPorts};

/// Windows の `SerialPorts` 実装を返す（スタブ。#671 で置き換える）。
pub(crate) fn implementation() -> Arc<dyn SerialPorts> {
    Arc::new(UnsupportedSerialPorts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::serial_port::SerialPortFailure;

    #[test]
    fn stub_never_succeeds() {
        let p = implementation();
        assert_eq!(p.list().map(|_| ()), Err(SerialPortFailure::Unsupported));
        assert!(matches!(
            p.open("COM1"),
            Err(SerialPortFailure::Unsupported)
        ));
    }
}
