//! Android 向け共有シート実装（Issue #899、親 #896、#1302 で機能 crate へ移動）。`driver` の Android 経路から
//! [`request_share`] が呼ばれ、Java 側 `ShareSheetHelper.share(Activity, long, String, String)` の static メソッドを
//! JNI で橋渡しする。`Intent.createChooser` は選択結果を返さないため、Java 側が `IntentSender` の
//! broadcast で選択完了を検出し、受信できれば `Shared`、chooser が閉じても届かなければ `Unknown`
//! （結果不明。`driver::outcome_from_code` の code 4）として `nativeOnShareResult` で通知する。
//!
//! JNI 窓口の署名は Java（`ShareSheetHelper`）と本ファイルの `jni_sig!`・エクスポート名の 2 か所でそろえる:
//!
//! - `boolean share(Activity, long, String, String)`: 提示を依頼して即座に戻る（`url` は null なら URL 無し）。
//!   helper が無い（生成前・破棄後）なら false で、呼び出し元が失敗として確定する
//! - `static native void nativeOnShareResult(long, int)` → `Java_com_fandhe_platform_ShareSheetHelper_nativeOnShareResult`
//!
//! 共有テキスト・URL はログへ出さない（security.md A09）。

use crate::contract::ShareRequest;

/// Java 側 `ShareSheetHelper.share` を呼び、共有シートの提示を要求する。結果を待たずに戻る
/// （実結果は `nativeOnShareResult` が非同期に届ける）。戻り値は「依頼を受理できたか」
/// （`false` はメソッド未検出・例外・Activity 未確立・helper 不在。呼び出し元が `Internal` として確定する）。
pub(crate) fn request_share(request_id: u64, request: &ShareRequest) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ShareSheetHelper"),
        |env, activity, class| -> Result<bool, JniCallFailed> {
            let result = (|| {
                let j_text = env.new_string(request.text())?;
                let j_url = match request.url() {
                    Some(url) => jni::objects::JObject::from(env.new_string(url)?),
                    None => jni::objects::JObject::null(),
                };
                let value = env.call_static_method(
                    class,
                    jni::jni_str!("share"),
                    jni::jni_sig!((android.app.Activity, long, java.lang.String, java.lang.String) -> boolean),
                    &[
                        jni::objects::JValue::Object(activity),
                        jni::objects::JValue::Long(request_id as jni::sys::jlong),
                        jni::objects::JValue::Object(&j_text),
                        jni::objects::JValue::Object(&j_url),
                    ],
                )?;
                value.z()
            })();
            if result.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            Ok(result?)
        },
    )
    .unwrap_or(false)
}

/// `ShareSheetHelper.nativeOnShareResult`（Java 側）から呼ばれる JNI エクスポート関数。`code` の意味は
/// `driver::outcome_from_code` の表（iOS / macOS / Android 共通）と対で決める。`request_id` が現在追跡中の要求と
/// 不一致（旧要求の遅延到着）なら `deliver` が黙って捨てる。成否に関わらず次フレームを起こし、effect が
/// `bridge` を観測できるようにする。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名で、
/// 本 crate は 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ShareSheetHelper_nativeOnShareResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    crate::driver::bridge::deliver(request_id as u64, crate::driver::outcome_from_code(code));
    fandhe_runtime::request_repaint();
}
