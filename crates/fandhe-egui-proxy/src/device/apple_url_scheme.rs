//! macOS / iOS 向けカスタム URL scheme 受信（Issue #1215・#1217）。`device/url_scheme.rs::
//! platform_url_scheme` の macOS・iOS 分岐から使われる。macOS は
//! `macos/fandhe_macos_url_scheme.m` が登録する Apple Event `kAEGetURL` ハンドラ、iOS は
//! `ios/fandhe_ios_url_scheme.m` が差し込む delegate の `application:openURL:options:`
//! と、起動通知の `launchOptions`（コールドスタート）から
//! [`fandhe_rust_url_scheme_received`] が呼ばれる（いずれも `build.rs` がコンパイルする）。
//!
//! iOS は scene 形態のため URL・ホーム画面ショートカット（Issue #425）とも
//! `ios/fandhe_ios_scene.m` の通知経由で届く。
//!
//! 受け口 [`INBOX`] はプロセス全体の static で、UI ができる前（コールドスタート）に
//! 届いた URL もここに溜まる。検査・保持は共通側の `fandhe_device::url_scheme::UrlInbox`
//! に任せ、本モジュールは FFI の橋渡し（`unsafe` 2 箇所）だけを持つ。
//! URL は秘密値を含みうるためログへ出さない。

use fandhe_device::url_scheme::{InboxSnapshot, UrlInbox, UrlScheme, SCHEME};
use fandhe_device::DeviceError;

// SAFETY: `macos/fandhe_macos_url_scheme.m` / `ios/fandhe_ios_url_scheme.m` のどちらかが
// 定義する C ABI（`build.rs` が OS ごとにコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    /// 受信ハンドラ（macOS: Apple Event、iOS: 起動通知の observer）を登録する。
    /// main thread から呼ぶ。冪等。
    fn fandhe_apple_url_scheme_register();
}

/// 受信した URL の受け口（プロセス全体で 1 つ）。
static INBOX: UrlInbox = UrlInbox::new();

/// 受信ハンドラを登録する。イベントループ開始前（macOS は `run_native` より前、iOS は
/// `UIApplicationMain` より前）に main thread から呼ぶ。起動時の URL に間に合わせるため
/// `.m` 側が起動通知（macOS: WillFinishLaunching、iOS: DidFinishLaunching）も使う。
#[allow(unsafe_code)]
pub fn register() {
    // SAFETY: 引数なしの C 関数。main thread から呼ぶ契約は呼び出し元（`main`）が守る。
    unsafe { fandhe_apple_url_scheme_register() }
}

/// 受信時刻（UNIX ms）。エポック前なら 0。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `.m` 側のハンドラから呼ばれる。`ptr` は UTF-8 バイト列（`len` バイト）で、呼び出し中
/// だけ有効。null または `len == 0` は「文字列が取れなかった/空」として拒否に数える。
/// 戻り値の `Err` は件数に反映済みのため握って捨てる（ログに URL を出さない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_url_scheme_received(ptr: *const u8, len: usize) {
    let bytes = if ptr.is_null() || len == 0 {
        None
    } else {
        // SAFETY: `.m` 側が `NSData` の `bytes`/`length` を渡す契約で、ポインタは本呼び出しの
        // 間だけ有効。null と len 0 は上で除外済みで、ここでは即座にコピーせず検査まで
        // 呼び出し中に完結させる。
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    };
    let _ = INBOX.receive_bytes(bytes, SCHEME, now_ms());
    // iOS は入力がないと egui が描画しないため、受信直後に次フレームを起こす（契約表 4）。
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}

/// macOS / iOS 向け `UrlScheme` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
pub(crate) struct AppleUrlScheme;

impl UrlScheme for AppleUrlScheme {
    fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
        Ok(INBOX.snapshot())
    }
}
