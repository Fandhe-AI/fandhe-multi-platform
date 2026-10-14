//! Android 向けバッジ設定実装（Issue #1362、親 #1141）。`device/app_badge.rs` の Android 経路
//! （`begin_set`）から呼ばれる。Android にバッジ専用 API は無いため、Java 側 `AppBadgeHelper` が
//! `NotificationCompat.Builder.setNumber` を付けた無音の通知を固定 ID で 1 件出し、0 では
//! `cancel` で消す。
//!
//! 流れ: `begin_set`（UI スレッド）→ JNI `MainActivity.requestAppBadge(id, value)` → Java が
//! 結果コードを `nativeOnAppBadgeResult(id, value, code)` で返す → [`decode`] で契約の結果へ
//! 写像 → `app_badge::bridge::deliver` が `SLOT` の id と照合して置く。`value` は 0 の成功を
//! 「消去」の文言に分けるため Java が保持した値をそのまま返させる（Rust 側に値の static を持たない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・API 26 未満/拒否/通知オフを成功に畳まないこと・
//! Activity 再生成時の旧要求の破棄。守らない範囲: 通知のスワイプ等で OS 側のバッジが消えた
//! ときの画面表示値との食い違い、ランチャーごとの表示差。`Cancelled` は出さない
//! （拒否とダイアログを閉じただけの場合を区別できないため）。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestAppBadge` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（pedometer と同じ）。

use fandhe_device::app_badge::{BadgeOutcome, BadgeValue};
use fandhe_device::DeviceError;

/// 結果コード（SSOT は Java 側 `AppBadgeHelper.RESULT_*`）。
const RESULT_SUBMITTED: i32 = 0;
const RESULT_UNSUPPORTED: i32 = 1;
const RESULT_PERMISSION_DENIED: i32 = 2;

const INTERNAL: DeviceError = DeviceError::Internal("app_badge set");

/// Java の結果コードと値を `(値, 契約の結果)` へ写像する。未知のコードと負値は `Internal`。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn decode(value: i32, code: i32) -> (u32, Result<BadgeOutcome, DeviceError>) {
    let Ok(v) = u32::try_from(value) else {
        return (0, Err(INTERNAL));
    };
    let r = match code {
        RESULT_SUBMITTED => Ok(BadgeOutcome::Submitted),
        RESULT_UNSUPPORTED => Err(DeviceError::Unsupported("app_badge")),
        RESULT_PERMISSION_DENIED => Err(DeviceError::PermissionDenied("app_badge")),
        _ => Err(INTERNAL),
    };
    (v, r)
}

/// 設定を依頼する。`Ok` は受理済みで、結果は後から `nativeOnAppBadgeResult` で届く。
/// 値域（0〜9999）は `BadgeValue::new` で検証済みなので `i32` へ必ず収まる。
#[cfg(target_os = "android")]
pub(crate) fn begin_set(request_id: u64, value: BadgeValue) -> Result<(), DeviceError> {
    let v = i32::try_from(value.get()).map_err(|_| INTERNAL)?;
    if call_activity(request_id, v) {
        Ok(())
    } else {
        Err(INTERNAL)
    }
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_set(_request_id: u64, _value: BadgeValue) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("app_badge"))
}

/// `MainActivity.requestAppBadge(long, int)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_pedometer::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, value: i32) -> bool {
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
            jni::objects::JValue::Int(value),
        ];
        env.call_method(
            &activity,
            jni::jni_str!("requestAppBadge"),
            jni::jni_sig!((long, int) -> ()),
            &args,
        )?;
        Ok(())
    })
    .is_ok()
}

/// `AppBadgeHelper.nativeOnAppBadgeResult`（Java 側）から呼ばれる JNI エクスポート。
/// 値・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_AppBadgeHelper_nativeOnAppBadgeResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    value: jni::sys::jint,
    code: jni::sys::jint,
) {
    let (v, r) = decode(value, code);
    super::app_badge::bridge::deliver(request_id as u64, v, r);
    // 入力が無いと egui が描画せず「設定中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_codes() {
        assert!(matches!(decode(3, 0), (3, Ok(BadgeOutcome::Submitted))));
        assert!(matches!(decode(0, 0), (0, Ok(BadgeOutcome::Submitted))));
        assert!(matches!(
            decode(3, 1),
            (3, Err(DeviceError::Unsupported(_)))
        ));
        assert!(matches!(
            decode(3, 2),
            (3, Err(DeviceError::PermissionDenied(_)))
        ));
        assert!(matches!(decode(3, 3), (3, Err(DeviceError::Internal(_)))));
        assert!(matches!(decode(3, 99), (3, Err(DeviceError::Internal(_)))));
        assert!(matches!(decode(-1, 0), (0, Err(DeviceError::Internal(_)))));
    }
}
