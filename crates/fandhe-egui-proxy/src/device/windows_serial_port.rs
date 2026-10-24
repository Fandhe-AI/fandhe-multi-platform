//! Windows 向けシリアルポート実装（Issue #671、親 #131）。Win32 の COM ポート API
//! （`GetCommPorts` / `CreateFileW` / `SetCommState` / `SetCommTimeouts` / `PurgeComm` /
//! `ReadFile` / `WriteFile`）を `windows` クレート経由で呼ぶ。WinRT `SerialDevice` は package
//! identity が必須のため使わない（unpackaged の exe で動かすため）。
//!
//! 呼び出し元: `device/serial_port.rs` の Windows 経路が [`implementation`] を呼ぶ。契約の SSOT は
//! `fandhe_device::serial_port` で、往復ループ `round_trip` は共用でここには持たない。dispatch・
//! `device.rs`・共通コアの画面には触らない。
//!
//! # 構成
//!
//! - 一覧: `GetCommPorts`（Windows 10 1803 以降）。`ERROR_FILE_NOT_FOUND` だけを 0 件の成功にし、
//!   `ERROR_MORE_DATA` は 1 回だけ拡張して再試行する。一覧の失敗を `failure_from_win32` には通さない
//!   （2 が `Gone`、5 が `Busy` になり、一覧の文脈では意味がずれるため）。結果は
//!   `normalize_ports(.., is_windows_com_name)` を通す
//! - 開く: 名前を `is_windows_com_name` で検証し `\\.\COMn` にして `CreateFileW`（共有モード 0 の
//!   排他、非 overlapped）。DCB（9600 bps・8N1・フロー制御なし）と `COMMTIMEOUTS` を適用し、
//!   `GetCommState` で読み直して `verify_dcb` で確かめ（ドライバが黙って別の値にしても成功にしない）、
//!   `ClearCommError` と `PurgeComm` で受信バッファまで捨てて返す。任意のデバイスパスは開かない
//!   （security.md A03）
//! - 読み書き: 期限は `wait` に合わせた `COMMTIMEOUTS` で表す。0 バイトは「まだ届かない・書けない」で、
//!   期限は `round_trip` が単調時計で `Timeout` に確定する
//! - 閉じる: `WinSerialLink` の `Drop` が唯一の close 経路。変更前の DCB とタイムアウトを戻して
//!   `CloseHandle` する（全終了経路。共有資源を残さない）
//!
//! エラーは [`failure_from_hresult`] の 1 か所で `failure_from_win32` に写す（HRESULT の
//! `FACILITY_WIN32` から下位 16 bit を取り出す。それ以外は `Internal`）。OS から受けた件数・バイト数は
//! 使う前に上限で検証する。ポート名・送受信の中身・Win32 エラーの生値をログ・`Debug`・失敗文言に
//! 出さない（security.md A09）。
//!
//! # 既知の天井（【推測】は Windows 実機確認 #667 で確かめる）
//!
//! - 【推測】使用中の COM ポートで `CreateFileW` が `ERROR_ACCESS_DENIED` を返し `Busy` になるか
//! - 【推測】USB シリアルの抜去で `ReadFile` が FALSE（`ERROR_GEN_FAILURE` 等）を返し `Gone` になるか。
//!   TRUE と 0 バイトが続く場合は `Timeout` になる（嘘の成功にはならない）
//! - 【推測】書き込みの期限切れが TRUE と短いバイト数で返るか（`ERROR_SEM_TIMEOUT`・`ERROR_TIMEOUT` で
//!   返る場合は防御的に 0 バイトとして扱う）
//! - 【推測】unpackaged の exe で manifest・capability の宣言が要らないか
//! - `GetCommPorts` が無い 1803 未満は対象外（静的 import のため起動できない可能性がある）
//! - 終了時に待つ 300 ms（dispatch 側）で足りるか
//!
//! - DTR / RTS を ENABLE にしているのは、機器によっては信号線が立たないと応答しないため（フロー制御
//!   ではなく静的な出力）。Arduino 系はオープン時の DTR で自動リセットされ、起動が終わるまで 3 秒の期限内に
//!   応答できないことがある（期限切れになる）
//! - COM256 以上は一覧に出ない（`is_windows_com_name` が `COM1`〜`COM255` のみ通す）
//!
//! 型検査は `x86_64-pc-windows-gnu` で行い、実機確認は #667 で行う。純関数は OS 非依存でホストでも
//! テストできるよう、モジュールは `cfg(any(windows, test))`。

