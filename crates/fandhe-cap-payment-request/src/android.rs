//! Issue #1131（親 #114）: Android 向け決済シートの実装。`driver.rs` の
//! `drive_callback`（Android 経路）から呼ばれ、Google Pay API（`play-services-wallet`、環境は TEST 固定）
//! で「使えるか」の確認と固定 1 円の決済シートを出す。Java 側は `PaymentRequestHelper`。
//!
//! 流れ: `begin(id, action)`（UI スレッド）→ JNI `PaymentRequestHelper.request(activity, id, pay, isReadyJson,
//! paymentDataJson)` → Java が結果の種別を `nativeOnCheckResult` / `nativeOnPayResult` で返す →
//! [`decode_check`] / [`decode_pay`] で契約の結果へ写像 → `driver.rs` の `bridge::deliver_*`。
//! 要求 JSON は `crate::contract::ANDROID_*_JSON` を JNI で渡す（Java に複製しない）。
//!
//! 写像（SSOT は `contract.rs`）: GMS なし・Pay 時に `isReadyToPay` が偽は `Unsupported`、
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
//! feature `device` 無効ビルドではその経路に到達しない（translation と同じ）。

use crate::contract::{PaymentOutcome, outcome_from_android_result};
use crate::error::PaymentRequestError;

/// 結果の種別（SSOT は Java 側 `PaymentRequestHelper.KIND_*`）。
const KIND_READY: i32 = 0;
const KIND_NOT_READY: i32 = 1;
const KIND_ACTIVITY_RESULT: i32 = 2;
const KIND_UNSUPPORTED: i32 = 3;
// KIND_FAILED（4）と未知の値は、写像の既定（失敗）に入るため定数にしない。

/// 「使えるか」の結果を契約の結果へ写す。`Ok(true)` になるのは `KIND_READY` だけ。
fn decode_check(kind: i32) -> Result<bool, PaymentRequestError> {
    match kind {
        KIND_READY => Ok(true),
        KIND_NOT_READY => Ok(false),
        KIND_UNSUPPORTED => Err(PaymentRequestError::Unsupported),
        // KIND_FAILED・Check に来ないはずの種別・未知の値。
        _ => Err(PaymentRequestError::Internal("payment_request failed")),
    }
}

/// 決済シートの結果を契約の結果へ写す。`TestAuthorized` になるのは
/// `KIND_ACTIVITY_RESULT` かつ `(RESULT_OK, データあり)` のときだけ。
fn decode_pay(
    kind: i32,
    result_code: i32,
    has_payment_data: bool,
) -> Result<PaymentOutcome, PaymentRequestError> {
    match kind {
        KIND_ACTIVITY_RESULT => Ok(outcome_from_android_result(result_code, has_payment_data)),
        KIND_UNSUPPORTED => Err(PaymentRequestError::Unsupported),
        // KIND_FAILED・Pay に来ないはずの種別・未知の値。
        _ => Ok(PaymentOutcome::Failed),
    }
}

/// OS へ依頼する。`Ok` は受理済みで結果は後から `bridge::deliver_*` で届く。`Err` は呼び出し自体の
/// 失敗で、呼び出し元がその場で結果を置く。
#[cfg(target_os = "android")]
/// `pay` が `true` なら決済シート、`false` なら「使えるか」の確認（画面の型は driver.rs で写す）。
pub(crate) fn begin(request_id: u64, pay: bool) -> Result<(), PaymentRequestError> {
    use crate::contract::{ANDROID_IS_READY_TO_PAY_JSON, ANDROID_PAYMENT_DATA_REQUEST_JSON};
    let payment_json = pay.then_some(ANDROID_PAYMENT_DATA_REQUEST_JSON);
    if call_request(request_id, pay, ANDROID_IS_READY_TO_PAY_JSON, payment_json) {
        Ok(())
    } else {
        Err(PaymentRequestError::Internal("payment_request failed"))
    }
}

/// ホストのテストビルド用。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn begin(_request_id: u64, _pay: bool) -> Result<(), PaymentRequestError> {
    Err(PaymentRequestError::Unsupported)
}

/// `PaymentRequestHelper.request(Activity, long, boolean, String, String)`（static）を JNI で呼ぶ。
/// 戻り値は Helper が要求を受理したか。`false`（呼び出し失敗・Helper なし＝生成前・破棄後）は呼び出し元が
/// 失敗で確定する（結果が来ないまま待たせない）。クラスは Activity の class loader で解決する。
#[cfg(target_os = "android")]
fn call_request(id: u64, pay: bool, is_ready_json: &str, payment_json: Option<&str>) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.PaymentRequestHelper"),
        |env, activity, class| -> Result<bool, JniCallFailed> {
            let j_ready = env.new_string(is_ready_json)?;
            let j_payment = match payment_json {
                Some(s) => Some(env.new_string(s)?),
                None => None,
            };
            let null = jni::objects::JObject::null();
            let accepted = env.call_static_method(
                class,
                jni::jni_str!("request"),
                jni::jni_sig!((android.app.Activity, long, boolean, java.lang.String, java.lang.String) -> boolean),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(id as jni::sys::jlong),
                    jni::objects::JValue::Bool(pay),
                    jni::objects::JValue::Object(&j_ready),
                    jni::objects::JValue::Object(match &j_payment {
                        Some(s) => s,
                        None => &null,
                    }),
                ],
            )?;
            Ok(accepted.z()?)
        },
    )
    .unwrap_or(false)
}

/// `PaymentRequestHelper.nativeOnCheckResult`（Java 側）から呼ばれる JNI エクスポート。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_PaymentRequestHelper_nativeOnCheckResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
) {
    let _ = unowned_env.with_env(|_env| -> Result<(), jni::errors::Error> {
        crate::driver::bridge::deliver_check(request_id as u64, decode_check(kind));
        Ok(())
    });
}

/// `PaymentRequestHelper.nativeOnPayResult`（Java 側）から呼ばれる JNI エクスポート。
/// 決済データの中身は受け取らない（有無だけ）。
///
/// # Safety
/// [`Java_com_fandhe_platform_PaymentRequestHelper_nativeOnCheckResult`] と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_PaymentRequestHelper_nativeOnPayResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
    result_code: jni::sys::jint,
    has_payment_data: jni::sys::jboolean,
) {
    let _ = unowned_env.with_env(|_env| -> Result<(), jni::errors::Error> {
        crate::driver::bridge::deliver_pay(
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
        for pay in [false, true] {
            assert!(matches!(
                begin(1, pay),
                Err(PaymentRequestError::Unsupported)
            ));
        }
    }

    #[test]
    fn decode_check_maps_kinds() {
        assert!(matches!(decode_check(KIND_READY), Ok(true)));
        assert!(matches!(decode_check(KIND_NOT_READY), Ok(false)));
        assert!(matches!(
            decode_check(KIND_UNSUPPORTED),
            Err(PaymentRequestError::Unsupported)
        ));
        for k in [KIND_ACTIVITY_RESULT, 4, -1, 99, i32::MIN, i32::MAX] {
            assert!(matches!(
                decode_check(k),
                Err(PaymentRequestError::Internal("payment_request failed"))
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
            Err(PaymentRequestError::Unsupported)
        ));
        for k in [KIND_READY, KIND_NOT_READY, 4, 99, -1] {
            assert!(matches!(
                decode_pay(k, -1, true),
                Ok(PaymentOutcome::Failed)
            ));
        }
    }
}
