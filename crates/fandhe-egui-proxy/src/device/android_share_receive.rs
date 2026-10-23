//! Android 向け共有の受け取り（Issue #607、親 #122）。`device/share_receive.rs::
//! platform_share_receive` の Android 経路から呼ばれる。
//!
//! 他アプリの共有シート（`ACTION_SEND` + `text/plain` + `EXTRA_TEXT`）から届いたテキストを、
//! Java 側 `ShareReceiveHelper.deliver`（`MainActivity` の `onCreate` / `onNewIntent` から
//! 呼ばれる）が [`Java_com_fandhe_androidapp_ShareReceiveHelper_nativeOnShareReceived`] 経由で
//! 渡す。受け口 [`INBOX`] はプロセス全体の static で、UI ができる前（コールドスタート）に
//! 届いた分もここに溜まる。検査・保持は共通層 `fandhe_device::share_receive::ShareInbox` に
//! 一本化し、本モジュールは JNI の橋渡しだけを持つ。本文は秘密値を含みうるためログへ出さない。
//! feature `device` 無効ビルド向けの no-op は `crate::android` に置く。

use fandhe_device::share_receive::{ShareInbox, ShareReceive, ShareSnapshot};
use fandhe_device::DeviceError;

/// 受信した共有テキストの受け口（プロセス全体で 1 つ）。
static INBOX: ShareInbox = ShareInbox::new();

/// 受信時刻（UNIX ms）。エポック前なら 0。表示用の壁時計で、経過時間の計測には使わない。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 受け口へ 1 件渡す。`None`（null・変換失敗・Java 側で拒否した入力）は拒否として数える。
/// `Err` は件数に反映済みで、本文を出さないため握って捨てる。
fn deliver(inbox: &ShareInbox, text: Option<&str>, now_ms: u64) {
    let _ = inbox.receive_bytes(text.map(str::as_bytes), now_ms);
}

/// `ShareReceiveHelper.nativeOnShareReceived(String)` の実体。`text` が null または
/// 文字列へ変換できない場合は `None` として拒否件数に数える（検査は `ShareInbox` に一本化）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、`nativeOnUrlSchemeReceived` と同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_ShareReceiveHelper_nativeOnShareReceived<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    text: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let value = if text.is_null() {
            None
        } else {
            text.try_to_string(env).ok()
        };
        deliver(&INBOX, value.as_deref(), now_ms());
        Ok(())
    });
    let _ = outcome.into_outcome();
}

/// Android 向け `ShareReceive` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
pub(crate) struct AndroidShareReceive;

impl ShareReceive for AndroidShareReceive {
    fn snapshot(&self) -> Result<ShareSnapshot, DeviceError> {
        Ok(INBOX.snapshot())
    }
}

/// Android の実装を返す。
pub(crate) fn implementation() -> &'static dyn ShareReceive {
    &AndroidShareReceive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 受け取った本文と時刻が入る() {
        let inbox = ShareInbox::new();
        deliver(&inbox, Some("hi"), 5);
        let snap = inbox.snapshot();
        let last = snap.last.expect("受け取った本文が入る");
        assert_eq!(last.text, "hi");
        assert_eq!(last.received_at_unix_ms, 5);
    }

    #[test]
    fn nullは拒否として数え直前の値を変えない() {
        let inbox = ShareInbox::new();
        deliver(&inbox, Some("a"), 1);
        let before = inbox.snapshot();
        deliver(&inbox, None, 2);
        let after = inbox.snapshot();
        assert_eq!(after.rejected, before.rejected + 1);
        assert_eq!(after.seq, before.seq + 1);
        assert_eq!(after.last, before.last);
    }

    #[test]
    fn 実装はunsupportedを返さない() {
        assert!(implementation().snapshot().is_ok());
    }
}