use std::time::Duration;

use fandhe_device::serial_port::{
    failure_from_win32, is_windows_com_name, normalize_ports, SerialPortEntry, SerialPortFailure,
};

/// `ERROR_FILE_NOT_FOUND`。`GetCommPorts` は COM ポートが 1 つも無いときこれを返す。
const ERROR_FILE_NOT_FOUND: u32 = 2;
/// `ERROR_MORE_DATA`。`GetCommPorts` のバッファ不足（必要件数は `found` に入る）。
const ERROR_MORE_DATA: u32 = 234;
/// `ERROR_SEM_TIMEOUT`。
const ERROR_SEM_TIMEOUT: u32 = 121;
/// `ERROR_TIMEOUT`。
const ERROR_TIMEOUT: u32 = 1460;
/// `FACILITY_WIN32` の HRESULT 接頭辞（`0x8007xxxx`）。
const HRESULT_WIN32_PREFIX: u32 = 0x8007_0000;
/// 一覧に使う最初のバッファ件数。足りなければ [`LIST_CAP_MAX`] まで 1 回だけ拡張する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const LIST_CAP_INIT: usize = 256;
/// 一覧バッファの上限件数（OS が返した必要件数はこれで丸める）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const LIST_CAP_MAX: usize = 4096;

/// `MAXDWORD`。
const MAXDWORD: u32 = u32::MAX;

/// `DCB._bitfield` のビット位置（`fBinary` 0、`fParity` 1、`fOutxCtsFlow` 2、`fOutxDsrFlow` 3、
/// `fDtrControl` 4-5、`fDsrSensitivity` 6、`fTXContinueOnXoff` 7、`fOutX` 8、`fInX` 9、
/// `fErrorChar` 10、`fNull` 11、`fRtsControl` 12-13、`fAbortOnError` 14）。
const DCB_F_BINARY: u32 = 1 << 0;
const DCB_DTR_CONTROL_SHIFT: u32 = 4;
const DCB_RTS_CONTROL_SHIFT: u32 = 12;
/// `DTR_CONTROL_ENABLE` / `RTS_CONTROL_ENABLE`。
const CONTROL_ENABLE: u32 = 1;
/// 上の定義済みビット全体（0〜14）。予約ビット（15〜31）は触らない。
const DCB_DEFINED_MASK: u32 = 0x7FFF;

/// [`classify_list_status`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListStatus {
    /// 成功（`found` 件が入っている）。
    Ok,
    /// COM ポートが無い。0 件の成功。
    Empty,
    /// バッファ不足。必要件数で再試行する。
    Retry,
    /// それ以外は内部失敗。
    Failed,
}

/// HRESULT が `FACILITY_WIN32` なら Win32 エラーコード（下位 16 bit）を返す。
fn win32_code_from_hresult(hr: i32) -> Option<u32> {
    let hr = hr as u32;
    (hr & 0xFFFF_0000 == HRESULT_WIN32_PREFIX).then_some(hr & 0xFFFF)
}

/// HRESULT を失敗種別へ写す（全呼び出し箇所で共用する唯一の写像）。Win32 由来でなければ `Internal`。
fn failure_from_hresult(hr: i32) -> SerialPortFailure {
    win32_code_from_hresult(hr).map_or(SerialPortFailure::Internal, failure_from_win32)
}

/// `GetCommPorts` の戻りコードを振り分ける。
fn classify_list_status(code: u32) -> ListStatus {
    match code {
        0 => ListStatus::Ok,
        ERROR_FILE_NOT_FOUND => ListStatus::Empty,
        ERROR_MORE_DATA => ListStatus::Retry,
        _ => ListStatus::Failed,
    }
}

