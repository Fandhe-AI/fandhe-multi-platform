//! Android 向けライブアクティビティ実装（Issue #296、親 #88）。`device/live_activity.rs` の
//! `platform_live_activity` android 経路から呼ばれる。Java 側 `LiveActivityHelper` が
//! `NotificationCompat` の進行中通知（`setOngoing(true)` + `setProgress`）を固定 tag/ID で 1 件
//! 出し、開始・更新・終了を行う。Android 16 の Live Updates（`ProgressStyle`）は範囲外。
//!
//! 流れ: `submit`（UI スレッド）→ JNI `MainActivity.requestLiveActivity(id, op, value)` → Java が
//! 結果コードを `nativeOnLiveActivityResult(id, op, code)` で返す → [`decode`] で契約の結果へ
//! 写像 → `live_activity::bridge::deliver` が `SLOT` の id と照合して置く。
//!
//! 結果コード: 0 Started / 1 Updated / 2 Ended / 3 Dismissed / 4 Unsupported（API 24〜25）/
//! 5 PermissionDenied（未許可・通知オフ・channel ブロック）/ 6 Failed。操作と結果の組み合わせが
//! 合わないもの（更新への `Started` 等）は写像ではじかず、共通コアの `apply_result` が失敗として見せる。
//!
//! 守る範囲: 1 プロセス内の要求順序（main looper 1 本）・拒否/通知オフを成功に畳まないこと・
//! 閉じられた通知を再投稿しないこと・再生成時と起動時の通知の解放。守らない範囲: 利用者が閉じて
//! から次の操作までの画面表示の食い違い、OEM 独自の表示規則。`Cancelled` は出さない。
//!
//! 再生成時: `android_main` → `reset_pending_tracking` → `SLOT.reset()` + `dismiss_all`
//! （通知を cancel）。旧インスタンスの結果は id 照合で捨てられる。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestLiveActivity` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（app-badge と同じ）。

use std::sync::Arc;

#[cfg(not(target_os = "android"))]
use fandhe_device::live_activity::UnsupportedLiveActivity;
use fandhe_device::live_activity::{LiveActivity, LiveOutcome};
use fandhe_device::DeviceError;

/// 操作コード（SSOT は Java 側 `LiveActivityHelper.OP_*`）。
const OP_START: i32 = 0;
const OP_UPDATE: i32 = 1;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const OP_END: i32 = 2;

/// 結果コード（SSOT は Java 側 `LiveActivityHelper.RESULT_*`）。
const RESULT_STARTED: i32 = 0;
const RESULT_UPDATED: i32 = 1;
const RESULT_ENDED: i32 = 2;
const RESULT_DISMISSED: i32 = 3;
const RESULT_UNSUPPORTED: i32 = 4;
const RESULT_PERMISSION_DENIED: i32 = 5;

/// Java の結果コードを契約の結果へ写像する。失敗・未知・負値は操作名つきの `Internal`。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn decode(op: i32, code: i32) -> Result<LiveOutcome, DeviceError> {
    match code {
        RESULT_STARTED => Ok(LiveOutcome::Started),
        RESULT_UPDATED => Ok(LiveOutcome::Updated),
        RESULT_ENDED => Ok(LiveOutcome::Ended),
        RESULT_DISMISSED => Ok(LiveOutcome::Dismissed),
        RESULT_UNSUPPORTED => Err(DeviceError::Unsupported("live_activity")),
        RESULT_PERMISSION_DENIED => Err(DeviceError::PermissionDenied("live_activity")),
        _ => Err(DeviceError::Internal(match op {
            OP_START => "live_activity start",
            OP_UPDATE => "live_activity update",
            OP_END => "live_activity end",
            _ => "live_activity",
        })),
    }
}

