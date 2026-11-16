//! Android 向け印刷実装（Issue #328、親 #94）。`driver` の Android 経路（`begin_print`）
//! から呼ばれる。Java 側 `PrintHelper` が `PrintManager` + `PrintDocumentAdapter` で OS の印刷
//! ダイアログを開く。
//!
//! 流れ: `begin_print` → JNI `PrintHelper.print(Activity, id, title, body)`（static）→ Java が
//! `nativeOnPrintResult(id, code)` で返す → `driver::outcome_from_android_code` で契約の結果へ写像
//! → `driver::bridge::deliver` が `SLOT` の id と照合して置く。値は `PrintRequest` の検証を通った
//! ものだけを渡す（Canvas に描くだけで、URL・Intent・ファイルパスには使わない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・取り消しを失敗にしないこと・嘘の成功の禁止・Activity 再生成時
//! の旧要求の破棄。守らない範囲: 紙に出たか・ジョブ投入後のプリンタ側の失敗。権限と `<queries>` は
//! 宣言しない。見出し・本文・結果はログに出さない。
//!
//! 画面の印刷（#1012）: `print_screen_page_effect` → 取り込み確定 → `begin_print_image` → JNI
//! `PrintHelper.printImage(Activity, long, int[], int, int)`（署名 `(Landroid/app/Activity;J[III)V`。Java・`jni_sig!`・本文の
//! 3 か所で一致させる） → 同じ `nativeOnPrintResult`。画素はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の入口（`print` / `printImage`）を経た後でしか呼ばれない。

use crate::contract::PrintRequest;
use crate::error::PrintError;

/// 印刷ダイアログの表示を依頼する。`Ok` は受理済みで、結果は後から `nativeOnPrintResult` で届く。
#[cfg(target_os = "android")]
pub(crate) fn begin_print(request_id: u64, req: &PrintRequest) -> Result<(), PrintError> {
    if call_activity(request_id, req) {
        Ok(())
    } else {
        Err(PrintError::Internal("print present"))
    }
}

/// ホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_print(_request_id: u64, _req: &PrintRequest) -> Result<(), PrintError> {
    Err(PrintError::Unsupported)
}

/// `PrintHelper.print(Activity, long, String, String)`（static）を JNI で呼ぶ。`false` は呼び出し失敗。
/// JavaVM と Activity の取得は `fandhe_platform_android::with_activity_class` に任せ、ここは `unsafe` を持たない。
#[cfg(target_os = "android")]
fn call_activity(id: u64, req: &PrintRequest) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.PrintHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let title = env.new_string(req.title())?;
            let body = env.new_string(req.body())?;
            let args = [
                jni::objects::JValue::Object(activity),
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Object(&title),
                jni::objects::JValue::Object(&body),
            ];
            let r = env.call_static_method(
                class,
                jni::jni_str!("print"),
                jni::jni_sig!((android.app.Activity, long, java.lang.String, java.lang.String) -> ()),
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

/// 画面の画像（print-screen、親 #341）の印刷依頼。`driver` の取り込み確定後に呼ばれる。
/// `Ok` は受理済みで、結果は文章の印刷と同じ `nativeOnPrintResult` で届く。
#[cfg(target_os = "android")]
pub(crate) fn begin_print_image(
    request_id: u64,
    img: &crate::contract::PrintImage,
) -> Result<(), PrintError> {
    if call_activity_image(request_id, img) {
        Ok(())
    } else {
        Err(PrintError::Internal("print present"))
    }
}

/// ホストのテストビルド用。dispatch は Android でしか呼ばない。成功を作らない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_print_image(
    _request_id: u64,
    _img: &crate::contract::PrintImage,
) -> Result<(), PrintError> {
    Err(PrintError::Unsupported)
}

/// `PrintHelper.printImage(Activity, long, int[], int, int)`（static、JNI 署名 `(Landroid/app/Activity;J[III)V`）を呼ぶ。
/// 画素は ARGB の `int[]`。寸法は `PrintImage` の不変条件で各辺 1〜8192 なので `jint`・`jsize` に
/// 収まる。画素・寸法はログに出さない。
#[cfg(target_os = "android")]
fn call_activity_image(id: u64, img: &crate::contract::PrintImage) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.PrintHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let argb = img.to_argb();
            let arr = env.new_int_array(argb.len())?;
            arr.set_region(env, 0, &argb)?;
            let args = [
                jni::objects::JValue::Object(activity),
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Object(&arr),
                jni::objects::JValue::Int(img.width() as jni::sys::jint),
                jni::objects::JValue::Int(img.height() as jni::sys::jint),
            ];
            let r = env.call_static_method(
                class,
                jni::jni_str!("printImage"),
                jni::jni_sig!((android.app.Activity, long, int[], int, int) -> ()),
                &args,
            );
            if r.is_err() && env.exception_check() {
                env.exception_clear();
            }
            r?;
            Ok(())
        },
    )
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_PrintHelper_nativeOnPrintResult<'caller>(
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
        let req = PrintRequest::new("見出し", "本文").unwrap();
        assert!(matches!(begin_print(1, &req), Err(PrintError::Unsupported)));
    }

    #[test]
    fn host_stub_image_never_succeeds() {
        let img = crate::contract::PrintImage::new(vec![0; 4], 1, 1).unwrap();
        assert!(matches!(
            begin_print_image(1, &img),
            Err(PrintError::Unsupported)
        ));
    }
}
