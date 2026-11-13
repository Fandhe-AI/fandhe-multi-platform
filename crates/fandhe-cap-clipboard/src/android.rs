//! Android 向けクリップボード実装（`ClipboardManager`、Issue #859）。`driver` の `platform_clipboard` が
//! Android 経路で [`AndroidClipboard`] を返し、Java 側 `ClipboardHelper.request(Context, long, int, String)`（static）を
//! JNI で呼んで、`nativeOnClipboardResult` で返った値を [`decode_clipboard_result`] で契約の結果へ写す。
//!
//! `ClipboardManager` は Java 側の `Context` から取得する system service で、UI スレッドから触るのが Android の作法
//! （`getPrimaryClip` は Android 10 以降フォーカス状態に依存する）。一方 `android_main`（effect ドライバが走るスレッド）は
//! Java の UI スレッドとは別のネイティブスレッドであるため、Java が UI スレッドへ post して実行し、結果を JNI で返す往復にする。
//! 要求の追跡は [`RequestSlot`]（`fandhe-runtime`）で、待ちは [`CLIPBOARD_TIMEOUT`]（単調時計）で打ち切る。
//!
//! 結果コードの SSOT は Java 側 `ClipboardHelper` の `STATUS_*`。守る範囲は失敗を成功に畳まないこと・非テキストを `Ok(None)` に
//! 保つこと・Activity 再生成時の保留要求の破棄（[`reset_pending_tracking`]）、守らない範囲は他アプリが同時に内容を書き換えること。
//! クリップボードの内容・id・コードはログに出さない（security.md A09）。
//!
//! no-op スタブは置かない: Java の native は Rust の `ClipboardHelper.request` を経た後でしか呼ばれない。

use std::time::{Duration, Instant};

use crate::error::ClipboardError;

use fandhe_runtime::request_slot::{POLL_INTERVAL, RequestSlot};

/// クリップボード要求の種別（Java 側 `ClipboardHelper.OP_READ` / `OP_WRITE` と対）。
const CLIPBOARD_OP_READ: i32 = 0;
/// 同上（書き込み）。
const CLIPBOARD_OP_WRITE: i32 = 1;

/// JNI 往復の待ち上限。クリップボードは OS 内の即時操作で、UI スレッドへの post の待ちしか入らないため短く取る
/// （カメラの 300 秒と違いユーザー操作を待たない）。固着防止の安全弁であり、値自体に強い実測根拠はない
/// （ponytail: 実機で timeout の報告が出たら見直す）。
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(5);

/// Java から届く 1 件ぶんの結果（テキスト・状態コード）。
type Reply = (Option<String>, i32);

/// Java 側から届いたクリップボード結果を解釈する（`nativeOnClipboardResult` 本体）。
/// `status` の意味は Java 側 `ClipboardHelper.STATUS_*` と対で決める:
/// 0 = 成功（読み出しは `text` に内容、書き込みは `text` が null）、
/// 1 = クリップボードにテキストが無い（読み出しのみ。`Ok(None)`）、
/// 2 = `ClipboardManager` を取得できない等の内部エラー。それ以外は未知の失敗。
fn decode_clipboard_result(
    text: Option<String>,
    status: i32,
) -> Result<Option<String>, ClipboardError> {
    match status {
        0 => Ok(text),
        1 => Ok(None),
        _ => Err(ClipboardError::Internal("clipboard")),
    }
}

/// 要求 1 件を出して結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）。
/// 期限は呼び出しの開始（単調時計）から [`CLIPBOARD_TIMEOUT`]。差し替え・破棄された slot は他の要求のものなので消さない。
fn request_with(
    slot: &RequestSlot<Reply>,
    timeout: Duration,
    start: impl FnOnce(u64) -> bool,
) -> Result<Option<String>, ClipboardError> {
    let began = Instant::now();
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(ClipboardError::Internal("clipboard jni"));
    }
    loop {
        match slot.snapshot() {
            Some((cur, Some((text, status)))) if cur == id => {
                slot.clear(id);
                return decode_clipboard_result(text, status);
            }
            Some((cur, None)) if cur == id => {
                if began.elapsed() >= timeout {
                    slot.clear(id);
                    return Err(ClipboardError::Internal("clipboard timeout"));
                }
                std::thread::sleep(POLL_INTERVAL.min(timeout.saturating_sub(began.elapsed())));
            }
            _ => return Err(ClipboardError::Internal("clipboard")),
        }
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Reply> = RequestSlot::new();

/// Activity 再生成直後に gallery の `android_main` から（`driver::reset_pending_tracking` 経由で）呼ぶ。
/// 保留中の要求を捨て、待機中のワーカーを失敗で戻し、旧要求の遅延完了を id 不一致で無視させる。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    SLOT.reset();
}

