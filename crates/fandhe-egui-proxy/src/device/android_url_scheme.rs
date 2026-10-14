//! Android 向けカスタム URL scheme 受信（Issue #1218）。`device/url_scheme.rs::
//! platform_url_scheme` の Android 分岐から使われ、Java 側 `MainActivity` の
//! `deliverUrlIntent`（`onCreate` の `getIntent()` / `onNewIntent`）が
//! [`Java_com_fandhe_androidapp_MainActivity_nativeOnUrlSchemeReceived`] を呼ぶ。
//!
//! 受け口 [`INBOX`] はプロセス全体の static で、UI ができる前（コールドスタート）に
//! 届いた URL もここに溜まる。検査・保持は共通側の `fandhe_device::url_scheme::UrlInbox`
//! に任せ、本モジュールは JNI の橋渡しだけを持つ。URL は秘密値を含みうるためログへ出さない。
//! feature `device` 無効ビルド向けの no-op は `crate::android` に置く。

use fandhe_device::url_scheme::{InboxSnapshot, UrlInbox, UrlScheme, SCHEME};
use fandhe_device::DeviceError;

/// 受信した URL の受け口（プロセス全体で 1 つ）。
static INBOX: UrlInbox = UrlInbox::new();

/// 受信時刻（UNIX ms）。エポック前なら 0。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `MainActivity.nativeOnUrlSchemeReceived(String)` の実体。`url` が null または
/// 文字列へ変換できない場合は `None` として拒否件数に数える（検査は `UrlInbox` に一本化）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、`nativeOnClipboardResult` と同じ根拠。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnUrlSchemeReceived<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    url: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let value = if url.is_null() {
            None
        } else {
            url.try_to_string(env).ok()
        };
        // `Err`（拒否）は件数に反映済み。URL をログに出さないため握って捨てる。
        let _ = INBOX.receive_bytes(value.as_deref().map(str::as_bytes), SCHEME, now_ms());
        Ok(())
    });
    let _ = outcome.into_outcome();
}

/// Android 向け `UrlScheme` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
pub(crate) struct AndroidUrlScheme;

impl UrlScheme for AndroidUrlScheme {
    fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
        Ok(INBOX.snapshot())
    }
}
