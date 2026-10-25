//! Android 向け動画の撮影（Issue #823、親 #169）。`device/video_capture.rs` の Android 経路
//! （`platform`）から呼ばれ、OS 標準のカメラアプリ（`MediaStore.ACTION_VIDEO_CAPTURE`）で撮った動画の
//! 長さ・寸法・サイズを契約（`fandhe_device::video_capture`）どおりに返す。実体は Java 側
//! `VideoCaptureHelper`（許可・起動・結果・メタデータ・一時ファイルの掃除）で、本ファイルは
//! JNI の入口と受け口だけを持つ。
//!
//! 流れ: `capture`（JNI `MainActivity.requestVideoCapture(long, int)`）→ Java が CAMERA の許可を求め、
//! 起動の直前に `nativeTryBeginPresent` → カメラアプリの結果を読んで `nativeOnVideoCaptureResult` →
//! `result_from_code` で結果へ写して `complete_from_os`。数値表の SSOT は
//! `fandhe_device::video_capture::result_from_code`（再実装しない）。
//!
//! JNI 署名は Java の `native` 宣言・本ファイルのエクスポート・`requestVideoCapture` の呼び出しの
//! 3 か所で一致させる。no-op スタブは置かない: Java の native が呼ばれるのは本ファイルが
//! `requestVideoCapture` を呼んだ後だけで、feature `device` が無効なら呼ばれない
//! （`ScreenCaptureHelper` と同じ扱い）。
//!
//! 期限・タイマーは持たず、完了は OS の結果通知だけで決める。パス・OS のエラー文はログに出さない。
//! 守る範囲: 1 プロセス内の順序・嘘の成功の禁止・一時ファイルの解放。守らない範囲:
//! `EXTRA_OUTPUT` を無視するカメラアプリ（空のファイルとして失敗にする）、撮影中の背景遷移、プロセス死。

use std::sync::Arc;

use fandhe_device::video_capture::VideoCapturer;
#[cfg(target_os = "android")]
use fandhe_device::video_capture::{result_from_code, VideoCaptureFailure};

/// Java の `long` のサイズを `u64` へ寄せる。負値は 0 にして、撮れたときは `Empty` に落とす。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn size_from_jlong(size: i64) -> u64 {
    u64::try_from(size).unwrap_or(0)
}

#[cfg(target_os = "android")]
struct AndroidVideoCapturer;

#[cfg(target_os = "android")]
impl VideoCapturer for AndroidVideoCapturer {
    fn capture(&self, request_id: u64, max_duration_secs: u32) -> Result<(), VideoCaptureFailure> {
        let secs = i32::try_from(max_duration_secs).unwrap_or(i32::MAX);
        call_activity(request_id, secs).map_err(|_| VideoCaptureFailure::Internal)
    }
}

/// `MainActivity.requestVideoCapture(long, int)` を JNI で呼ぶ。
/// `unsafe` 2 箇所は `android_screen_capture.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, max_secs: i32) -> Result<(), ()> {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return Err(());
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestVideoCapture"),
            jni::jni_sig!((long, int) -> ()),
            &[
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Int(max_secs),
            ],
        )?;
        Ok(())
    })
    .map_err(|_| ())
}

/// Android の `VideoCapturer` 実装を返す。ホストのテストビルドでは `Unsupported` 固定
/// （成功へ倒さない）。
pub(crate) fn implementation() -> Arc<dyn VideoCapturer> {
    #[cfg(target_os = "android")]
    {
        Arc::new(AndroidVideoCapturer)
    }
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(fandhe_device::video_capture::UnsupportedVideoCapturer)
    }
}

/// `VideoCaptureHelper.nativeTryBeginPresent`（Java 側）から呼ばれる JNI エクスポート。
/// カメラアプリを起動する直前に呼ばれ、`false`（古い要求・記録なし）なら Java は起動しない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_VideoCaptureHelper_nativeTryBeginPresent<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
) -> jni::sys::jboolean {
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let outcome = unowned_env.with_env(|_env| -> Result<bool, jni::errors::Error> {
        Ok(super::video_capture::try_begin_present(request_id as u64))
    });
    matches!(outcome.into_outcome(), jni::Outcome::Ok(true))
}

/// `VideoCaptureHelper.nativeOnVideoCaptureResult`（Java 側）から呼ばれる JNI エクスポート。
/// 結果を 1 回だけ bridge へ渡し、次フレームを起こす（契約表 4 節）。パスは受け取らない。
///
/// # Safety
/// 上と同じ根拠（`unsafe` 操作なし、固定シンボル名）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_VideoCaptureHelper_nativeOnVideoCaptureResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
    size_bytes: jni::sys::jlong,
    duration_ms: jni::sys::jlong,
    width: jni::sys::jint,
    height: jni::sys::jint,
    rotation_deg: jni::sys::jint,
) {
    super::video_capture::complete_from_os(
        request_id as u64,
        result_from_code(
            kind,
            size_from_jlong(size_bytes),
            duration_ms,
            width,
            height,
            rotation_deg,
        ),
    );
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::video_capture::{
        result_from_code, VideoCaptureFailure, VideoCaptureOutcome,
    };

    #[test]
    fn host_implementation_never_succeeds() {
        assert_eq!(
            implementation().capture(1, 30),
            Err(VideoCaptureFailure::Unsupported)
        );
    }

    #[test]
    fn size_from_jlong_clamps_negative_to_zero() {
        assert_eq!(size_from_jlong(-1), 0);
        assert_eq!(size_from_jlong(0), 0);
        assert_eq!(size_from_jlong(42), 42);
    }

    #[test]
    fn receiver_mapping_does_not_fake_success() {
        assert_eq!(
            result_from_code(0, size_from_jlong(0), 1000, 1920, 1080, 0),
            Err(VideoCaptureFailure::Empty)
        );
        assert_eq!(
            result_from_code(0, size_from_jlong(-5), 1000, 1920, 1080, 0),
            Err(VideoCaptureFailure::Empty)
        );
        assert_eq!(
            result_from_code(0, 10, 1000, -1, 1080, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            result_from_code(0, 10, 1000, 1920, 1080, 45),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            result_from_code(99, 10, 1000, 1920, 1080, 0),
            Err(VideoCaptureFailure::Internal)
        );
        assert_eq!(
            result_from_code(1, 0, 0, 0, 0, 0),
            Ok(VideoCaptureOutcome::Cancelled)
        );
    }
}
