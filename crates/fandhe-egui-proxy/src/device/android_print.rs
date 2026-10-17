//! Android 向け印刷実装（Issue #328、親 #94）。`device/print.rs` の Android 経路（`begin_print`）
//! から呼ばれる。Java 側 `PrintHelper` が `PrintManager` + `PrintDocumentAdapter` で OS の印刷
//! ダイアログを開く。
//!
//! 流れ: `begin_print` → JNI `MainActivity.requestPrint(id, title, body)` → Java が
//! `nativeOnPrintResult(id, code)` で返す → `print::outcome_from_android_code` で契約の結果へ写像
//! → `print::bridge::deliver` が `SLOT` の id と照合して置く。値は `PrintRequest` の検証を通った
//! ものだけを渡す（Canvas に描くだけで、URL・Intent・ファイルパスには使わない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・取り消しを失敗にしないこと・嘘の成功の禁止・Activity 再生成時
//! の旧要求の破棄。守らない範囲: 紙に出たか・ジョブ投入後のプリンタ側の失敗。権限と `<queries>` は
//! 宣言しない。見出し・本文・結果はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestPrint` を経た後でしか呼ばれない。

use fandhe_device::print::PrintRequest;
use fandhe_device::DeviceError;

/// 印刷ダイアログの表示を依頼する。`Ok` は受理済みで、結果は後から `nativeOnPrintResult` で届く。
#[cfg(target_os = "android")]
pub(crate) fn begin_print(request_id: u64, req: &PrintRequest) -> Result<(), DeviceError> {
    if call_activity(request_id, req) {
        Ok(())
    } else {
        Err(DeviceError::Internal("print present"))
    }
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_print(_request_id: u64, _req: &PrintRequest) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("print"))
}

/// `MainActivity.requestPrint(long, String, String)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_compose_mail_sms::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, req: &PrintRequest) -> bool {
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
        let title = env.new_string(req.title())?;
        let body = env.new_string(req.body())?;
        let args = [
            jni::objects::JValue::Long(id as jni::sys::jlong),
            jni::objects::JValue::Object(&title),
            jni::objects::JValue::Object(&body),
        ];
        env.call_method(
            &activity,
            jni::jni_str!("requestPrint"),
            jni::jni_sig!((long, java.lang.String, java.lang.String) -> ()),
            &args,
        )?;
        Ok(())
    })
    .is_ok()
}

/// `PrintHelper.nativeOnPrintResult`（Java 側）から呼ばれる JNI エクスポート。
/// 値・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PrintHelper_nativeOnPrintResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::print::bridge::deliver(
        request_id as u64,
        super::print::outcome_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_stub_never_succeeds() {
        let req = PrintRequest::new("見出し", "本文").unwrap();
        assert!(matches!(
            begin_print(1, &req),
            Err(DeviceError::Unsupported("print"))
        ));
    }
}
