//! Android 向け NFC タグ読み取り実装（Issue #1045、親 #157）。`device/nfc.rs::platform` の Android 経路から
//! [`implementation`] が呼ばれ、Java 側 `NfcReadHelper`（`NfcAdapter.enableReaderMode`）を JNI で橋渡しする。
//! 契約の SSOT は `fandhe_device::nfc`（写像表・`MAX_NDEF_BYTES`・`parse_ndef`）。
//!
//! 流れ:
//! - ワーカーが [`read_with`] で `MainActivity.requestNfcRead(long, int)` を呼び、Java が
//!   `nativeOnNfcReadResult(id, code, ndef)` で返した値を [`decode`] で契約の結果へ写す。
//!   NDEF のバイト列は境界を確かめる `parse_ndef` だけを通す
//! - 期限（呼び出し元から渡る残り時間、起点は単調時計）が切れたら `cancelNfcRead(long)` を呼び、
//!   Java が reader mode を解除した後の最終通知を待ってから `TimedOut` を返す
//!   （期限の後に届いた成功は成功にしない）
//!
//! 結果コードの SSOT は Java 側 `NfcReadHelper.CODE_*`。バイト数の上限の SSOT は Rust の
//! `MAX_NDEF_BYTES` で、Java へは要求ごとに渡し、JNI の受け口でもコピーの前に確かめる。
//!
//! 守る範囲: 1 プロセス内の要求順序・失敗を成功に畳まないこと・reader mode の解除・Activity 再生成時の
//! 保留中の要求の破棄（Java の `onDestroy` が `Interrupted` で確定し、[`reset_pending_tracking`] が待機中の
//! ワーカーを失敗で戻す）。守らない範囲: かざしたが届く前の離脱、2 枚同時にかざしたときの選択、
//! 他アプリ・OS が先にタグを処理した場合。タグの中身・UID・id・コードはログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestNfcRead` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（`android_scanner_devices` と同じ）。

use std::time::{Duration, Instant};

use fandhe_device::nfc::{parse_ndef, NfcReadFailure, NfcReadOutcome};

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 結果コード（SSOT は Java 側 `NfcReadHelper.CODE_*`）。
const CODE_OK: i32 = 0;
const CODE_EMPTY: i32 = 1;
const CODE_UNSUPPORTED: i32 = 2;
const CODE_DISABLED: i32 = 3;
const CODE_PERMISSION_DENIED: i32 = 4;
const CODE_UNREADABLE: i32 = 5;
const CODE_TOO_LARGE: i32 = 6;
const CODE_INTERRUPTED: i32 = 7;
const CODE_FAILED: i32 = 8;
const CODE_CANCELLED: i32 = 9;

/// Java から届く 1 件ぶんの結果（コード・NDEF のバイト列）。
type Reply = (i32, Option<Vec<u8>>);
type Outcome = Result<NfcReadOutcome, NfcReadFailure>;

/// 結果コードを契約の結果へ写す。OK でもバイト列が無ければ `Failed`、未知のコードと
/// `CODE_CANCELLED`（Rust が取り消した要求の最終通知）は `Failed`（嘘の成功にしない）。
fn decode(code: i32, ndef: Option<Vec<u8>>) -> Outcome {
    match code {
        CODE_OK => match ndef {
            Some(bytes) => parse_ndef(&bytes),
            None => Err(NfcReadFailure::Failed),
        },
        CODE_EMPTY => Ok(NfcReadOutcome::EmptyTag),
        CODE_UNSUPPORTED => Err(NfcReadFailure::Unsupported),
        CODE_DISABLED => Err(NfcReadFailure::Disabled),
        CODE_PERMISSION_DENIED => Err(NfcReadFailure::PermissionDenied),
        CODE_UNREADABLE => Err(NfcReadFailure::Unreadable),
        CODE_TOO_LARGE => Err(NfcReadFailure::TooLarge),
        CODE_INTERRUPTED => Err(NfcReadFailure::Interrupted),
        CODE_FAILED | CODE_CANCELLED => Err(NfcReadFailure::Failed),
        _ => Err(NfcReadFailure::Failed),
    }
}

