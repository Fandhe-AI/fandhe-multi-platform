//! macOS / iOS 向け近くの端末との直接通信の実装（Issue #511 macOS。iOS は b-ios が委譲する）。
//! `device/local_peer.rs` の macOS 経路（iOS は `ios_local_peer` の委譲）から呼ばれ、
//! `apple/fandhe_apple_local_peer.m`（MultipeerConnectivity、`build.rs` が iOS / macOS の
//! feature `device` 有効時にコンパイル・リンクする）を `unsafe extern "C"` 2 関数
//! （[`present`] / [`stop`]）で呼ぶ。`unsafe` はこの呼び出しと `CStr` / `slice` の読み取りに閉じ
//! `// SAFETY:` で契約を明記する（`apple_lan_discovery.rs` と同型、coding-rust.md の FFI 例外規定）。
//!
//! `.m` 側のコールバックから [`fandhe_rust_local_peer_offer`] / [`fandhe_rust_local_peer_finished`]
//! （本モジュールが公開する `extern "C"` 関数）が呼ばれ、`device/local_peer` の受信バッファへ渡す。
//! OS から受けた名前とバイト列は非信頼入力として `Pending::offer` が検証する。
//! コードの写像は `fandhe_device::local_peer::decode_code`（SSOT）に一本化する。
//! 表示名・受け取った文字列をログへ出力しない（security.md A09）。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::time::Duration;

use fandhe_device::local_peer::decode_code;
use fandhe_device::DeviceError;

use super::local_peer::{finish, PENDING};

// SAFETY: `apple/fandhe_apple_local_peer.m` が定義する C ABI（`build.rs` が iOS / macOS の
// feature `device` 有効時にコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_local_peer_start(
        request_id: u64,
        own_name: *const c_char,
        outgoing: *const u8,
        outgoing_len: usize,
        window_ms: u64,
    ) -> i32;
    fn fandhe_apple_local_peer_stop();
}

/// 交換開始を依頼する。窓は呼び出し元が渡す `window`（`EXCHANGE_WINDOW_MS`）で、`.m` 側はこの値で
/// 閉じる。`.m` は引数を同期でコピーし、常に 0（受理）を返す契約。0 以外は依頼の失敗として返す。
#[allow(unsafe_code)]
pub(crate) fn present(
    request_id: u64,
    own_name: &str,
    outgoing: &str,
    window: Duration,
) -> Result<(), DeviceError> {
    let own = CString::new(own_name).map_err(|_| DeviceError::Internal("local_peer name"))?;
    let ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    // SAFETY: `own` は NUL 終端、`outgoing` は長さ付きで、どちらもこの呼び出しの間だけ有効。
    // `.m` 側は戻る前に `NSString` / `NSData` へコピーし、非同期処理へポインタを持ち越さない。
    let code = unsafe {
        fandhe_apple_local_peer_start(
            request_id,
            own.as_ptr(),
            outgoing.as_ptr(),
            outgoing.len(),
            ms,
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("local_peer present"))
    }
}

/// 進行中の交換を止める（終了時専用。完了コールバックは呼ばれない）。
#[allow(unsafe_code)]
pub(crate) fn stop() {
    // SAFETY: 引数なし。`.m` 側は専用キューへ同期ディスパッチして advertiser・browser・session を
    // 解放する（main queue は使わないため、呼び出し元がどのロックを持っていてもデッドロックしない。
    // ただし呼び出し元は Rust 側のロックを握らない契約）。
    unsafe { fandhe_apple_local_peer_stop() };
}

/// 相手からの受信 1 件の通知（`.m` の専用キューから呼ばれる）。受け付けて保存したら `true`
/// （`.m` は窓を閉じる）、検証に落ちた・古い・2 件目なら `false`（`.m` は待ち続ける）。
/// ポインタはこの呼び出しが戻るまでのみ有効なため、ここで同期的にコピーし切る。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_local_peer_offer(
    request_id: u64,
    peer_name: *const c_char,
    bytes: *const u8,
    len: usize,
) -> bool {
    if peer_name.is_null() || bytes.is_null() {
        return false;
    }
    // SAFETY: `peer_name` は非 NULL で、`.m` 側が NUL 終端 UTF-8 を渡す契約。呼び出しが戻るまで有効。
    let Ok(name) = unsafe { CStr::from_ptr(peer_name) }.to_str() else {
        return false;
    };
    // SAFETY: `bytes` は非 NULL で `len` バイト読める（`.m` が `NSData` の長さを渡す契約）。
    // 呼び出しが戻るまで有効で、`offer` 内で検証とコピーを済ませる。
    let data = unsafe { std::slice::from_raw_parts(bytes, len) };
    PENDING.offer(request_id, name, data)
}

/// 完了通知（`.m` の専用キューから 1 要求につき 1 回だけ呼ばれる）。
/// `code` の意味は `fandhe_device::local_peer::decode_code`。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_local_peer_finished(request_id: u64, code: i32) {
    finish(request_id, decode_code(code));
    // iOS では OS コールバックが結果を共有状態へ入れた直後に次フレームを起こす
    // （入力が無いと egui が描画せず画面が交換中のまま止まる、`device-bridge-contracts.md` 4 節）。
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
