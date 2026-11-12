//! Android 向けライブアクティビティ実装（Issue #296、親 #88）。`driver.rs` の
//! `platform_live_activity` android 経路から呼ばれる。Java 側 `LiveActivityHelper` が
//! `NotificationCompat` の進行中通知（`setOngoing(true)` + `setProgress`）を固定 tag/ID で 1 件
//! 出し、開始・更新・終了を行う。Android 16 の Live Updates（`ProgressStyle`）は範囲外。
//!
//! 流れ: `submit`（UI スレッド）→ JNI `LiveActivityHelper.submit(activity, id, op, value)`（static） → Java が
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
//! no-op スタブは置かない: Java の native は Rust の `submit` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（app-badge と同じ）。

use std::sync::Arc;

#[cfg(not(target_os = "android"))]
use crate::contract::UnsupportedLiveActivity;
use crate::contract::{LiveActivity, LiveOutcome};
use crate::error::LiveActivityError;

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
fn decode(op: i32, code: i32) -> Result<LiveOutcome, LiveActivityError> {
    match code {
        RESULT_STARTED => Ok(LiveOutcome::Started),
        RESULT_UPDATED => Ok(LiveOutcome::Updated),
        RESULT_ENDED => Ok(LiveOutcome::Ended),
        RESULT_DISMISSED => Ok(LiveOutcome::Dismissed),
        RESULT_UNSUPPORTED => Err(LiveActivityError::Unsupported),
        RESULT_PERMISSION_DENIED => Err(LiveActivityError::PermissionDenied),
        _ => Err(LiveActivityError::Internal(match op {
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
        op: crate::contract::LiveOperation,
    ) -> Result<(), LiveActivityError> {
        use crate::contract::LiveOperation;
        let (code, value) = match op {
            LiveOperation::Start(v) => (OP_START, v.get()),
            LiveOperation::Update(v) => (OP_UPDATE, v.get()),
            LiveOperation::End => (OP_END, 0),
        };
        const INTERNAL: LiveActivityError = LiveActivityError::Internal("live_activity submit");
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

/// `LiveActivityHelper.submit(Activity, long, int, int)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// クラスは Activity の class loader で解決する（`fandhe_platform_android::with_activity_class`）。
#[cfg(target_os = "android")]
fn call_request(id: u64, op: i32, value: i32) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.LiveActivityHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            env.call_static_method(
                class,
                jni::jni_str!("submit"),
                jni::jni_sig!((android.app.Activity, long, int, int) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(id as jni::sys::jlong),
                    jni::objects::JValue::Int(op),
                    jni::objects::JValue::Int(value),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `LiveActivityHelper.dismissAll(Activity)` を JNI で呼ぶ。`false` は呼び出し失敗。
#[cfg(target_os = "android")]
fn call_dismiss_all() -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.LiveActivityHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            env.call_static_method(
                class,
                jni::jni_str!("dismissAll"),
                jni::jni_sig!((android.app.Activity) -> ()),
                &[jni::objects::JValue::Object(activity)],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `LiveActivityHelper.nativeOnLiveActivityResult`（Java 側）から呼ばれる JNI エクスポート。
/// 値・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_LiveActivityHelper_nativeOnLiveActivityResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    op: jni::sys::jint,
    code: jni::sys::jint,
) {
    crate::driver::bridge::deliver(request_id as u64, decode(op, code));
    // 入力が無いと egui が描画せず処理中で止まるため次フレームを起こす。
    fandhe_runtime::request_repaint();
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
            Err(LiveActivityError::Unsupported)
        ));
        assert!(matches!(
            decode(OP_START, 5),
            Err(LiveActivityError::PermissionDenied)
        ));
    }

    #[test]
    fn decode_failure_and_unknown_are_internal() {
        assert!(matches!(
            decode(OP_START, 6),
            Err(LiveActivityError::Internal("live_activity start"))
        ));
        assert!(matches!(
            decode(OP_UPDATE, 99),
            Err(LiveActivityError::Internal("live_activity update"))
        ));
        assert!(matches!(
            decode(OP_END, -1),
            Err(LiveActivityError::Internal("live_activity end"))
        ));
        assert!(matches!(
            decode(42, 99),
            Err(LiveActivityError::Internal("live_activity"))
        ));
    }
}
