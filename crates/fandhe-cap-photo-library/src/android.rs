//! Android 向け写真ライブラリ保存（Issue #1256）。`driver.rs` の Android 経路から
//! 呼ばれ、Java 側 `PhotoSaveHelper`（static 入口）へ PNG を渡して
//! MediaStore（API 29+、権限不要）に保存する。結果は `nativeOnPhotoSaveResult` が
//! `photo_save::bridge::deliver` へ pull 型で届ける。API 28 以下は `Unsupported`。
//! 結果コード表の SSOT は Java 側 `PhotoSaveHelper`。`unsafe` は JNI 境界の 2 箇所だけ。

use crate::contract::SavedTo;
use crate::error::PhotoLibraryError;

/// Java 側 `PhotoSaveHelper.RESULT_*` を契約上の結果へ写像する。未知のコードは成功にしない。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | `IS_PENDING=0` で確定 | `Ok(SavedTo::MediaStore)` |
/// | 1 | API 28 以下 | `Unsupported("photo_library")` |
/// | 2 | insert・書き込み・確定の失敗（片付け済み） | `Internal("photo_library save")` |
/// | 3 | 片付け（delete）の失敗 | `Internal("photo_library cleanup")` |
/// | 他 | 未知 | `Internal("photo_library outcome")` |
pub(crate) fn outcome_from_android_code(code: i32) -> Result<SavedTo, PhotoLibraryError> {
    match code {
        0 => Ok(SavedTo::MediaStore),
        1 => Err(PhotoLibraryError::Unsupported("photo_library")),
        2 => Err(PhotoLibraryError::Internal("photo_library save")),
        3 => Err(PhotoLibraryError::Internal("photo_library cleanup")),
        _ => Err(PhotoLibraryError::Internal("photo_library outcome")),
    }
}

#[cfg(target_os = "android")]
use crate::contract::PngImage;

/// 保存を依頼する。`Ok` は受理済み（結果は後から `bridge::deliver` で届く）。
/// `PhotoSaveHelper.save(Context, long, byte[])` を JNI の static 呼び出しで呼ぶだけで即座に戻る。
/// JavaVM と Activity の取得は `with_activity_class` に任せ、ここは `unsafe` を持たない。
/// 画像バイト列はログへ出さない（security.md A09）。
#[cfg(target_os = "android")]
pub(crate) fn begin_save(request_id: u64, image: &PngImage) -> Result<(), PhotoLibraryError> {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    use jni::objects::JValue;
    let present = PhotoLibraryError::Internal("photo_library present");
    let Some(app) = android_app() else {
        return Err(present);
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.PhotoSaveHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let j_png = env.byte_array_from_slice(image.bytes())?;
            env.call_static_method(
                class,
                jni::jni_str!("save"),
                jni::jni_sig!((android.content.Context, long, byte[]) -> ()),
                &[
                    JValue::Object(activity),
                    JValue::Long(request_id as jni::sys::jlong),
                    JValue::Object(&j_png),
                ],
            )?;
            Ok(())
        },
    )
    .map_err(|_| present)
}

/// `PhotoSaveHelper.nativeOnPhotoSaveResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// 旧要求の遅延到着は `deliver` が id 不一致で捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_PhotoSaveHelper_nativeOnPhotoSaveResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    crate::driver::bridge::deliver(request_id as u64, outcome_from_android_code(code));
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_from_android_code_table() {
        assert!(matches!(
            outcome_from_android_code(0),
            Ok(SavedTo::MediaStore)
        ));
        assert!(matches!(
            outcome_from_android_code(1),
            Err(PhotoLibraryError::Unsupported("photo_library"))
        ));
        for code in [2, 3, 4, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_android_code(code),
                Err(PhotoLibraryError::Internal(_))
            ));
        }
    }
}
