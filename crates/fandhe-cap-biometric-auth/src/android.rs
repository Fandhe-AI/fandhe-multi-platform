//! Android 向け生体認証（Issue #908）。`crate::driver` の Android 経路から呼ばれ、提示は
//! `BiometricAuthHelper.request(Activity, long, String)`（static）、結果は `nativeOnBiometricResult` で 1 度だけ届く
//! pull 型 bridge（`crate::driver::bridge`）。`BiometricAuth` trait は通らない。`code` の意味は
//! `crate::driver::outcome_from_android_code` の doc（Java 側 `BiometricAuthHelper` の code 定数と対）を参照。
//!
//! `unsafe` は JNI 受け口の `#[unsafe(no_mangle)]` だけで、JNI 呼び出しは共通層
//! `fandhe_platform_android::with_activity_class` に閉じている。生体データ・識別子は引数に含まれない（security.md A09）。

use fandhe_platform_android::{JniCallFailed, with_activity_class};

/// `BiometricAuthHelper.request(Activity, long, String)` を呼び、生体認証プロンプトの提示を要求する。
/// 結果を待たずに即座に戻る（実結果は `nativeOnBiometricResult` が非同期に届ける）。`reason` は呼び出し元で
/// `validate_reason` 済みの非空文字列。戻り値は JNI 呼び出し自体の成否のみ（`false` は呼び出し元が
/// `Err(Internal("biometric present"))` として `bridge::deliver` する）。
pub(crate) fn request_biometric(request_id: u64, reason: &str) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.BiometricAuthHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let r = (|| -> Result<(), jni::errors::Error> {
                let j_reason = env.new_string(reason)?;
                env.call_static_method(
                    class,
                    jni::jni_str!("request"),
                    jni::jni_sig!((android.app.Activity, long, java.lang.String) -> ()),
                    &[
                        jni::objects::JValue::Object(activity),
                        jni::objects::JValue::Long(request_id as jni::sys::jlong),
                        jni::objects::JValue::Object(&j_reason),
                    ],
                )?;
                Ok(())
            })();
            if r.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            Ok(r?)
        },
    )
    .is_ok()
}

/// `BiometricAuthHelper.nativeOnBiometricResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// `crate::driver::outcome_from_android_code` で変換し、`crate::driver::bridge::deliver` へ届ける。`request_id` が
/// 現在追跡中の要求と不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる。
/// `deliver` の成否に関わらず無条件で再描画を要求し、`drive_callback` が次フレームで結果を観測できるようにする。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_BiometricAuthHelper_nativeOnBiometricResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let outcome = crate::driver::outcome_from_android_code(code);
    crate::driver::bridge::deliver(request_id as u64, outcome);
    fandhe_runtime::request_repaint();
}
