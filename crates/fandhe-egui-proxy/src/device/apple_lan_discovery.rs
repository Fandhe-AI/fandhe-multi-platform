//! macOS / iOS 向け LAN のサービス探索実装（Issue #486 macOS。iOS は #488）。
//! `device/lan_discovery.rs` の macOS 経路（iOS は `ios_lan_discovery` の委譲）から呼ばれ、
//! `apple/fandhe_apple_lan_discovery.m`（`nw_browser` による発見と dns_sd による解決、`build.rs` が
//! iOS / macOS の feature `device` 有効時にコンパイル・リンクする）を `unsafe extern "C"` 2 関数
//! （[`present`] / [`stop`]）で呼ぶ。`unsafe` はこの呼び出しと `CStr::from_ptr` に閉じ
//! `// SAFETY:` で契約を明記する（`apple_bluetooth_le.rs` と同型、coding-rust.md の FFI 例外規定）。
//!
//! `.m` 側のコールバックから [`fandhe_rust_lan_service`] / [`fandhe_rust_lan_finished`]
//! （本モジュールが公開する `extern "C"` 関数）が呼ばれ、`device/lan_discovery` の観測バッファへ
//! push する。OS から受けた名前・アドレス・ポートは非信頼入力として検証してから入れる。
//! コードの写像は `fandhe_device::lan_discovery::decode_code`（SSOT）に一本化する。
//! サービス名・アドレスをログへ出力しない（security.md A09）。

use std::ffi::CStr;
use std::os::raw::c_char;
use std::time::Duration;

use fandhe_device::lan_discovery::{clamp_name, decode_code, parse_addr, port_from, LanService};
use fandhe_device::DeviceError;

use super::lan_discovery::{finish, OBSERVED};

// SAFETY: `apple/fandhe_apple_lan_discovery.m` が定義する C ABI（`build.rs` が iOS / macOS の
// feature `device` 有効時にコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_lan_start(request_id: u64, duration_ms: u64) -> i32;
    fn fandhe_apple_lan_stop();
}

/// 探索開始を依頼する。窓は呼び出し元が渡す `duration`（`BROWSE_DURATION_MS`）で、`.m` 側は
/// この値で閉じる。`.m` は常に 0（受理）を返す契約で、0 以外は依頼の失敗として返す。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, duration: Duration) -> Result<(), DeviceError> {
    let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    // SAFETY: 引数はプリミティブのみでポインタを経由しない。戻り値 0 は依頼を受理したことだけを
    // 意味し、結果は非同期に `fandhe_rust_lan_*` 経由で届く。
    let code = unsafe { fandhe_apple_lan_start(request_id, ms) };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("lan_discovery present"))
    }
}

/// 進行中の探索を止める（終了時専用。完了コールバックは呼ばれない）。
#[allow(unsafe_code)]
pub(crate) fn stop() {
    // SAFETY: 引数なし。`.m` 側は専用キューへ同期ディスパッチして `nw_browser` と dns_sd の
    // 参照を解放する（main queue は使わないため、呼び出し元がどのロックを持っていても
    // デッドロックしない。ただし呼び出し元は Rust 側のロックを握らない契約）。
    unsafe { fandhe_apple_lan_stop() };
}

/// サービス 1 件の通知（`.m` の専用キューから呼ばれる）。`addr` は NULL 可（未解決）。`port` は
/// 0 で未解決。ポインタはこの呼び出しが戻るまでのみ有効なため、ここで同期的にコピーし切る。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_lan_service(
    request_id: u64,
    name: *const c_char,
    addr: *const c_char,
    port: u16,
) {
    if name.is_null() {
        return;
    }
    // SAFETY: `name` は非 NULL で、`.m` 側が NUL 終端 UTF-8 を渡す契約。呼び出しが戻るまで有効で、
    // ここで即座にコピーする。
    let name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        return;
    }
    let addr = if addr.is_null() {
        None
    } else {
        // SAFETY: 同上（NULL は上で除外済み）。
        let a = unsafe { CStr::from_ptr(addr) }.to_string_lossy();
        parse_addr(&a)
    };
    OBSERVED.observe(
        request_id,
        LanService {
            name: clamp_name(name),
            addr,
            port: port_from(port),
        },
    );
}

/// 完了通知（`.m` の専用キューから 1 要求につき 1 回だけ呼ばれる）。
/// `code` の意味は `fandhe_device::lan_discovery::decode_code`。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_lan_finished(request_id: u64, code: i32) {
    finish(request_id, decode_code(code));
    // iOS では OS コールバックが結果を共有状態へ入れた直後に次フレームを起こす
    // （入力が無いと egui が描画せず画面が探索中のまま止まる、`device-bridge-contracts.md` 4 節）。
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
