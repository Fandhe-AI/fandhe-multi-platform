//! Android 向け「電話発信・地図など他アプリの起動」実装（親 #176、Issue #863）。`driver.rs` の
//! Android 経路（dispatch）から呼ばれる。Java 側 `ExternalAppLaunchHelper` が固定の 2 種
//! （電話 `ACTION_DIAL`・地図 `ACTION_VIEW`）を `startActivity` で開く。契約の SSOT は
//! `crate::contract`。
//!
//! 流れ: `begin_launch` → JNI static の `ExternalAppLaunchHelper.launch(Activity, id, target, uri)` →
//! Java が `nativeOnExternalAppLaunchResult(id, code)` で返す → `outcome_from_android_code` で契約の結果へ
//! 写像 → `driver::bridge::deliver` が `SLOT` の id と照合して置く。
//! 渡すのは `long` の id・`int` の target コード・契約の固定 URI だけ。外部入力は通らない。
//!
//! 守る範囲: 1 プロセス内の要求順序・非前景や JNI 失敗を成功に畳まないこと・Activity 再生成時の
//! 旧要求の破棄。守らない範囲: 他アプリへ渡した後の利用者の操作（結果は `HandedOff` までしか言えない）。
//! `<queries>`・権限・`resolveActivity` は使わない。`ACTION_CALL` は使わない。開けなかった操作を別の URI で代用しない。
//! id・結果はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestExternalAppLaunch` を経た後でしか
//! 呼ばれない（open-system-settings と同じ）。

use crate::contract::{GEO_URI, LaunchTarget, PHONE_URI};
use crate::error::ExternalAppLaunchError;

/// target コード（Java 側 `ExternalAppLaunchHelper.TARGET_*` と対）。
const TARGET_PHONE: i32 = 0;
const TARGET_MAP: i32 = 1;

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn target_code(target: LaunchTarget) -> i32 {
    match target {
        LaunchTarget::Phone => TARGET_PHONE,
        LaunchTarget::Map => TARGET_MAP,
    }
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn target_uri(target: LaunchTarget) -> &'static str {
    match target {
        LaunchTarget::Phone => PHONE_URI,
        LaunchTarget::Map => GEO_URI,
    }
}

/// OS へ起動を依頼する。`Ok` は受理済みで、結果は後から `nativeOnExternalAppLaunchResult` で届く。
#[cfg(target_os = "android")]
pub(crate) fn begin_launch(
    request_id: u64,
    target: LaunchTarget,
) -> Result<(), ExternalAppLaunchError> {
    if call_activity(request_id, target) {
        Ok(())
    } else {
        Err(ExternalAppLaunchError::Internal("external app present"))
    }
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_launch(
    _request_id: u64,
    _target: LaunchTarget,
) -> Result<(), ExternalAppLaunchError> {
    Err(ExternalAppLaunchError::Unsupported("external app launch"))
}

/// `ExternalAppLaunchHelper.launch(Activity, long, int, String)` を JNI で呼ぶ。`false` は呼び出し失敗
/// （helper が無い場合を含む）。クラスは Activity の class loader で解決する
/// （`fandhe_platform_android::with_activity_class`）。
#[cfg(target_os = "android")]
fn call_activity(id: u64, target: LaunchTarget) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ExternalAppLaunchHelper"),
        |env, activity, class| -> Result<bool, JniCallFailed> {
            let uri = env.new_string(target_uri(target))?;
            let args = [
                jni::objects::JValue::Object(activity),
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Int(target_code(target)),
                jni::objects::JValue::Object(&uri),
            ];
            let result = env.call_static_method(
                class,
                jni::jni_str!("launch"),
                jni::jni_sig!((android.app.Activity, long, int, java.lang.String) -> boolean),
                &args,
            );
            if result.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            Ok(result?.z()?)
        },
    )
    .unwrap_or(false)
}

/// `ExternalAppLaunchHelper.nativeOnExternalAppLaunchResult`（Java 側）から呼ばれる JNI エクスポート。
/// id・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ExternalAppLaunchHelper_nativeOnExternalAppLaunchResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    crate::driver::bridge::deliver(
        request_id as u64,
        crate::driver::outcome_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_stub_never_succeeds() {
        for t in [LaunchTarget::Phone, LaunchTarget::Map] {
            assert!(matches!(
                begin_launch(1, t),
                Err(ExternalAppLaunchError::Unsupported("external app launch"))
            ));
        }
    }

    #[test]
    fn target_code_matches_java() {
        assert_eq!(target_code(LaunchTarget::Phone), 0);
        assert_eq!(target_code(LaunchTarget::Map), 1);
    }

    #[test]
    fn target_uri_is_fixed_constants() {
        assert_eq!(target_uri(LaunchTarget::Phone), PHONE_URI);
        assert!(target_uri(LaunchTarget::Phone).starts_with("tel:"));
        assert_eq!(target_uri(LaunchTarget::Map), GEO_URI);
        assert!(target_uri(LaunchTarget::Map).starts_with("geo:"));
    }
}
