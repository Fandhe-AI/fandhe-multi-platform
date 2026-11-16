//! Android 向けディープリンク（App Links）受信（Issue #1279、親 #153）。
//! `driver::platform_deep_link` の Android 経路から使われ、Java 側
//! `DeepLinkHelper.deliver`（`DeepLinkLifecycle` が共通層の `deliverIntents` で起動時と `onNewIntent` を配る。
//! scheme が https の `ACTION_VIEW` だけを扱う）が `Java_com_fandhe_platform_DeepLinkHelper_nativeOnDeepLinkReceived`
//! を呼ぶ。
//!
//! 受け口 [`INBOX`] はプロセス全体の static で、UI ができる前（コールドスタート）に届いた URL も
//! ここに溜まる。検査・保持は `crate::contract::receive` に一本化し、本モジュールは
//! JNI の橋渡しだけを持つ。URL は秘密値を含みうるためログへ出さない。
//!
//! 許可ホストはビルド時の環境変数 `FANDHE_ANDROID_DEEP_LINK_HOST`（`option_env!`。Gradle の
//! manifest 側も同じ変数を読む）。値は本リポジトリに書かない。未設定・不正な形式のときは
//! `validate_host` に通らず、届いた URL を全件拒否する（件数だけ数える。成功を装わない）。
//! feature `device` 無効ビルド向けの no-op は `fandhe-gallery` の `lib.rs` に置く。JNI 署名は Java の native 宣言・no-op スタブ・
//! 本ファイルの 3 か所で `(String) -> void` に一致させる。

use crate::contract::{self, DeepLink};
use crate::error::DeepLinkError;
use fandhe_device::url_scheme::{InboxSnapshot, UrlInbox};

/// 受信した URL の受け口（プロセス全体で 1 つ）。
static INBOX: UrlInbox = UrlInbox::new();

/// 許可ホスト（ビルド時に渡す）。形式検査の正は `deep_link::validate_host`。
#[cfg(target_os = "android")]
const HOST: Option<&str> = option_env!("FANDHE_ANDROID_DEEP_LINK_HOST");

/// 受信時刻（UNIX ms）。表示用の時刻で、エポック前なら 0。
#[cfg(target_os = "android")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 検査して記録する。拒否は件数に反映済みなので戻り値は捨てる（URL をログに出さない）。
#[cfg(any(target_os = "android", test))]
fn record(inbox: &UrlInbox, url: Option<&str>, host: Option<&str>, now_ms: u64) {
    let _ = contract::receive(inbox, url.map(str::as_bytes), host.unwrap_or(""), now_ms);
}

/// `DeepLinkHelper.nativeOnDeepLinkReceived(String)` の実体。`url` が null または
/// 文字列へ変換できない場合は `None` として拒否件数に数える。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、`nativeOnUrlSchemeReceived` と同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_DeepLinkHelper_nativeOnDeepLinkReceived<'caller>(
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
        record(&INBOX, value.as_deref(), HOST, now_ms());
        Ok(())
    });
    let _ = outcome.into_outcome();
}

/// Android 向け `DeepLink` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
struct AndroidDeepLink;

impl DeepLink for AndroidDeepLink {
    fn snapshot(&self) -> Result<InboxSnapshot, DeepLinkError> {
        Ok(INBOX.snapshot())
    }
}

/// Android の実装を返す。
pub(crate) fn implementation() -> &'static dyn DeepLink {
    &AndroidDeepLink
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://example.com/x";

    #[test]
    fn no_host_rejects_everything() {
        let inbox = UrlInbox::new();
        record(&inbox, Some(URL), None, 1);
        let s = inbox.snapshot();
        assert!(s.last.is_none());
        assert_eq!(s.rejected, 1);
    }

    #[test]
    fn allowed_host_is_received_and_others_rejected() {
        let inbox = UrlInbox::new();
        record(&inbox, Some(URL), Some("example.com"), 1);
        record(
            &inbox,
            Some("https://example.org/x"),
            Some("example.com"),
            2,
        );
        record(&inbox, Some("http://example.com/x"), Some("example.com"), 3);
        record(&inbox, None, Some("example.com"), 4);
        let s = inbox.snapshot();
        assert_eq!(s.last.map(|l| l.url), Some(URL.to_string()));
        assert_eq!(s.rejected, 3);
    }

    #[test]
    fn malformed_host_rejects_everything() {
        let inbox = UrlInbox::new();
        record(&inbox, Some(URL), Some("Example.com"), 1);
        let s = inbox.snapshot();
        assert!(s.last.is_none());
        assert_eq!(s.rejected, 1);
    }
}
