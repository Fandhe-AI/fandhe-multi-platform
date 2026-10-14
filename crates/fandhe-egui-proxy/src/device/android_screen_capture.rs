//! Android 向け画面録画（Issue #1337、親 #1135）。`device/screen_capture.rs` の dispatch から
//! `capturer()` 経由で呼ばれる。実体は Java 側 `ScreenCaptureHelper`（同意ダイアログ・
//! ライフサイクル）と `ScreenCaptureService`（`MediaProjection` を持つフォアグラウンド
//! サービス、型 `mediaProjection`）。
//!
//! 流れ: `start`（JNI `MainActivity.requestScreenCaptureStart`）→ 同意 → 録画開始で
//! `nativeOnScreenCaptureStarted` → 停止で `nativeOnScreenCaptureResult(id, code, size, ms)`
//! → [`decode`] で契約の結果へ写像し `bridge::deliver`。コード表は `device/screen_capture.rs` の
//! `error_from_code` が SSOT（Java 側 `ScreenCaptureHelper.RESULT_*` と同じ値）。
//! 0 バイト・0 ms の拒否は `build_capture_result` に一本化する。映像バイトは JNI を越えない。
//!
//! 守る範囲: 1 プロセス内の順序・嘘の成功の禁止・資源の解放。守らない範囲: 映像の中身、
//! OS 同意 UI の見た目、複数プロセス、プロセス死。

use fandhe_device::screen_capture::{build_capture_result, CaptureResult};
use fandhe_device::DeviceError;

/// Java 側が書き出す録画ファイルの mime（JNI では渡さない）。
const MIME: &str = "video/mp4";

/// Java の結果（コード・バイト数・録画時間）を契約の結果へ写像する。負値は 0 扱いで
/// `build_capture_result` に拒否させる。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode(code: i32, size: i64, duration_ms: i64) -> Result<CaptureResult, DeviceError> {
    if code == 0 {
        build_capture_result(size.max(0) as u64, MIME.into(), duration_ms.max(0) as u64)
    } else {
        Err(super::screen_capture::error_from_code(code))
    }
}

#[cfg(target_os = "android")]
struct AndroidScreenCapturer;

#[cfg(target_os = "android")]
impl fandhe_device::screen_capture::ScreenCapturer for AndroidScreenCapturer {
    fn start(&self, request_id: u64, max_duration: std::time::Duration) -> Result<(), DeviceError> {
        let max_ms = max_duration.as_millis().min(i32::MAX as u128) as i32;
        call_activity(request_id, Some(max_ms))
            .map_err(|_| DeviceError::Internal("screen_capture present"))
    }

    fn stop(&self, request_id: u64) -> Result<(), DeviceError> {
        call_activity(request_id, None).map_err(|_| DeviceError::Internal("screen_capture stop"))
    }
}

/// `MainActivity.requestScreenCaptureStart(long, int)`（`max_ms` あり）または
/// `requestScreenCaptureStop(long)` を JNI で呼ぶ。
/// `unsafe` 2 箇所は `android_pedometer.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, max_ms: Option<i32>) -> Result<(), ()> {
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
        let id = jni::objects::JValue::Long(id as jni::sys::jlong);
        match max_ms {
            Some(ms) => env.call_method(
                &activity,
                jni::jni_str!("requestScreenCaptureStart"),
                jni::jni_sig!((long, int) -> ()),
                &[id, jni::objects::JValue::Int(ms)],
            )?,
            None => env.call_method(
                &activity,
                jni::jni_str!("requestScreenCaptureStop"),
                jni::jni_sig!((long) -> ()),
                &[id],
            )?,
        };
        Ok(())
    })
    .map_err(|_| ())
}

/// OS 別エンジンを返す。
#[cfg(target_os = "android")]
pub(crate) fn capturer() -> &'static dyn fandhe_device::screen_capture::ScreenCapturer {
    &AndroidScreenCapturer
}

/// ホストのテストビルド用。`Unsupported` 固定（成功へ倒さない）。
#[cfg(not(target_os = "android"))]
pub(crate) fn capturer() -> &'static dyn fandhe_device::screen_capture::ScreenCapturer {
    &fandhe_device::screen_capture::UnsupportedScreenCapturer
}

/// `ScreenCaptureHelper.nativeOnScreenCaptureStarted`（Java 側）から呼ばれる JNI エクスポート。
/// 実際に録画が始まった合図で、経過時間の起点になる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_ScreenCaptureHelper_nativeOnScreenCaptureStarted<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
) {
    super::screen_capture::bridge::mark_started(request_id as u64);
    crate::android::request_repaint();
}

/// `ScreenCaptureHelper.nativeOnScreenCaptureResult`（Java 側）から呼ばれる JNI エクスポート。
/// 映像内容・パスはログに出さない。
///
/// # Safety
/// 上と同じ根拠（`unsafe` 操作なし、固定シンボル名）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_ScreenCaptureHelper_nativeOnScreenCaptureResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    size_bytes: jni::sys::jlong,
    duration_ms: jni::sys::jlong,
) {
    super::screen_capture::bridge::deliver(
        request_id as u64,
        decode(code, size_bytes, duration_ms),
    );
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_codes() {
        assert!(decode(0, 10, 1_000).is_ok());
        assert!(decode(0, 0, 1_000).is_err());
        assert!(decode(0, 10, 0).is_err());
        assert!(decode(0, -1, 1_000).is_err());
        assert!(matches!(
            decode(1, 0, 0),
            Err(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(decode(4, 0, 0), Err(DeviceError::Cancelled(_))));
        assert!(matches!(decode(6, 0, 0), Err(DeviceError::Unsupported(_))));
        assert!(matches!(decode(99, 5, 5), Err(DeviceError::Internal(_))));
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn stub_is_unsupported() {
        assert!(capturer()
            .start(1, std::time::Duration::from_secs(1))
            .is_err());
    }
}