/// 要求 1 件を出して結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）。
/// `remaining` を過ぎたら `cancel` を 1 回呼び、Java が解除を済ませた最終通知を待って `TimedOut` を返す。
/// reset や差し替えで slot が他の要求のものになったら、その slot は消さずに戻る。
fn read_with(
    slot: &RequestSlot<Reply>,
    remaining: Duration,
    start: impl FnOnce(u64) -> bool,
    cancel: impl FnOnce(u64) -> bool,
) -> Outcome {
    let begin = Instant::now();
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(NfcReadFailure::Failed);
    }
    let mut cancel = Some(cancel);
    let mut timed_out = false;
    loop {
        match slot.snapshot() {
            Some((cur, Some((code, ndef)))) if cur == id => {
                slot.clear(id);
                // 期限の後に届いた成功と取り消しの最終通知は TimedOut。それ以外の失敗は実際の理由を通す。
                let outcome = decode(code, ndef);
                return if timed_out && (code == CODE_CANCELLED || outcome.is_ok()) {
                    Err(NfcReadFailure::TimedOut)
                } else {
                    outcome
                };
            }
            Some((cur, None)) if cur == id => {
                if !timed_out && begin.elapsed() >= remaining {
                    timed_out = true;
                    if let Some(cancel) = cancel.take() {
                        // ponytail: JNI が失敗したら解除の完了を待てない（天井）。待たずに戻す。
                        if !cancel(id) {
                            slot.clear(id);
                            return Err(NfcReadFailure::TimedOut);
                        }
                    }
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            _ => {
                return Err(if timed_out {
                    NfcReadFailure::TimedOut
                } else {
                    NfcReadFailure::Failed
                })
            }
        }
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Reply> = RequestSlot::new();

/// Activity 再生成直後に `android::reset_pending_tracking` から呼ぶ。保留中の要求を捨て、
/// 待機中のワーカーを失敗で戻し、旧要求の遅延完了を id 不一致で無視させる。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    SLOT.reset();
}

/// Android の `NfcReader` 実装。
#[cfg(target_os = "android")]
struct AndroidNfcReader;

#[cfg(target_os = "android")]
impl fandhe_device::nfc::NfcReader for AndroidNfcReader {
    fn read_one(&self, remaining: Duration) -> Outcome {
        let max_bytes = fandhe_device::nfc::MAX_NDEF_BYTES.min(i32::MAX as usize) as i32;
        read_with(
            &SLOT,
            remaining,
            |id| call_start(id, max_bytes),
            call_cancel,
        )
    }
}

/// Android の実装を返す。`device/nfc.rs::platform` は android の cfg 内でだけ呼ぶ。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::nfc::NfcReader> {
    std::sync::Arc::new(AndroidNfcReader)
}

/// `MainActivity.requestNfcRead(long, int)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_scanner_devices.rs::call_start` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_start(id: u64, max_bytes: i32) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(
        |env: &mut jni::Env| -> Result<(), super::android::JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestNfcRead"),
                jni::jni_sig!((long, int) -> ()),
                &[
                    jni::objects::JValue::Long(id as jni::sys::jlong),
                    jni::objects::JValue::Int(max_bytes),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `MainActivity.cancelNfcRead(long)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `call_start` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_cancel(id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `call_start` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(
        |env: &mut jni::Env| -> Result<(), super::android::JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `call_start` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("cancelNfcRead"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(id as jni::sys::jlong)],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `NfcReadHelper.nativeOnNfcReadResult`（Java 側）から呼ばれる JNI エクスポート。
/// 旧要求の遅延到着は `deliver` が id 不一致で捨てる。NDEF は長さを `MAX_NDEF_BYTES` で検査してから
/// コピーし、超過は `CODE_TOO_LARGE` に差し替える。値はログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クラスは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_NfcReadHelper_nativeOnNfcReadResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    ndef: jni::objects::JByteArray<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let mut code = code;
        let bytes = if ndef.is_null() {
            None
        } else if ndef.len(env)? > fandhe_device::nfc::MAX_NDEF_BYTES {
            code = CODE_TOO_LARGE;
            None
        } else {
            Some(env.convert_byte_array(&ndef)?)
        };
        SLOT.deliver(request_id as u64, (code, bytes));
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

#[cfg(test)]
mod tests {
    use super::*;
    use NfcReadFailure as F;

    const SHORT: Duration = Duration::from_millis(50);
    const LONG: Duration = Duration::from_secs(30);

    /// Well-known `T`、lang `en`、本文 `hi` の NDEF メッセージ 1 件。
    fn text_ndef() -> Vec<u8> {
        vec![0xD1, 0x01, 0x05, b'T', 0x02, b'e', b'n', b'h', b'i']
    }

    fn ok_reply() -> Reply {
        (CODE_OK, Some(text_ndef()))
    }

    #[test]
    fn decode_maps_every_code_and_never_folds_to_success() {
        assert!(matches!(
            decode(CODE_OK, Some(text_ndef())),
            Ok(NfcReadOutcome::Read(_))
        ));
        assert_eq!(decode(CODE_EMPTY, None), Ok(NfcReadOutcome::EmptyTag));
        let cases = [
            (CODE_UNSUPPORTED, F::Unsupported),
            (CODE_DISABLED, F::Disabled),
            (CODE_PERMISSION_DENIED, F::PermissionDenied),
            (CODE_UNREADABLE, F::Unreadable),
            (CODE_TOO_LARGE, F::TooLarge),
            (CODE_INTERRUPTED, F::Interrupted),
            (CODE_FAILED, F::Failed),
            (CODE_CANCELLED, F::Failed),
            (10, F::Failed),
            (-1, F::Failed),
        ];
        for (code, want) in cases {
            assert_eq!(decode(code, None).err(), Some(want), "code {code}");
        }
    }

    #[test]
    fn decode_ok_requires_parsable_bytes() {
        assert_eq!(decode(CODE_OK, None).err(), Some(F::Failed));
        assert_eq!(
            decode(CODE_OK, Some(vec![0xFF, 0xFF])).err(),
            Some(F::Unreadable)
        );
    }

    #[test]
    fn read_fails_and_clears_when_start_fails() {
        let slot = RequestSlot::<Reply>::new();
        let r = read_with(&slot, LONG, |_| false, |_| true);
        assert_eq!(r.err(), Some(F::Failed));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn read_returns_delivered_result_and_clears() {
        let slot = RequestSlot::<Reply>::new();
        let r = read_with(
            &slot,
            LONG,
            |id| {
                slot.deliver(id, ok_reply());
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert!(matches!(r, Ok(NfcReadOutcome::Read(_))));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn read_ignores_other_ids_and_fails_after_reset() {
        let slot = RequestSlot::<Reply>::new();
        let r = read_with(
            &slot,
            LONG,
            |id| {
                assert!(!slot.deliver(id + 1, ok_reply()));
                slot.reset();
                true
            },
            |_| true,
        );
        assert_eq!(r.err(), Some(F::Failed));
    }

    #[test]
    fn deadline_cancels_once_and_waits_for_final_notice() {
        let slot = RequestSlot::<Reply>::new();
        let mut calls = 0;
        let r = read_with(
            &slot,
            SHORT,
            |_| true,
            |id| {
                calls += 1;
                slot.deliver(id, (CODE_CANCELLED, None));
                true
            },
        );
        assert_eq!(r.err(), Some(F::TimedOut));
        assert_eq!(calls, 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn success_after_deadline_is_not_accepted() {
        let slot = RequestSlot::<Reply>::new();
        let r = read_with(
            &slot,
            SHORT,
            |_| true,
            |id| {
                slot.deliver(id, ok_reply());
                true
            },
        );
        assert_eq!(r.err(), Some(F::TimedOut));
    }

    #[test]
    fn failure_after_deadline_keeps_real_reason() {
        let slot = RequestSlot::<Reply>::new();
        let r = read_with(
            &slot,
            SHORT,
            |_| true,
            |id| {
                slot.deliver(id, (CODE_FAILED, None));
                true
            },
        );
        assert_eq!(r.err(), Some(F::Failed));
    }

    #[test]
    fn cancel_call_failure_returns_timed_out_immediately() {
        let slot = RequestSlot::<Reply>::new();
        let r = read_with(&slot, SHORT, |_| true, |_| false);
        assert_eq!(r.err(), Some(F::TimedOut));
        assert!(slot.snapshot().is_none());
    }
}
