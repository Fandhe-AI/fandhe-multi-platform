//! macOS 向け共有の受け取り（Issue #606）。`device/share_receive.rs::platform_share_receive` の
//! macOS 経路と `main.rs`（登録）から使われる。`macos/fandhe_macos_share_receive.m` が
//! サービスメニューの要求（`NSServices`）を受け、[`fandhe_rust_share_receive_received`] を
//! 呼ぶ（`build.rs` がコンパイルする）。
//!
//! 受け口 [`INBOX`] はプロセス全体の static で、UI ができる前（コールドスタート）に届いた
//! テキストもここに溜まる。検査・保持は共通側の `fandhe_device::share_receive::ShareInbox` に
//! 任せ、本モジュールは FFI の橋渡し（`unsafe` 2 箇所）だけを持つ。本文はログへ出さない。

use fandhe_device::share_receive::{ShareInbox, ShareReceive, ShareSnapshot};
use fandhe_device::DeviceError;

// SAFETY: `macos/fandhe_macos_share_receive.m` が定義する C ABI（`build.rs` が macOS で
// コンパイル・リンクする）。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    /// サービス provider を登録する。main thread から呼ぶ。冪等。
    fn fandhe_macos_share_receive_register();
}

/// 受信したテキストの受け口（プロセス全体で 1 つ）。
static INBOX: ShareInbox = ShareInbox::new();

/// provider を登録する。`run_native` より前に main thread から呼ぶ。
#[allow(unsafe_code)]
pub fn register() {
    // SAFETY: 引数なしの C 関数。main thread から呼ぶ契約は呼び出し元（`main`）が守る。
    unsafe { fandhe_macos_share_receive_register() }
}

/// 受信時刻（UNIX ms）。表示用なので壁時計でよい（経過時間の計測には使わない）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `.m` 側の provider から呼ばれる。`ptr` は UTF-8 バイト列（`len` バイト）で、呼び出し中
/// だけ有効。null または `len == 0` は「文字列が取れなかった/空」として拒否に数える。
/// 戻り値は 0 = 受理、1 = 拒否（`.m` 側がサービスへ固定文言のエラーを返す判断に使う）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_share_receive_received(ptr: *const u8, len: usize) -> i32 {
    let bytes = if ptr.is_null() || len == 0 {
        None
    } else {
        // SAFETY: `.m` 側が `NSData` の `bytes`/`length` を渡す契約で、ポインタは本呼び出しの
        // 間だけ有効。null と len 0 は上で除外済みで、検査・コピーまで呼び出し中に完結させる。
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    };
    match INBOX.receive_bytes(bytes, now_ms()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// macOS 向け `ShareReceive` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
pub(crate) struct MacosShareReceive;

impl ShareReceive for MacosShareReceive {
    fn snapshot(&self) -> Result<ShareSnapshot, DeviceError> {
        Ok(INBOX.snapshot())
    }
}
