//! Android 向け画面録画（Issue #1337、親 #1135）。`fandhe-cap-screen-capture/src/driver.rs` の dispatch から
//! `capturer()` 経由で呼ばれる。実体は Java 側 `ScreenCaptureHelper`（同意ダイアログ・
//! ライフサイクル）と `ScreenCaptureService`（`MediaProjection` を持つフォアグラウンド
//! サービス、型 `mediaProjection`）。
//!
//! 流れ: `start`（JNI `ScreenCaptureHelper.start`）→ 同意 → 録画開始で
//! `nativeOnScreenCaptureStarted` → 停止で `nativeOnScreenCaptureResult(id, code, size, ms)`
//! → [`decode`] で契約の結果へ写像し `bridge::deliver`。コード表は `fandhe-cap-screen-capture/src/driver.rs` の
//! `error_from_code` が SSOT（Java 側 `ScreenCaptureHelper.RESULT_*` と同じ値）。
//! 0 バイト・0 ms の拒否は `build_capture_result` に一本化する。映像バイトは JNI を越えない。
//!
//! 守る範囲: 1 プロセス内の順序・嘘の成功の禁止・資源の解放。守らない範囲: 映像の中身、
//! OS 同意 UI の見た目、複数プロセス、プロセス死。

use crate::contract::{CaptureResult, build_capture_result};
use crate::error::ScreenCaptureError;

/// Java 側が書き出す録画ファイルの mime（JNI では渡さない）。
const MIME: &str = "video/mp4";

/// Java の結果（コード・バイト数・録画時間）を契約の結果へ写像する。負値は 0 扱いで
/// `build_capture_result` に拒否させる。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode(code: i32, size: i64, duration_ms: i64) -> Result<CaptureResult, ScreenCaptureError> {
    if code == 0 {
        build_capture_result(size.max(0) as u64, MIME.into(), duration_ms.max(0) as u64)
    } else {
        Err(crate::driver::error_from_code(code))
    }
}

#[cfg(target_os = "android")]
struct AndroidScreenCapturer;

#[cfg(target_os = "android")]
impl crate::contract::ScreenCapturer for AndroidScreenCapturer {
    fn start(
        &self,
        request_id: u64,
        max_duration: std::time::Duration,
    ) -> Result<(), ScreenCaptureError> {
        let max_ms = max_duration.as_millis().min(i32::MAX as u128) as i32;
        call_activity(request_id, Some(max_ms))
            .map_err(|_| ScreenCaptureError::Internal("screen_capture"))
    }

    fn stop(&self, request_id: u64) -> Result<(), ScreenCaptureError> {
        call_activity(request_id, None).map_err(|_| ScreenCaptureError::Internal("screen_capture"))
    }
}

/// `ScreenCaptureHelper.start(Activity, long, int)`（`max_ms` あり）または
/// `stop(Activity, long)`（static）を JNI で呼ぶ。
/// `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じている。
#[cfg(target_os = "android")]
fn call_activity(id: u64, max_ms: Option<i32>) -> Result<(), ()> {
    use fandhe_platform_android::{JniCallFailed, with_activity_class};
    let Some(app) = fandhe_platform_android::android_app() else {
        return Err(());
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ScreenCaptureHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let id = jni::objects::JValue::Long(id as jni::sys::jlong);
            let a = jni::objects::JValue::Object(activity);
            let r = match max_ms {
                Some(ms) => env.call_static_method(
                    class,
                    jni::jni_str!("start"),
                    jni::jni_sig!((android.app.Activity, long, int) -> ()),
                    &[a, id, jni::objects::JValue::Int(ms)],
                ),
                None => env.call_static_method(
                    class,
                    jni::jni_str!("stop"),
                    jni::jni_sig!((android.app.Activity, long) -> ()),
                    &[a, id],
                ),
            };
            if r.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            r?;
            Ok(())
        },
    )
    .map_err(|_| ())
}

/// OS 別エンジンを返す。
#[cfg(target_os = "android")]
pub(crate) fn capturer() -> &'static dyn crate::contract::ScreenCapturer {
    &AndroidScreenCapturer
}

/// ホストのテストビルド用。`Unsupported` 固定（成功へ倒さない）。
#[cfg(not(target_os = "android"))]
pub(crate) fn capturer() -> &'static dyn crate::contract::ScreenCapturer {
    &crate::contract::UnsupportedScreenCapturer
}

/// `ScreenCaptureHelper.nativeOnScreenCaptureStarted`（Java 側）から呼ばれる JNI エクスポート。
/// 実際に録画が始まった合図で、経過時間の起点になる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ScreenCaptureHelper_nativeOnScreenCaptureStarted<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
) {
    crate::driver::bridge::mark_started(request_id as u64);
    fandhe_runtime::request_repaint();
}

/// `ScreenCaptureHelper.nativeOnScreenCaptureResult`（Java 側）から呼ばれる JNI エクスポート。
/// 映像内容・パスはログに出さない。
///
/// # Safety
/// 上と同じ根拠（`unsafe` 操作なし、固定シンボル名）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ScreenCaptureHelper_nativeOnScreenCaptureResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    size_bytes: jni::sys::jlong,
    duration_ms: jni::sys::jlong,
) {
    crate::driver::bridge::deliver(request_id as u64, decode(code, size_bytes, duration_ms));
    fandhe_runtime::request_repaint();
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
            Err(ScreenCaptureError::PermissionDenied)
        ));
        assert!(matches!(
            decode(4, 0, 0),
            Err(ScreenCaptureError::Cancelled)
        ));
        assert!(matches!(
            decode(6, 0, 0),
            Err(ScreenCaptureError::Unsupported)
        ));
        assert!(matches!(
            decode(99, 5, 5),
            Err(ScreenCaptureError::Internal("screen_capture"))
        ));
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn stub_is_unsupported() {
        assert!(
            capturer()
                .start(1, std::time::Duration::from_secs(1))
                .is_err()
        );
    }
}
