//! Android 向けローカル通知の橋渡し（Issue #1006）。本 crate の `driver` の Android 経路から、通知の表示（権限確認込み）を
//! Java 側 `NotificationHelper.requestNotification(Activity, long, String, String)` へ JNI の static 呼び出しで依頼する
//! （`fandhe_platform_android::with_activity_class`。第 1 引数は Activity）。結果を待たずに即座に戻り、実結果は
//! `NotificationHelper.nativeOnNotificationResult`（下の JNI エクスポート）が非同期に届ける（`crate::bridge` と pull 型）。
//! `code` の意味は `crate::outcome_from_code` の doc（Java 側 `RESULT_*` 定数と対）が SSOT。
//!
//! 守る範囲: 要求 id の照合（旧要求の遅延到着は `bridge::deliver` が捨てる）、嘘の成功の禁止。
//! 守らない範囲: 通知の表示の確認、通知タップへの応答。タイトル・本文はログに出さない（security.md A09）。

use fandhe_platform_android::JniCallFailed;

/// Java 側 `NotificationHelper.requestNotification` を呼び、通知の表示（権限確認込み）を要求する。
/// `title`/`body` は呼び出し元（`driver`）で `NotificationRequest::new` 検証済みの文字列。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元が `false` を見て
/// `Err(Internal("notification present"))` として `bridge::deliver` する）。`unsafe` は共通層
/// `fandhe_platform_android::with_activity_class` に閉じている。
pub(crate) fn request_notification(request_id: u64, title: &str, body: &str) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    fandhe_platform_android::with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.NotificationHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let j_title = env.new_string(title)?;
            let j_body = env.new_string(body)?;
            env.call_static_method(
                class,
                jni::jni_str!("requestNotification"),
                jni::jni_sig!((android.app.Activity, long, java.lang.String, java.lang.String) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Object(&j_title),
                    jni::objects::JValue::Object(&j_body),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `NotificationHelper.nativeOnNotificationResult`（Java 側）から呼ばれる JNI エクスポート関数（Issue #1006）。
/// シンボル名は JNI の命名規約で固定。`code` は [`crate::outcome_from_code`] で変換し、[`crate::bridge::deliver`] へ届ける。
/// `request_id` が現在追跡中の要求と不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる。
/// `deliver` の成否に関わらず無条件で再描画を起こし、`drive_callback` が次フレームで結果を観測できるようにする。
/// タイトル・本文は引数に含まれないためログ出力の懸念は無い（security.md A09）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名であり、本 crate は
/// cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_NotificationHelper_nativeOnNotificationResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let outcome = crate::outcome_from_code(code);
    crate::bridge::deliver(request_id as u64, outcome);
    fandhe_runtime::request_repaint();
}