/// `GetCommPorts` が返したポート番号を一覧の項目へ整える。`found` がバッファ長を超えるのは
/// OS の契約違反で、0 件の成功に畳まず `Internal`。
fn entries_from_port_numbers(
    numbers: &[u32],
    found: u32,
) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
    let found = usize::try_from(found).map_err(|_| SerialPortFailure::Internal)?;
    if found > numbers.len() {
        return Err(SerialPortFailure::Internal);
    }
    let mut nums = numbers[..found].to_vec();
    nums.sort_unstable();
    Ok(normalize_ports(
        nums.into_iter().map(|n| format!("COM{n}")),
        is_windows_com_name,
    ))
}

/// 検証済みの COM ポート名を `\\.\COMn` の NUL 終端 UTF-16 にする（`COM10` 以上は接頭辞が必須）。
/// 検証を通らない名前は `None`（任意のデバイスパスを開かない、security.md A03）。
fn device_path(name: &str) -> Option<Vec<u16>> {
    if !is_windows_com_name(name) {
        return None;
    }
    Some(
        format!(r"\\.\{name}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect(),
    )
}

/// DCB のビットフィールドを固定設定へ組む。バイナリモード・DTR/RTS 常時 ON（信号線の静的な出力で
/// フロー制御ではない）以外は全部落とす（パリティ・CTS/DSR/XON-XOFF フロー制御・`fNull`・
/// `fErrorChar`・`fAbortOnError`）。`fAbortOnError` を落とすのは、通信エラーで以後の I/O が
/// `ERROR_OPERATION_ABORTED` に固着するのを避けるため。予約ビットは保つ。
fn dcb_bitfield(current: u32) -> u32 {
    (current & !DCB_DEFINED_MASK)
        | DCB_F_BINARY
        | (CONTROL_ENABLE << DCB_DTR_CONTROL_SHIFT)
        | (CONTROL_ENABLE << DCB_RTS_CONTROL_SHIFT)
}

/// `fOutxCtsFlow` / `fOutxDsrFlow` / `fOutX` / `fInX`（フロー制御）と DTR / RTS 制御のビット。
const DCB_VERIFY_MASK: u32 = (1 << 2)
    | (1 << 3)
    | (3 << DCB_DTR_CONTROL_SHIFT)
    | (1 << 8)
    | (1 << 9)
    | (3 << DCB_RTS_CONTROL_SHIFT);

/// `SetCommState` 後に `GetCommState` で読み直した DCB が、適用した固定設定
/// （9600 bps・8 bit・パリティなし・1 ストップビット・フロー制御なし・DTR/RTS は `dcb_bitfield` の値）
/// と一致するかを確かめる。ドライバが値を黙って丸めたら嘘の成功にせず `Internal`
/// （macOS 側の `tcgetattr` 読み直しと同じ考え方）。
fn verify_dcb(
    baud: u32,
    byte_size: u8,
    parity: u8,
    stop_bits: u8,
    bitfield: u32,
    expected_bitfield: u32,
) -> Result<(), SerialPortFailure> {
    let ok = baud == fandhe_device::serial_port::BAUD_RATE
        && byte_size == 8
        && parity == 0
        && stop_bits == 0
        && (bitfield & DCB_VERIFY_MASK) == (expected_bitfield & DCB_VERIFY_MASK);
    if ok {
        Ok(())
    } else {
        Err(SerialPortFailure::Internal)
    }
}

/// `GetCommPorts` が `ERROR_MORE_DATA` を返したときの次のバッファ件数。拡張は 1 回だけで、
/// すでに拡張済み・必要件数が現在以下（契約違反）・上限到達なら `None`（呼び出し側は `Internal`）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn next_list_cap(cap: usize, found: u32, retried: bool) -> Option<usize> {
    if retried || cap >= LIST_CAP_MAX {
        return None;
    }
    let need = usize::try_from(found).ok()?;
    (need > cap).then(|| need.min(LIST_CAP_MAX))
}

