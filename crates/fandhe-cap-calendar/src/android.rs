//! Android 向け予定追加実装（Issue #1226、#1187）。`driver.rs` の effect ドライバから呼ばれ、Java の
//! `CalendarHelper.request(Activity, id, title, ...)`（static）を JNI で呼んでカレンダーアプリの編集画面
//! （`ACTION_INSERT`）を開く。権限は使わない。結果は Java の `nativeOnCalendarResult` が
//! `driver::bridge`（pull 型）へ非同期に届ける（`outcome_from_android_code` で契約の結果へ写す）。
//!
//! 結果コードは `startActivity` が例外なく戻った時点で返し、編集画面からの戻りは待たない
//! （保存の有無は分からない）。件名・日時はログへ出さない（security.md A09）。
//! Java 側に対になる no-op スタブは置かない: `request` を経ないと `nativeOnCalendarResult` を呼ぶ経路自体が無いため。

#[cfg(target_os = "android")]
use crate::contract::EventDraft;

/// `CalendarHelper.request` を呼び、カレンダーアプリの編集画面を開く。戻り値は JNI 呼び出し自体の成否のみ
/// （`false` は呼び出し元が失敗として畳む）。
#[cfg(target_os = "android")]
pub(crate) fn request_calendar_insert(request_id: u64, draft: &EventDraft) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    let (s, e) = (draft.start(), draft.end());
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.CalendarHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let j_title = env.new_string(draft.title())?;
            let i = |v: u8| jni::objects::JValue::Int(v as jni::sys::jint);
            env.call_static_method(
                class,
                jni::jni_str!("request"),
                jni::jni_sig!((android.app.Activity, long, java.lang.String, int, int, int, int, int, int, int, int, int, int) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Object(&j_title),
                    jni::objects::JValue::Int(s.year as jni::sys::jint),
                    i(s.month),
                    i(s.day),
                    i(s.hour),
                    i(s.minute),
                    jni::objects::JValue::Int(e.year as jni::sys::jint),
                    i(e.month),
                    i(e.day),
                    i(e.hour),
                    i(e.minute),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `CalendarHelper.nativeOnCalendarResult`（Java 側）から呼ばれる JNI エクスポート関数。`code` の意味は
/// `driver.rs::outcome_from_android_code` の doc（Java 側 `RESULT_*` と対）。旧要求の遅延到着は `deliver` が
/// id 不一致で捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_CalendarHelper_nativeOnCalendarResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let outcome = crate::driver::outcome_from_android_code(code);
    crate::driver::bridge::deliver(request_id as u64, outcome);
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    #[test]
    fn android_codes_are_never_success_except_handed() {
        use crate::driver::outcome_from_android_code;
        assert!(outcome_from_android_code(0).is_ok());
        for code in [1, 2, 3, 99, -1] {
            assert!(outcome_from_android_code(code).is_err());
        }
    }
}
