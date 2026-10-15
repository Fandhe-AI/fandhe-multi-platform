//! Android 向けメール・SMS 作成画面実装（Issue #1386、親 #1142）。`device/compose_mail_sms.rs` の
//! Android 経路（`begin_compose`）から呼ばれる。Java 側 `ComposeHelper` が `ACTION_SENDTO`
//! （メールは `mailto:`、SMS は `smsto:` + `sms_body`）で OS の作成画面を開く。
//!
//! 流れ: `begin_compose` → JNI `MainActivity.requestCompose(id, kind, to, cc, bcc, subject, body, attData, attMime, attName)` →
//! Java が `nativeOnComposeResult(id, code)` で返す → `outcome_from_android_code` で契約の結果へ
//! 写像 → `compose_mail_sms::bridge::deliver` が `SLOT` の id と照合して置く。
//! 値は `ComposeRequest` の検証を通ったものだけを渡す（URL は組み立てない）。
//! 添付（Issue #32、メールのみ）は長さ 0 の配列と空文字列が「なし」。ありのとき Java は
//! `cache/compose/` に要求ごとのディレクトリで書き（10 分超の古いものだけ回収）、`FileProvider` の content URI で `ACTION_SEND` する。
//!
//! 守る範囲: 1 プロセス内の要求順序・NoHandler / 非前景を成功に畳まないこと・Activity 再生成時の
//! 旧要求の破棄。守らない範囲: 作成画面へ渡した後の送信可否（結果は `HandedOff` までしか言えない）。
//! `<queries>` と `SEND_SMS` は宣言しない。値・結果はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestCompose` を経た後でしか呼ばれない
//! （app_badge と同じ）。

use fandhe_device::compose_mail_sms::{ComposeKind, ComposeRequest};
use fandhe_device::DeviceError;

/// 種類コード（Java 側 `ComposeHelper.KIND_*` と対）。
const KIND_MAIL: i32 = 0;
const KIND_SMS: i32 = 1;

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn kind_code(kind: ComposeKind) -> i32 {
    match kind {
        ComposeKind::Mail => KIND_MAIL,
        ComposeKind::Sms => KIND_SMS,
    }
}

/// 作成画面の表示を依頼する。`Ok` は受理済みで、結果は後から `nativeOnComposeResult` で届く。
#[cfg(target_os = "android")]
pub(crate) fn begin_compose(request_id: u64, req: &ComposeRequest) -> Result<(), DeviceError> {
    if call_activity(request_id, req) {
        Ok(())
    } else {
        Err(DeviceError::Internal("compose present"))
    }
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_compose(_request_id: u64, _req: &ComposeRequest) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("compose"))
}

/// `MainActivity.requestCompose(long, int, String×5, byte[], String, String)` を JNI で呼ぶ。
/// `false` は呼び出し失敗。`unsafe` 2 箇所は `android_app_badge::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, req: &ComposeRequest) -> bool {
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
        // 各リストは `\n` 連結で渡す（`\n` は検証で拒否される文字で宛先に現れない。Java 側で配列へ戻す）。
        let to = env.new_string(req.to().join("\n"))?;
        let cc = env.new_string(req.cc().join("\n"))?;
        let bcc = env.new_string(req.bcc().join("\n"))?;
        let subject = env.new_string(req.subject())?;
        let body = env.new_string(req.body())?;
        let att = req.attachment();
        let att_data = env.byte_array_from_slice(att.map_or(&[][..], |a| a.data()))?;
        let att_mime = env.new_string(att.map_or("", |a| a.mime_type()))?;
        let att_name = env.new_string(att.map_or("", |a| a.file_name()))?;
        let args = [
            jni::objects::JValue::Long(id as jni::sys::jlong),
            jni::objects::JValue::Int(kind_code(req.kind())),
            jni::objects::JValue::Object(&to),
            jni::objects::JValue::Object(&cc),
            jni::objects::JValue::Object(&bcc),
            jni::objects::JValue::Object(&subject),
            jni::objects::JValue::Object(&body),
            jni::objects::JValue::Object(&att_data),
            jni::objects::JValue::Object(&att_mime),
            jni::objects::JValue::Object(&att_name),
        ];
        env.call_method(
            &activity,
            jni::jni_str!("requestCompose"),
            jni::jni_sig!((long, int, java.lang.String, java.lang.String, java.lang.String, java.lang.String, java.lang.String, byte[], java.lang.String, java.lang.String) -> ()),
            &args,
        )?;
        Ok(())
    })
    .is_ok()
}

/// `ComposeHelper.nativeOnComposeResult`（Java 側）から呼ばれる JNI エクスポート。
/// 値・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_ComposeHelper_nativeOnComposeResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::compose_mail_sms::bridge::deliver(
        request_id as u64,
        super::compose_mail_sms::outcome_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_never_succeeds() {
        let req = ComposeRequest::mail(&["a@b.c"], &[], &[], "s", "b").unwrap();
        assert!(matches!(
            begin_compose(1, &req),
            Err(DeviceError::Unsupported("compose"))
        ));
    }

    #[test]
    fn kind_code_matches_java() {
        assert_eq!(kind_code(ComposeKind::Mail), 0);
        assert_eq!(kind_code(ComposeKind::Sms), 1);
    }
}