/// `wait` を `COMMTIMEOUTS` の Constant（ミリ秒）へ丸める。`ReadIntervalTimeout` と
/// `ReadTotalTimeoutMultiplier` を `MAXDWORD` にする組み合わせでは、Constant は `1..MAXDWORD` の範囲。
fn timeout_ms(wait: Duration) -> u32 {
    u32::try_from(wait.as_millis())
        .unwrap_or(MAXDWORD)
        .clamp(1, MAXDWORD - 1)
}

/// `ReadFile` / `WriteFile` の結果を契約の値へ写す。`requested` を超える件数は嘘の成功にせず
/// `Internal`。期限切れ系の 2 コードは「まだ」として 0 バイト（期限は `round_trip` が確定する）。
fn classify_io(result: Result<u32, i32>, requested: usize) -> Result<usize, SerialPortFailure> {
    match result {
        Ok(n) => {
            let n = usize::try_from(n).map_err(|_| SerialPortFailure::Internal)?;
            if n > requested {
                Err(SerialPortFailure::Internal)
            } else {
                Ok(n)
            }
        }
        Err(hr) => match win32_code_from_hresult(hr) {
            Some(ERROR_SEM_TIMEOUT | ERROR_TIMEOUT) => Ok(0),
            _ => Err(failure_from_hresult(hr)),
        },
    }
}

