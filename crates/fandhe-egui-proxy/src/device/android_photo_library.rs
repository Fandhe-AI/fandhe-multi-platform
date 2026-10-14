//! Android 向け写真ライブラリ保存（Issue #1256）。`device/photo_save.rs` の Android 経路から
//! 呼ばれ、Java 側 `PhotoSaveHelper`（`MainActivity.requestPhotoSave` 経由）へ PNG を渡して
//! MediaStore（API 29+、権限不要）に保存する。結果は `nativeOnPhotoSaveResult` が
//! `photo_save::bridge::deliver` へ pull 型で届ける。API 28 以下は `Unsupported`。
//! 結果コード表の SSOT は Java 側 `PhotoSaveHelper`。`unsafe` は JNI 境界の 2 箇所だけ。

use fandhe_device::photo_library::SavedTo;
use fandhe_device::DeviceError;

/// Java 側 `PhotoSaveHelper.RESULT_*` を契約上の結果へ写像する。未知のコードは成功にしない。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | `IS_PENDING=0` で確定 | `Ok(SavedTo::MediaStore)` |
/// | 1 | API 28 以下 | `Unsupported("photo_library")` |
/// | 2 | insert・書き込み・確定の失敗（片付け済み） | `Internal("photo_library save")` |
/// | 3 | 片付け（delete）の失敗 | `Internal("photo_library cleanup")` |
/// | 他 | 未知 | `Internal("photo_library outcome")` |
pub(crate) fn outcome_from_android_code(code: i32) -> Result<SavedTo, DeviceError> {
    match code {
        0 => Ok(SavedTo::MediaStore),
        1 => Err(DeviceError::Unsupported("photo_library")),
        2 => Err(DeviceError::Internal("photo_library save")),
        3 => Err(DeviceError::Internal("photo_library cleanup")),
        _ => Err(DeviceError::Internal("photo_library outcome")),
    }
}

#[cfg(target_os = "android")]
use fandhe_device::photo_library::PngImage;

/// 保存を依頼する。`Ok` は受理済み（結果は後から `bridge::deliver` で届く）。
/// 画像バイト列はログへ出さない（security.md A09）。
// 本体内の 2 箇所の `unsafe` は `request_calendar_insert` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
pub(crate) fn begin_save(request_id: u64, image: &PngImage) -> Result<(), DeviceError> {
    use super::android::JniCallFailed;
    let present = DeviceError::Internal("photo_library present");
    let Some(app) = crate::android::android_app() else {
        return Err(present);
    };

    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する（`request_calendar_insert` と同一）。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_png = env.byte_array_from_slice(image.bytes())?;
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない
        // （`request_calendar_insert` と同一）。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestPhotoSave"),
            jni::jni_sig!((long, byte[]) -> ()),
            &[
                jni::objects::JValue::Long(request_id as jni::sys::jlong),
                jni::objects::JValue::Object(&j_png),
            ],
        )?;
        Ok(())
    })
    .map_err(|_| present)
}

/// `PhotoSaveHelper.nativeOnPhotoSaveResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// 旧要求の遅延到着は `deliver` が id 不一致で捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PhotoSaveHelper_nativeOnPhotoSaveResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::photo_save::bridge::deliver(request_id as u64, outcome_from_android_code(code));
    crate::android::request_repaint();
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
            Err(DeviceError::Unsupported("photo_library"))
        ));
        for code in [2, 3, 4, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_android_code(code),
                Err(DeviceError::Internal(_))
            ));
        }
    }
}
