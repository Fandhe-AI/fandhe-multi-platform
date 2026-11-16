//! Android 向け連絡先選択（Issue #1222、親 #1117）。`crate::driver` の Android 経路から呼ばれ、
//! 提示は `ContactsHelper.request(Activity, long)`（static）、結果は `nativeOnContactPickerResult` で 1 度だけ届く
//! pull 型 bridge。コードは 0 = 選択（name 有効）/ 1 = キャンセル / 3 = 失敗で、写像は
//! `crate::driver::outcome_from_code` に一本化する。
//!
//! `unsafe` は JNI 受け口の `#[unsafe(no_mangle)]` だけで、JNI 呼び出しは共通層
//! `fandhe_platform_android::with_activity_class` に閉じている。

#[cfg(target_os = "android")]
use fandhe_platform_android::{JniCallFailed, with_activity_class};

/// `ContactsHelper.request(Activity, long)` を呼び、ピッカーの提示だけを要求する。
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` は呼び出し元が `Internal` に畳む）。
#[cfg(target_os = "android")]
pub(crate) fn request_contact_pick(request_id: u64) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ContactsHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let r = env.call_static_method(
                class,
                jni::jni_str!("request"),
                jni::jni_sig!((android.app.Activity, long) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                ],
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

/// `ContactsHelper.nativeOnContactPickerResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// `code`: 0 = 選択（`name` 有効）/ 1 = キャンセル / 3 = 失敗。`name` が null・変換不能なら `None` とし、
/// `outcome_from_code` が `Internal` にする（成功にしない）。`request_id` 不一致（再生成後の遅延到着等）は
/// `deliver` が捨てる。名前はログ出力しない（A09）。pull 型のため `deliver` の成否に関わらず再描画を要求する。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ContactsHelper_nativeOnContactPickerResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let name = if name.is_null() {
            None
        } else {
            name.try_to_string(env).ok()
        };
        let result = crate::driver::outcome_from_code(code, name);
        crate::driver::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    fandhe_runtime::request_repaint();
}
