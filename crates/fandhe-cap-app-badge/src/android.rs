//! Android 向けバッジ設定実装（Issue #1362、親 #1141）。`fandhe-cap-app-badge/src/driver.rs` の Android 経路
//! （`begin_set`）から呼ばれる。Android にバッジ専用 API は無いため、Java 側 `AppBadgeHelper` が
//! `NotificationCompat.Builder.setNumber` を付けた無音の通知を固定 ID で 1 件出し、0 では
//! `cancel` で消す。
//!
//! 流れ: `begin_set`（UI スレッド）→ JNI `AppBadgeHelper.set(activity, id, value)` → Java が
//! 結果コードを `nativeOnAppBadgeResult(id, value, code)` で返す → [`decode`] で契約の結果へ
//! 写像 → `app_badge::bridge::deliver` が `SLOT` の id と照合して置く。`value` は 0 の成功を
//! 「消去」の文言に分けるため Java が保持した値をそのまま返させる（Rust 側に値の static を持たない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・API 26 未満/拒否/通知オフを成功に畳まないこと・
//! Activity 再生成時の旧要求の破棄。守らない範囲: 通知のスワイプ等で OS 側のバッジが消えた
//! ときの画面表示値との食い違い、ランチャーごとの表示差。`Cancelled` は出さない
//! （拒否とダイアログを閉じただけの場合を区別できないため）。
//!
//! no-op スタブは置かない: Java の native は Rust の `AppBadgeHelper.set` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（pedometer と同じ）。

use crate::contract::{BadgeOutcome, BadgeValue};
use crate::error::AppBadgeError;

/// 結果コード（SSOT は Java 側 `AppBadgeHelper.RESULT_*`）。
const RESULT_SUBMITTED: i32 = 0;
const RESULT_UNSUPPORTED: i32 = 1;
const RESULT_PERMISSION_DENIED: i32 = 2;

const INTERNAL: AppBadgeError = AppBadgeError::Internal;

/// Java の結果コードと値を `(値, 契約の結果)` へ写像する。未知のコードと負値は `Internal`。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn decode(value: i32, code: i32) -> (u32, Result<BadgeOutcome, AppBadgeError>) {
    let Ok(v) = u32::try_from(value) else {
        return (0, Err(INTERNAL));
    };
    let r = match code {
        RESULT_SUBMITTED => Ok(BadgeOutcome::Submitted),
        RESULT_UNSUPPORTED => Err(AppBadgeError::Unsupported),
        RESULT_PERMISSION_DENIED => Err(AppBadgeError::PermissionDenied),
        _ => Err(INTERNAL),
    };
    (v, r)
}

/// 設定を依頼する。`Ok` は受理済みで、結果は後から `nativeOnAppBadgeResult` で届く。
/// 値域（0〜9999）は `BadgeValue::new` で検証済みなので `i32` へ必ず収まる。
#[cfg(target_os = "android")]
pub(crate) fn begin_set(request_id: u64, value: BadgeValue) -> Result<(), AppBadgeError> {
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
pub(crate) fn begin_set(_request_id: u64, _value: BadgeValue) -> Result<(), AppBadgeError> {
    Err(AppBadgeError::Unsupported)
}

/// `AppBadgeHelper.set(Activity, long, int)`（static）を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じている。
#[cfg(target_os = "android")]
fn call_activity(id: u64, value: i32) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.AppBadgeHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let args = [
                jni::objects::JValue::Object(activity),
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Int(value),
            ];
            let r = env.call_static_method(
                class,
                jni::jni_str!("set"),
                jni::jni_sig!((android.app.Activity, long, int) -> ()),
                &args,
            );
            if r.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            r?;
            Ok(())
        },
    )
    .is_ok()
}

/// `AppBadgeHelper.nativeOnAppBadgeResult`（Java 側）から呼ばれる JNI エクスポート。
/// 値・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_AppBadgeHelper_nativeOnAppBadgeResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    value: jni::sys::jint,
    code: jni::sys::jint,
) {
    let (v, r) = decode(value, code);
    crate::driver::bridge::deliver(request_id as u64, v, r);
    // 入力が無いと egui が描画せず「設定中」で止まるため次フレームを起こす。
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_codes() {
        assert!(matches!(decode(3, 0), (3, Ok(BadgeOutcome::Submitted))));
        assert!(matches!(decode(0, 0), (0, Ok(BadgeOutcome::Submitted))));
        assert!(matches!(decode(3, 1), (3, Err(AppBadgeError::Unsupported))));
        assert!(matches!(
            decode(3, 2),
            (3, Err(AppBadgeError::PermissionDenied))
        ));
        assert!(matches!(decode(3, 3), (3, Err(AppBadgeError::Internal))));
        assert!(matches!(decode(3, 99), (3, Err(AppBadgeError::Internal))));
        assert!(matches!(decode(-1, 0), (0, Err(AppBadgeError::Internal))));
    }
}