#[cfg(target_os = "windows")]
mod os {
    use super::*;
    use fandhe_device::serial_port::{SerialLink, SerialPorts, BAUD_RATE};
    use windows::core::PCWSTR;
    use windows::Win32::Devices::Communication::{
        ClearCommError, GetCommPorts, GetCommState, GetCommTimeouts, PurgeComm, SetCommState,
        SetCommTimeouts, COMMTIMEOUTS, DCB, DCB_PARITY, DCB_STOP_BITS, PURGE_RXCLEAR,
        PURGE_TXCLEAR,
    };
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_EXISTING,
    };

    /// 開く前の DCB とタイムアウト。`Drop` で戻す。
    struct Saved {
        dcb: DCB,
        timeouts: COMMTIMEOUTS,
    }

    /// 開いたポート 1 本。`Drop` が唯一の close 経路（元の設定を戻して `CloseHandle`）。
    /// ポート名・ハンドル値は `Debug` に出さないため、`Debug` は実装しない。
    pub(super) struct WinSerialLink {
        handle: HANDLE,
        saved: Option<Saved>,
        /// 最後に適用した (読み, 書き) の Constant（ミリ秒）。
        applied: (u32, u32),
    }

    // SAFETY: `HANDLE` は `WinSerialLink` が唯一の所有者で、`&mut self` の間だけ使う。ワーカー間で
    // 所有権を移すだけで、同時には使わない。
    #[allow(unsafe_code)]
    unsafe impl Send for WinSerialLink {}

    fn timeouts(read_ms: u32, write_ms: u32) -> COMMTIMEOUTS {
        COMMTIMEOUTS {
            ReadIntervalTimeout: MAXDWORD,
            ReadTotalTimeoutMultiplier: MAXDWORD,
            ReadTotalTimeoutConstant: read_ms,
            WriteTotalTimeoutMultiplier: 0,
            WriteTotalTimeoutConstant: write_ms,
        }
    }

    impl WinSerialLink {
        /// 必要なら読み・書きのタイムアウトを更新する。
        fn apply_timeouts(&mut self, read_ms: u32, write_ms: u32) -> Result<(), SerialPortFailure> {
            if self.applied == (read_ms, write_ms) {
                return Ok(());
            }
            let t = timeouts(read_ms, write_ms);
            // SAFETY: `handle` は open が返した未 close のもので、`&mut self` により排他。`t` は有効な参照。
            #[allow(unsafe_code)]
            unsafe { SetCommTimeouts(self.handle, &t) }
                .map_err(|e| failure_from_hresult(e.code().0))?;
            self.applied = (read_ms, write_ms);
            Ok(())
        }

        /// 固定設定を適用して受信バッファまで捨てる。失敗しても呼び出し側の `Drop` が閉じる。
        fn configure(&mut self) -> Result<(), SerialPortFailure> {
            let map = |e: windows::core::Error| failure_from_hresult(e.code().0);
            let mut dcb = DCB {
                DCBlength: std::mem::size_of::<DCB>() as u32,
                ..Default::default()
            };
            let mut old_t = COMMTIMEOUTS::default();
            // SAFETY: `handle` は有効で、`dcb` / `old_t` は呼び出しの間有効な排他参照。
            #[allow(unsafe_code)]
            unsafe {
                GetCommState(self.handle, &mut dcb).map_err(map)?;
                GetCommTimeouts(self.handle, &mut old_t).map_err(map)?;
            }
            self.saved = Some(Saved {
                dcb,
                timeouts: old_t,
            });
            dcb.BaudRate = BAUD_RATE;
            dcb.ByteSize = 8;
            dcb.Parity = DCB_PARITY(0); // NOPARITY
            dcb.StopBits = DCB_STOP_BITS(0); // ONESTOPBIT
            dcb._bitfield = dcb_bitfield(dcb._bitfield);
            let expected_bitfield = dcb._bitfield;
            let t = timeouts(100, 100);
            // SAFETY: `handle` は有効で、`dcb` は呼び出しの間有効な参照。
            #[allow(unsafe_code)]
            unsafe {
                SetCommState(self.handle, &dcb).map_err(map)?;
            }
            // 読み直して固定設定が本当に効いたかを確かめる。
            let mut got = DCB {
                DCBlength: std::mem::size_of::<DCB>() as u32,
                ..Default::default()
            };
            // SAFETY: `handle` は有効で、`got` は呼び出しの間有効な排他参照。
            #[allow(unsafe_code)]
            unsafe {
                GetCommState(self.handle, &mut got).map_err(map)?;
            }
            verify_dcb(
                got.BaudRate,
                got.ByteSize,
                got.Parity.0,
                got.StopBits.0,
                got._bitfield,
                expected_bitfield,
            )?;
            // SAFETY: `handle` は有効で、`t` は呼び出しの間有効な参照。`ClearCommError` の
            // 出力先は使わないため None。
            #[allow(unsafe_code)]
            unsafe {
                SetCommTimeouts(self.handle, &t).map_err(map)?;
                ClearCommError(self.handle, None, None).map_err(map)?;
                PurgeComm(self.handle, PURGE_RXCLEAR | PURGE_TXCLEAR).map_err(map)?;
            }
            self.applied = (100, 100);
            Ok(())
        }
    }

    impl SerialLink for WinSerialLink {
        fn write_some(&mut self, buf: &[u8], wait: Duration) -> Result<usize, SerialPortFailure> {
            self.apply_timeouts(self.applied.0, timeout_ms(wait))?;
            let mut n = 0u32;
            // SAFETY: `handle` は有効で `&mut self` により排他。`buf` / `n` は呼び出しの間有効（非 overlapped）。
            #[allow(unsafe_code)]
            let r = unsafe { WriteFile(self.handle, Some(buf), Some(&mut n), None) };
            classify_io(r.map(|()| n).map_err(|e| e.code().0), buf.len())
        }

        fn read_some(
            &mut self,
            buf: &mut [u8],
            wait: Duration,
        ) -> Result<usize, SerialPortFailure> {
            self.apply_timeouts(timeout_ms(wait), self.applied.1)?;
            let (len, mut n) = (buf.len(), 0u32);
            // SAFETY: `write_some` と同じ。OS は `len` を超えて書かない。
            #[allow(unsafe_code)]
            let r = unsafe { ReadFile(self.handle, Some(buf), Some(&mut n), None) };
            classify_io(r.map(|()| n).map_err(|e| e.code().0), len)
        }

        fn close(self: Box<Self>) {
            // `Drop` が設定を戻して閉じる。
            drop(self);
        }
    }

    impl Drop for WinSerialLink {
        fn drop(&mut self) {
            // SAFETY: open が返した未 close のハンドルで、Drop は 1 度だけ走り以後使わない。
            // 復元はベストエフォート（失敗は無視し、ログにも出さない）。
            #[allow(unsafe_code)]
            unsafe {
                if let Some(s) = self.saved.take() {
                    let _ = SetCommState(self.handle, &s.dcb);
                    let _ = SetCommTimeouts(self.handle, &s.timeouts);
                }
                let _ = CloseHandle(self.handle);
            }
        }
    }

    /// Windows の `SerialPorts` 実装。状態を持たない。
    pub(super) struct WinSerialPorts;

    impl SerialPorts for WinSerialPorts {
        fn list(&self) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
            let mut cap = LIST_CAP_INIT;
            let mut retried = false;
            loop {
                let mut buf = vec![0u32; cap];
                let mut found = 0u32;
                // SAFETY: `buf` / `found` は呼び出しの間有効な排他参照で、OS は保持しない。
                #[allow(unsafe_code)]
                let code = unsafe { GetCommPorts(&mut buf, &mut found) };
                match classify_list_status(code) {
                    ListStatus::Ok => return entries_from_port_numbers(&buf, found),
                    ListStatus::Empty => return Ok(Vec::new()),
                    ListStatus::Retry => {
                        // 必要件数（上限で丸める）で 1 回だけ拡張する。2 回目の不足・必要件数が
                        // 現在以下（契約違反）・上限到達は Internal。
                        cap = next_list_cap(cap, found, retried)
                            .ok_or(SerialPortFailure::Internal)?;
                        retried = true;
                    }
                    ListStatus::Failed => return Err(SerialPortFailure::Internal),
                }
            }
        }

        fn open(&self, name: &str) -> Result<Box<dyn SerialLink>, SerialPortFailure> {
            // 任意のデバイスパスを開かない（security.md A03）。
            let path = device_path(name).ok_or(SerialPortFailure::Internal)?;
            // SAFETY: `path` は NUL 終端の UTF-16 で呼び出しの間有効。共有モード 0 の排他、非 overlapped。
            #[allow(unsafe_code)]
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            }
            .map_err(|e| failure_from_hresult(e.code().0))?;
            // ハンドルを取った直後に所有構造体にし、以後の早期 return は Drop が閉じる。
            let mut link = WinSerialLink {
                handle,
                saved: None,
                applied: (0, 0),
            };
            link.configure()?;
            Ok(Box::new(link))
        }
    }
}