/// この OS の `LiveActivity` 実装。
pub(crate) fn implementation() -> Arc<dyn LiveActivity> {
    #[cfg(target_os = "android")]
    {
        Arc::new(AndroidLiveActivity)
    }
    #[cfg(not(target_os = "android"))]
    {
        // host のテストビルド用。成功へは倒さない。
        Arc::new(UnsupportedLiveActivity)
    }
}

#[cfg(target_os = "android")]
struct AndroidLiveActivity;

#[cfg(target_os = "android")]
impl LiveActivity for AndroidLiveActivity {
    fn submit(
        &self,
        request_id: u64,
        op: fandhe_device::live_activity::LiveOperation,
    ) -> Result<(), DeviceError> {
        use fandhe_device::live_activity::LiveOperation;
        let (code, value) = match op {
            LiveOperation::Start(v) => (OP_START, v.get()),
            LiveOperation::Update(v) => (OP_UPDATE, v.get()),
            LiveOperation::End => (OP_END, 0),
        };
        const INTERNAL: DeviceError = DeviceError::Internal("live_activity submit");
        // 値域（0〜9999）は `LiveValue` で検証済みなので `i32` へ必ず収まる。
        let value = i32::try_from(value).map_err(|_| INTERNAL)?;
        if call_request(request_id, code, value) {
            Ok(())
        } else {
            Err(INTERNAL)
        }
    }

    fn dismiss_all(&self) {
        // 冪等。Java が無い・破棄済みでも害は無いので失敗は無視する。
        let _ = call_dismiss_all();
    }
}

/// `MainActivity.requestLiveActivity(long, int, int)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_app_badge::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_request(id: u64, op: i32, value: i32) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        let args = [
            jni::objects::JValue::Long(id as jni::sys::jlong),
            jni::objects::JValue::Int(op),
            jni::objects::JValue::Int(value),
        ];
        env.call_method(
            &activity,
            jni::jni_str!("requestLiveActivity"),
            jni::jni_sig!((long, int, int) -> ()),
            &args,
        )?;
        Ok(())
    })
    .is_ok()
}

/// `MainActivity.liveActivityDismissAll()` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は [`call_request`] と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_dismiss_all() -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `call_request` と同じ。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: `call_request` と同じ。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("liveActivityDismissAll"),
            jni::jni_sig!(() -> ()),
            &[],
        )?;
        Ok(())
    })
    .is_ok()
}

/// `LiveActivityHelper.nativeOnLiveActivityResult`（Java 側）から呼ばれる JNI エクスポート。
/// 値・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_LiveActivityHelper_nativeOnLiveActivityResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    op: jni::sys::jint,
    code: jni::sys::jint,
) {
    super::live_activity::bridge::deliver(request_id as u64, decode(op, code));
    // 入力が無いと egui が描画せず処理中で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_codes() {
        assert!(matches!(decode(OP_START, 0), Ok(LiveOutcome::Started)));
        assert!(matches!(decode(OP_UPDATE, 1), Ok(LiveOutcome::Updated)));
        assert!(matches!(decode(OP_END, 2), Ok(LiveOutcome::Ended)));
        assert!(matches!(decode(OP_UPDATE, 3), Ok(LiveOutcome::Dismissed)));
        assert!(matches!(
            decode(OP_START, 4),
            Err(DeviceError::Unsupported("live_activity"))
        ));
        assert!(matches!(
            decode(OP_START, 5),
            Err(DeviceError::PermissionDenied("live_activity"))
        ));
    }

    #[test]
    fn decode_failure_and_unknown_are_internal() {
        assert!(matches!(
            decode(OP_START, 6),
            Err(DeviceError::Internal("live_activity start"))
        ));
        assert!(matches!(
            decode(OP_UPDATE, 99),
            Err(DeviceError::Internal("live_activity update"))
        ));
        assert!(matches!(
            decode(OP_END, -1),
            Err(DeviceError::Internal("live_activity end"))
        ));
        assert!(matches!(
            decode(42, 99),
            Err(DeviceError::Internal("live_activity"))
        ));
    }
}
