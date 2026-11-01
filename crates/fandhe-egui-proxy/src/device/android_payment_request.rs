//! Issue #1131（親 #114）: Android 向け決済シートの実装。`device/payment_request.rs` の
//! `drive_callback`（Android 経路）から呼ばれ、Google Pay API（`play-services-wallet`、環境は TEST 固定）
//! で「使えるか」の確認と固定 1 円の決済シートを出す。Java 側は `PaymentRequestHelper`。
//!
//! 流れ: `begin(id, action)`（UI スレッド）→ JNI `MainActivity.requestPayment(id, pay, isReadyJson,
//! paymentDataJson)` → Java が結果の種別を `nativeOnCheckResult` / `nativeOnPayResult` で返す →
//! [`decode_check`] / [`decode_pay`] で契約の結果へ写像 → `payment_request::bridge::deliver_*`。
//! 要求 JSON は `fandhe_device::payment_request::ANDROID_*_JSON` を JNI で渡す（Java に複製しない）。
//!
//! 写像（SSOT は `fandhe_device::payment_request`）: GMS なし・Pay 時に `isReadyToPay` が偽は `Unsupported`、
//! Check の `isReadyToPay` は真が `Ok(true)`・偽が `Ok(false)`、Task 失敗は失敗（「使えない」へ畳まない）。
//! 決済シートの結果は `outcome_from_android_result`（`RESULT_OK` かつデータありだけが
//! `TestAuthorized`、キャンセルは `Cancelled`）。返った決済データは読まず捨てる。
//!
//! ライフサイクル: Activity 再生成（`onDestroy`）で Java が追跡を外し、結果は送らない。Rust 側は
//! `reset_pending_tracking()` で slot を捨てる。`RequestSlot` の id は単調増加のため、旧結果は
//! 新しい要求を消費しない。期限は掛けない（利用者操作の UI）。
//!
//! 守る範囲: 1 プロセス内の要求と完了の順序・嘘の成功の禁止・課金できるトークンを作らないこと。
//! 守らない範囲: プロセス死をまたぐ結果・GMS の Task が返らない場合の処理中表示・複数プロセス。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestPayment` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（android_translation と同じ）。

use fandhe_core::screens::payment_request::PaymentAction;
use fandhe_device::payment_request::{outcome_from_android_result, PaymentOutcome};
use fandhe_device::DeviceError;

/// 結果の種別（SSOT は Java 側 `PaymentRequestHelper.KIND_*`）。
const KIND_READY: i32 = 0;
const KIND_NOT_READY: i32 = 1;
const KIND_ACTIVITY_RESULT: i32 = 2;
const KIND_UNSUPPORTED: i32 = 3;
// KIND_FAILED（4）と未知の値は、写像の既定（失敗）に入るため定数にしない。

/// 「使えるか」の結果を契約の結果へ写す。`Ok(true)` になるのは `KIND_READY` だけ。
fn decode_check(kind: i32) -> Result<bool, DeviceError> {
    match kind {
        KIND_READY => Ok(true),
        KIND_NOT_READY => Ok(false),
        KIND_UNSUPPORTED => Err(DeviceError::Unsupported("payment_request")),
        // KIND_FAILED・Check に来ないはずの種別・未知の値。
        _ => Err(DeviceError::Internal("payment_request failed")),
    }
}

/// 決済シートの結果を契約の結果へ写す。`TestAuthorized` になるのは
/// `KIND_ACTIVITY_RESULT` かつ `(RESULT_OK, データあり)` のときだけ。
fn decode_pay(
    kind: i32,
    result_code: i32,
    has_payment_data: bool,
) -> Result<PaymentOutcome, DeviceError> {
    match kind {
        KIND_ACTIVITY_RESULT => Ok(outcome_from_android_result(result_code, has_payment_data)),
        KIND_UNSUPPORTED => Err(DeviceError::Unsupported("payment_request")),
        // KIND_FAILED・Pay に来ないはずの種別・未知の値。
        _ => Ok(PaymentOutcome::Failed),
    }
}

/// OS へ依頼する。`Ok` は受理済みで結果は後から `bridge::deliver_*` で届く。`Err` は呼び出し自体の
/// 失敗で、呼び出し元がその場で結果を置く。
#[cfg(target_os = "android")]
pub(crate) fn begin(request_id: u64, action: PaymentAction) -> Result<(), DeviceError> {
    use fandhe_device::payment_request::{
        ANDROID_IS_READY_TO_PAY_JSON, ANDROID_PAYMENT_DATA_REQUEST_JSON,
    };
    let pay = matches!(action, PaymentAction::Pay);
    let payment_json = pay.then_some(ANDROID_PAYMENT_DATA_REQUEST_JSON);
    if call_request(request_id, pay, ANDROID_IS_READY_TO_PAY_JSON, payment_json) {
        Ok(())
    } else {
        Err(DeviceError::Internal("payment_request failed"))
    }
}