/// Windows の `SerialPorts` 実装を返す（`device/serial_port.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::serial_port::SerialPorts> {
    std::sync::Arc::new(os::WinSerialPorts)
}

/// Windows 以外（ホストの `cargo test`）では実装を持たない。ダミーの成功へは倒さず `Unsupported`
/// を返す（dispatch はこの経路を呼ばない）。
#[cfg(not(target_os = "windows"))]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::serial_port::SerialPorts> {
    std::sync::Arc::new(fandhe_device::serial_port::UnsupportedSerialPorts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hresult_maps_through_win32() {
        assert_eq!(
            failure_from_hresult(0x8007_0005_u32 as i32),
            SerialPortFailure::Busy
        );
        assert_eq!(
            failure_from_hresult(0x8007_0002_u32 as i32),
            SerialPortFailure::Gone
        );
        // E_FAIL は Win32 由来ではない。
        assert_eq!(
            failure_from_hresult(0x8000_4005_u32 as i32),
            SerialPortFailure::Internal
        );
        assert_eq!(win32_code_from_hresult(0), None);
    }

    #[test]
    fn port_numbers_are_sorted_validated_and_capped() {
        let v = entries_from_port_numbers(&[3, 1, 0, 256, 1, 0, 0], 5).unwrap();
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["COM1", "COM3"]);
        assert!(entries_from_port_numbers(&[1, 2], 0).unwrap().is_empty());
        let many: Vec<u32> = (1..=40).collect();
        assert_eq!(entries_from_port_numbers(&many, 40).unwrap().len(), 32);
    }

    #[test]
    fn found_beyond_buffer_is_internal() {
        assert_eq!(
            entries_from_port_numbers(&[1, 2], 3).unwrap_err(),
            SerialPortFailure::Internal
        );
    }

    #[test]
    fn list_status_classification() {
        assert_eq!(classify_list_status(0), ListStatus::Ok);
        assert_eq!(classify_list_status(2), ListStatus::Empty);
        assert_eq!(classify_list_status(234), ListStatus::Retry);
        assert_eq!(classify_list_status(5), ListStatus::Failed);
    }

    #[test]
    fn device_path_accepts_only_valid_com_names() {
        let p = device_path("COM1").unwrap();
        assert_eq!(*p.last().unwrap(), 0);
        assert_eq!(String::from_utf16(&p[..p.len() - 1]).unwrap(), r"\\.\COM1");
        assert!(device_path("COM255").is_some());
        for bad in ["COM0", "COM256", "com1", "COM01", "LPT1", r"..\x", ""] {
            assert!(device_path(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn dcb_bitfield_is_fixed_config_only() {
        assert_eq!(
            dcb_bitfield(u32::MAX),
            0xFFFF_8000 | 1 | (1 << 4) | (1 << 12)
        );
        assert_eq!(dcb_bitfield(0), 1 | (1 << 4) | (1 << 12));
    }

    #[test]
    fn verify_dcb_accepts_applied_config() {
        let exp = dcb_bitfield(0);
        assert_eq!(verify_dcb(9600, 8, 0, 0, exp, exp), Ok(()));
        // 検証対象外のビット（fAbortOnError 等）の差は許す。
        assert_eq!(verify_dcb(9600, 8, 0, 0, exp | (1 << 14), exp), Ok(()));
    }

    #[test]
    fn verify_dcb_rejects_each_mismatch() {
        let exp = dcb_bitfield(0);
        let bad = Err(SerialPortFailure::Internal);
        assert_eq!(verify_dcb(115_200, 8, 0, 0, exp, exp), bad);
        assert_eq!(verify_dcb(9600, 7, 0, 0, exp, exp), bad);
        assert_eq!(verify_dcb(9600, 8, 2, 0, exp, exp), bad);
        assert_eq!(verify_dcb(9600, 8, 0, 2, exp, exp), bad);
        for bit in [2u32, 3, 8, 9] {
            assert_eq!(
                verify_dcb(9600, 8, 0, 0, exp | (1 << bit), exp),
                bad,
                "{bit}"
            );
        }
        // DTR / RTS が期待と違う（ハンドシェイクに変えられた）。
        assert_eq!(verify_dcb(9600, 8, 0, 0, exp | (1 << 5), exp), bad);
        assert_eq!(verify_dcb(9600, 8, 0, 0, exp & !(1 << 12), exp), bad);
        assert_eq!(verify_dcb(9600, 8, 0, 0, exp & !(1 << 4), exp), bad);
    }

    #[test]
    fn list_cap_expands_once() {
        assert_eq!(next_list_cap(256, 300, false), Some(300));
        assert_eq!(next_list_cap(256, 100_000, false), Some(LIST_CAP_MAX));
        // 2 回目、必要件数が現在以下、上限到達は拡張しない。
        assert_eq!(next_list_cap(300, 400, true), None);
        assert_eq!(next_list_cap(256, 256, false), None);
        assert_eq!(next_list_cap(LIST_CAP_MAX, 5000, false), None);
    }

    #[test]
    fn timeout_is_clamped() {
        assert_eq!(timeout_ms(Duration::ZERO), 1);
        assert_eq!(timeout_ms(Duration::from_millis(100)), 100);
        assert_eq!(timeout_ms(Duration::from_secs(u64::MAX / 2)), MAXDWORD - 1);
    }

    #[test]
    fn io_classification() {
        assert_eq!(classify_io(Ok(3), 7), Ok(3));
        assert_eq!(classify_io(Ok(8), 7), Err(SerialPortFailure::Internal));
        for code in [121u32, 1460] {
            assert_eq!(classify_io(Err((0x8007_0000 | code) as i32), 7), Ok(0));
        }
        for code in [31u32, 1617] {
            assert_eq!(
                classify_io(Err((0x8007_0000 | code) as i32), 7),
                Err(SerialPortFailure::Gone)
            );
        }
        assert_eq!(
            classify_io(Err((0x8007_0000u32 | 9999) as i32), 7),
            Err(SerialPortFailure::Internal)
        );
    }
}
