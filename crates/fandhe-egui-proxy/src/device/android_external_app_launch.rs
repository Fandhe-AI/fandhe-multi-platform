//! Android 向け「電話発信・地図など他アプリの起動」実装（親 #176、Issue #863）。`device/external_app_launch.rs` の
//! Android 経路（dispatch）から呼ばれる。Java 側 `ExternalAppLaunchHelper` が固定の 2 種
//! （電話 `ACTION_DIAL`・地図 `ACTION_VIEW`）を `startActivity` で開く。契約の SSOT は
//! `fandhe_device::external_app_launch`。
//!
//! 流れ: `begin_launch` → JNI `MainActivity.requestExternalAppLaunch(id, target, uri)` →
//! Java が `nativeOnExternalAppLaunchResult(id, code)` で返す → `outcome_from_android_code` で契約の結果へ
//! 写像 → `external_app_launch::bridge::deliver` が `SLOT` の id と照合して置く。
//! 渡すのは `long` の id・`int` の target コード・`fandhe_device` の固定 URI だけ。外部入力は通らない。
//!
//! 守る範囲: 1 プロセス内の要求順序・非前景や JNI 失敗を成功に畳まないこと・Activity 再生成時の
//! 旧要求の破棄。守らない範囲: 他アプリへ渡した後の利用者の操作（結果は `HandedOff` までしか言えない）。
//! `<queries>`・権限・`resolveActivity` は使わない。`ACTION_CALL` は使わない。開けなかった操作を別の URI で代用しない。
//! id・結果はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestExternalAppLaunch` を経た後でしか
//! 呼ばれない（open_system_settings と同じ）。

use fandhe_device::external_app_launch::{LaunchTarget, GEO_URI, PHONE_URI};
use fandhe_device::DeviceError;

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
pub(crate) fn begin_launch(request_id: u64, target: LaunchTarget) -> Result<(), DeviceError> {
    if call_activity(request_id, target) {
        Ok(())
    } else {
        Err(DeviceError::Internal("external app present"))
    }
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_launch(_request_id: u64, _target: LaunchTarget) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("external app launch"))
}

/// `MainActivity.requestExternalAppLaunch(long, int, String)` を JNI で呼ぶ。
/// `false` は呼び出し失敗。`unsafe` 2 箇所は `android_compose_mail_sms::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, target: LaunchTarget) -> bool {
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
        let uri = env.new_string(target_uri(target))?;
        let args = [
            jni::objects::JValue::Long(id as jni::sys::jlong),
            jni::objects::JValue::Int(target_code(target)),
            jni::objects::JValue::Object(&uri),
        ];
        env.call_method(
            &activity,
            jni::jni_str!("requestExternalAppLaunch"),
            jni::jni_sig!((long, int, java.lang.String) -> ()),
            &args,
        )?;
        Ok(())
    })
    .is_ok()
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
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_ExternalAppLaunchHelper_nativeOnExternalAppLaunchResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::external_app_launch::bridge::deliver(
        request_id as u64,
        super::external_app_launch::outcome_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_stub_never_succeeds() {
        for t in [LaunchTarget::Phone, LaunchTarget::Map] {
            assert!(matches!(
                begin_launch(1, t),
                Err(DeviceError::Unsupported("external app launch"))
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