/// ホストのテストビルド用。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn begin(_request_id: u64, _action: PaymentAction) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("payment_request"))
}

/// `MainActivity.requestPayment(long, boolean, String, String)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `device/android_translation.rs::call_request` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_request(id: u64, pay: bool, is_ready_json: &str, payment_json: Option<&str>) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_ready = env.new_string(is_ready_json)?;
        let j_payment = match payment_json {
            Some(s) => Some(env.new_string(s)?),
            None => None,
        };
        let null = jni::objects::JObject::null();
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestPayment"),
            jni::jni_sig!((long, boolean, java.lang.String, java.lang.String) -> ()),
            &[
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Bool(pay),
                jni::objects::JValue::Object(&j_ready),
                jni::objects::JValue::Object(match &j_payment {
                    Some(s) => s,
                    None => &null,
                }),
            ],
        )?;
        Ok(())
    })
    .is_ok()
}

/// `PaymentRequestHelper.nativeOnCheckResult`（Java 側）から呼ばれる JNI エクスポート。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PaymentRequestHelper_nativeOnCheckResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
) {
    let _ = unowned_env.with_env(|_env| -> Result<(), jni::errors::Error> {
        super::payment_request::bridge::deliver_check(request_id as u64, decode_check(kind));
        Ok(())
    });
}

/// `PaymentRequestHelper.nativeOnPayResult`（Java 側）から呼ばれる JNI エクスポート。
/// 決済データの中身は受け取らない（有無だけ）。
///
/// # Safety
/// [`Java_com_fandhe_androidapp_PaymentRequestHelper_nativeOnCheckResult`] と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PaymentRequestHelper_nativeOnPayResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
    result_code: jni::sys::jint,
    has_payment_data: jni::sys::jboolean,
) {
    let _ = unowned_env.with_env(|_env| -> Result<(), jni::errors::Error> {
        super::payment_request::bridge::deliver_pay(
            request_id as u64,
            decode_pay(kind, result_code, has_payment_data),
        );
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [i32; 9] = [-1, 0, 1, 2, 3, 4, 5, i32::MIN, i32::MAX];

    #[test]
    fn host_begin_never_succeeds() {
        for a in [PaymentAction::Check, PaymentAction::Pay] {
            assert!(matches!(
                begin(1, a),
                Err(DeviceError::Unsupported("payment_request"))
            ));
        }
    }

    #[test]
    fn decode_check_maps_kinds() {
        assert!(matches!(decode_check(KIND_READY), Ok(true)));
        assert!(matches!(decode_check(KIND_NOT_READY), Ok(false)));
        assert!(matches!(
            decode_check(KIND_UNSUPPORTED),
            Err(DeviceError::Unsupported("payment_request"))
        ));
        for k in [KIND_ACTIVITY_RESULT, 4, -1, 99, i32::MIN, i32::MAX] {
            assert!(matches!(
                decode_check(k),
                Err(DeviceError::Internal("payment_request failed"))
            ));
        }
    }

    #[test]
    fn decode_pay_authorizes_only_ok_with_data() {
        for k in KINDS {
            for code in [-1, 0, 1, 2, i32::MIN, i32::MAX] {
                for has in [false, true] {
                    let authorized =
                        matches!(decode_pay(k, code, has), Ok(PaymentOutcome::TestAuthorized));
                    assert_eq!(
                        authorized,
                        k == KIND_ACTIVITY_RESULT && code == -1 && has,
                        "kind={k} code={code} has={has}"
                    );
                }
            }
        }
    }

    #[test]
    fn decode_pay_maps_cancel_unsupported_and_failure() {
        assert!(matches!(
            decode_pay(KIND_ACTIVITY_RESULT, 0, false),
            Ok(PaymentOutcome::Cancelled)
        ));
        assert!(matches!(
            decode_pay(KIND_ACTIVITY_RESULT, -1, false),
            Ok(PaymentOutcome::Failed)
        ));
        assert!(matches!(
            decode_pay(KIND_UNSUPPORTED, 0, false),
            Err(DeviceError::Unsupported("payment_request"))
        ));
        for k in [KIND_READY, KIND_NOT_READY, 4, 99, -1] {
            assert!(matches!(
                decode_pay(k, -1, true),
                Ok(PaymentOutcome::Failed)
            ));
        }
    }
}
