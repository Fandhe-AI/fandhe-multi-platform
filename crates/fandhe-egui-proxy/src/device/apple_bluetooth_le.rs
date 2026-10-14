//! iOS / macOS 向け BLE スキャン実装（Issue #1060 macOS・#1061 iOS）。
//! `device/bluetooth_le.rs` の iOS / macOS 経路から呼ばれ、CoreBluetooth 実装
//! （`apple/fandhe_apple_bluetooth_le.m`、`build.rs` が iOS / macOS の feature `device`
//! 有効時にコンパイル・リンクする）を `unsafe extern "C"` 2 関数（[`present`] /
//! [`stop`]）で呼ぶ。`unsafe` はこの呼び出しと `CStr::from_ptr` に閉じ `// SAFETY:` で
//! 契約を明記する（`apple_speech_recognition.rs` と同型、coding-rust.md の FFI 例外規定）。
//!
//! `.m` 側のコールバックから [`fandhe_rust_ble_device`] / [`fandhe_rust_ble_finished`]
//! （本モジュールが公開する `extern "C"` 関数）が呼ばれ、`device/bluetooth_le` の観測
//! バッファへ push する。コードの写像は `device/bluetooth_le.rs` の純関数（SSOT）に
//! 一本化する。周辺機器の名前・識別子をログへ出力しない（security.md A09）。

use std::ffi::CStr;
use std::os::raw::c_char;
use std::time::Duration;

use fandhe_device::bluetooth_le::BleDevice;

use super::bluetooth_le::{error_from_apple_code, finish, rssi_from_apple, OBSERVED};

// SAFETY: `apple/fandhe_apple_bluetooth_le.m` が定義する C ABI（`build.rs` が iOS / macOS の
// feature `device` 有効時にコンパイル・リンクする）。契約は `.m` ファイル冒頭コメント
// に集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_ble_start(request_id: u64, duration_ms: u64) -> i32;
    fn fandhe_apple_ble_stop();
}

/// スキャン開始を依頼する。期限は呼び出し元が渡す `duration`（`SCAN_DURATION_MS`）で、
/// `.m` 側はこの値で止める（OS 側の期限と契約の期限を一致させる）。`.m` は常に 0
/// （受理）を返す契約。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, duration: Duration) -> bool {
    let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    // SAFETY: 引数はプリミティブのみでポインタを経由しない。戻り値 0 は依頼を受理した
    // ことだけを意味し、結果は非同期に `fandhe_rust_ble_*` 経由で届く。
    let code = unsafe { fandhe_apple_ble_start(request_id, ms) };
    code == 0
}

/// 進行中のスキャンを止める（終了時専用。完了コールバックは呼ばれない）。
#[allow(unsafe_code)]
pub(crate) fn stop() {
    // SAFETY: 引数なし。`.m` 側は専用キューへ同期ディスパッチして `stopScan` と解放を行う
    // （main queue は使わないため、呼び出し元がどのロックを持っていてもデッドロックしない。
    // ただし呼び出し元は Rust 側のロックを握らない契約）。
    unsafe { fandhe_apple_ble_stop() };
}

/// 広告 1 件の通知（`.m` の専用キューから呼ばれる）。`name` は NULL 可。`rssi` は
/// 取れないとき 127（番兵、[`rssi_from_apple`] が `None` に写す）。ポインタはこの呼び出しが
/// 戻るまでのみ有効なため、ここで同期的にコピーし切る。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ble_device(
    request_id: u64,
    id: *const c_char,
    name: *const c_char,
    rssi: i32,
) {
    if id.is_null() {
        return;
    }
    // SAFETY: `id` は非 NULL で、`.m` 側が NUL 終端 UTF-8 を渡す契約。呼び出しが戻るまで
    // 有効で、ここで即座にコピーする。
    let id = unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned();
    let name = if name.is_null() {
        None
    } else {
        // SAFETY: 同上（NULL は上で除外済み）。
        let n = unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        (!n.is_empty()).then_some(n)
    };
    OBSERVED.observe(
        request_id,
        BleDevice {
            id,
            name,
            rssi_dbm: rssi_from_apple(rssi),
        },
    );
}

/// 完了通知（`.m` の専用キューから 1 要求につき 1 回だけ呼ばれる）。
/// `code`: 0 = 成功、1 = オフ、2 = 権限拒否、3 = 非対応、4 = 内部エラー。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ble_finished(request_id: u64, code: i32) {
    let result = if code == 0 {
        Ok(())
    } else {
        Err(error_from_apple_code(code))
    };
    finish(request_id, result);
}