/// Android 実クリップボード実装（`ClipboardManager`、Issue #859）。
#[cfg(target_os = "android")]
pub struct AndroidClipboard;

#[cfg(target_os = "android")]
impl AndroidClipboard {
    fn request(&self, op: i32, text: &str) -> Result<Option<String>, ClipboardError> {
        request_with(&SLOT, CLIPBOARD_TIMEOUT, |id| call_request(id, op, text))
    }
}

#[cfg(target_os = "android")]
impl crate::contract::Clipboard for AndroidClipboard {
    fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        self.request(CLIPBOARD_OP_READ, "")
    }

    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        self.request(CLIPBOARD_OP_WRITE, text).map(|_| ())
    }
}

/// `ClipboardHelper.request(Context, long, int, String)`（static）を JNI で呼ぶ。`false` は呼び出し失敗。
/// クラスは Activity の class loader で解決する（`fandhe_platform_android::with_activity_class`）。
/// Helper は状態を持たないため、生成前・破棄後を気にする必要はない。
#[cfg(target_os = "android")]
fn call_request(id: u64, op: i32, text: &str) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ClipboardHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            // クリップボードへ渡す文字列を Java String へ変換する（読み出し要求では空文字列を渡し、Java 側は `op` で無視する）。
            let j_text = env.new_string(text)?;
            env.call_static_method(
                class,
                jni::jni_str!("request"),
                jni::jni_sig!((android.content.Context, long, int, java.lang.String) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(id as jni::sys::jlong),
                    jni::objects::JValue::Int(op as jni::sys::jint),
                    jni::objects::JValue::Object(&j_text),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `ClipboardHelper.nativeOnClipboardResult`（Java 側）から呼ばれる JNI エクスポート関数（Issue #859）。
///
/// 受け取った内容は [`SLOT`] へ届けるだけで、ログ出力・保存は一切行わない（クリップボードは他アプリの
/// パスワード等が載り得る機密経路のため痕跡を残さない、security.md A09）。旧要求の遅延到着は `deliver` が
/// id 不一致で捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クラスは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ClipboardHelper_nativeOnClipboardResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    text: jni::objects::JString<'caller>,
    status: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        // `text` が null（書き込み要求の応答・テキスト不在）なら `None` にする。
        let value = if text.is_null() {
            None
        } else {
            // 変換失敗（不正な UTF-16 等）は内容を推測せず不在扱いにする
            // （security.md A03: 検証できない入力を通さない）。
            text.try_to_string(env).ok()
        };
        SLOT.deliver(request_id as u64, (value, status));
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_internal<T>(r: &Result<T, ClipboardError>) -> bool {
        matches!(r, Err(ClipboardError::Internal(_)))
    }

    #[test]
    fn decode_maps_status_codes_and_never_folds_failures_to_success() {
        assert_eq!(
            decode_clipboard_result(Some("a".into()), 0),
            Ok(Some("a".into()))
        );
        assert_eq!(decode_clipboard_result(None, 0), Ok(None));
        // テキストが無いのは異常ではない（非テキストは `Ok(None)` の契約）。
        assert_eq!(decode_clipboard_result(None, 1), Ok(None));
        for status in [2, 3, -1, 99] {
            assert!(
                is_internal(&decode_clipboard_result(None, status)),
                "status {status}"
            );
        }
    }

    #[test]
    fn request_fails_and_clears_when_start_fails() {
        let slot = RequestSlot::<Reply>::new();
        assert!(is_internal(&request_with(
            &slot,
            Duration::from_secs(1),
            |_| false
        )));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn request_returns_delivered_result_and_clears() {
        let slot = RequestSlot::<Reply>::new();
        let r = request_with(&slot, Duration::from_secs(30), |id| {
            slot.deliver(id, (Some("貼り付け".into()), 0));
            true
        });
        assert_eq!(r, Ok(Some("貼り付け".into())));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn request_times_out_and_clears() {
        let slot = RequestSlot::<Reply>::new();
        let r = request_with(&slot, Duration::from_millis(30), |_| true);
        assert_eq!(r, Err(ClipboardError::Internal("clipboard timeout")));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn request_ignores_other_ids_and_fails_after_reset() {
        let slot = RequestSlot::<Reply>::new();
        let r = request_with(&slot, Duration::from_secs(30), |id| {
            // 別 id の結果は受理されず、続く reset で待機が失敗で戻る。
            assert!(!slot.deliver(id + 1, (Some("x".into()), 0)));
            slot.reset();
            true
        });
        assert!(is_internal(&r));
    }
}
