//! iOS / macOS 向け BLE の周辺機器役の実装（Issue #778 macOS、#779 iOS）。
//! `device/ble_peripheral.rs` の macOS 経路と `ios_ble_peripheral`（委譲）から呼ばれ、
//! CoreBluetooth `CBPeripheralManager` 実装（`apple/fandhe_apple_ble_peripheral.m`、`build.rs` が
//! iOS / macOS の feature `device` 有効時にコンパイル・リンクする）を `unsafe extern "C"` 2 関数で
//! 呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する（`apple_bluetooth_le.rs` と
//! 同型、coding-rust.md の FFI 例外規定）。
//!
//! `.m` 側のコールバックから [`fandhe_rust_ble_peripheral_event`]（本モジュールが公開する
//! `extern "C"`）が呼ばれ、`device/ble_peripheral.rs` の `SessionSlot` が session を照合して
//! 通知先へ渡す。コードの写像は `fandhe_device::ble_peripheral` の純関数（SSOT）。
//! 中心役の識別子と読み取りの中身はログに出さない（security.md A09）。

use std::ffi::c_char;
use std::sync::Arc;

use fandhe_device::ble_peripheral::{
    event_from_apple_code, stop_outcome_from_apple, BlePeripheral, OnEvent, StartOutcome,
    StopOutcome, CHARACTERISTIC_UUID, LOCAL_NAME, READ_VALUE, SERVICE_UUID,
};
use fandhe_device::DeviceError;

use super::ble_peripheral::SessionSlot;

/// 現在の公開の session と通知先（プロセスに 1 つ）。
static SESSION: SessionSlot = SessionSlot::new();

// SAFETY: `apple/fandhe_apple_ble_peripheral.m` が定義する C ABI（`build.rs` が iOS / macOS の
// feature `device` 有効時にコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_ble_peripheral_start(
        session: u64,
        service_uuid: *const c_char,
        characteristic_uuid: *const c_char,
        local_name: *const c_char,
        value: *const c_char,
    ) -> i32;
    fn fandhe_apple_ble_peripheral_stop() -> i32;
}

/// 固定値を NUL 終端の `'static` 文字列にする（定数は ASCII で NUL を含まない）。
fn c_str(literal: &'static str) -> Option<std::ffi::CString> {
    std::ffi::CString::new(literal).ok()
}

/// CoreBluetooth 実装。状態は `.m` の専用キュー上と [`SESSION`] だけにあり、ここには持たない。
struct ApplePeripheral;

impl BlePeripheral for ApplePeripheral {
    #[allow(unsafe_code)]
    fn start(&self, session: u64, on_event: OnEvent) -> Result<StartOutcome, DeviceError> {
        let (Some(service), Some(characteristic), Some(name), Some(value)) = (
            c_str(SERVICE_UUID),
            c_str(CHARACTERISTIC_UUID),
            c_str(LOCAL_NAME),
            c_str(READ_VALUE),
        ) else {
            return Err(DeviceError::Internal("ble_peripheral start"));
        };
        // 先に通知先を置く（`.m` の最初の事象を落とさない）。古い session の事象は照合で捨てられる。
        SESSION.set(session, on_event);
        // SAFETY: 4 つのポインタは呼び出しの間生きている NUL 終端の CString を指す。`.m` は戻る前に
        // NSString へコピーし終える契約で、ポインタを保持しない。戻り値 0 は依頼の受理だけを意味し、
        // 結果は非同期に `fandhe_rust_ble_peripheral_event` 経由で届く。
        let code = unsafe {
            fandhe_apple_ble_peripheral_start(
                session,
                service.as_ptr(),
                characteristic.as_ptr(),
                name.as_ptr(),
                value.as_ptr(),
            )
        };
        if code == 0 {
            Ok(StartOutcome::Accepted)
        } else {
            SESSION.clear();
            Err(DeviceError::Internal("ble_peripheral start"))
        }
    }

    #[allow(unsafe_code)]
    fn stop(&self) -> Result<StopOutcome, DeviceError> {
        // Rust 側のロックを離してから `.m` の専用キューへ同期ディスパッチする（契約表 11）。
        SESSION.clear();
        // SAFETY: 引数なし。`.m` 側は専用キューへ同期ディスパッチして停止と解放を行う
        // （main queue は使わない）。呼び出し元は Rust 側のロックを握らない契約。
        let code = unsafe { fandhe_apple_ble_peripheral_stop() };
        stop_outcome_from_apple(code)
    }
}

/// macOS 経路の実装（iOS も `ios_ble_peripheral` が委譲する）。
pub(crate) fn implementation() -> Arc<dyn BlePeripheral> {
    Arc::new(ApplePeripheral)
}

/// 事象の通知（`.m` の専用キューから呼ばれる）。ポインタを受けず、panic せず、`dispatch_sync` しない。
/// `code` の意味は `fandhe_device::ble_peripheral` の `CODE_*`。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ble_peripheral_event(session: u64, code: i32) {
    SESSION.deliver(session, event_from_apple_code(code));
    // iOS では OS コールバックが事象を共有状態へ入れた直後に次フレームを起こす
    // （入力が無いと egui が描画せず画面が処理中のまま止まる、`device-bridge-contracts.md` 4 節）。
    // 古い session の事象でも再描画は無害なので `deliver` の結果に関係なく呼ぶ。
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_strings_convert_to_c_strings() {
        for s in [SERVICE_UUID, CHARACTERISTIC_UUID, LOCAL_NAME, READ_VALUE] {
            let c = c_str(s).unwrap();
            assert_eq!(c.to_str().unwrap(), s);
        }
    }
}
