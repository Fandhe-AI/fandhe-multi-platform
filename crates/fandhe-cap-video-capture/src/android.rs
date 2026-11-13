//! Android 向け動画の撮影（Issue #823、親 #169）。`driver` の Android 経路
//! （`platform`）から呼ばれ、OS 標準のカメラアプリ（`MediaStore.ACTION_VIDEO_CAPTURE`）で撮った動画の
//! 長さ・寸法・サイズを契約（`crate::contract`）どおりに返す。実体は Java 側
//! `VideoCaptureHelper`（許可・起動・結果・メタデータ・一時ファイルの掃除）で、本ファイルは
//! JNI の入口と受け口だけを持つ。
//!
//! 流れ: `capture`（JNI、static の `VideoCaptureHelper.capture(Activity, long, int)`）→ Java が CAMERA の許可を求め、
//! 起動の直前に `nativeTryBeginPresent` → カメラアプリの結果を読んで `nativeOnVideoCaptureResult` →
//! `result_from_code` で結果へ写して `complete_from_os`。数値表の SSOT は
//! `crate::contract::result_from_code`（再実装しない）。
//!
//! JNI 署名は Java の宣言（`capture`・`native` 2 つ）と本ファイル（`call_activity`・エクスポート）の 2 か所で一致させる。
//! no-op スタブは置かない: Java の native が呼ばれるのは本ファイルが `capture` を呼んだ後だけで、
//! 機能を選ばなければ Java ごと APK に入らない。
//!
//! 期限・タイマーは持たず、完了は OS の結果通知だけで決める。パス・OS のエラー文はログに出さない。
//! 守る範囲: 1 プロセス内の順序・嘘の成功の禁止・一時ファイルの解放。守らない範囲:
//! `EXTRA_OUTPUT` を無視するカメラアプリ（空のファイルとして失敗にする）、撮影中の背景遷移、プロセス死。

use std::sync::Arc;

use crate::contract::VideoCapturer;
#[cfg(target_os = "android")]
use crate::contract::{VideoCaptureFailure, result_from_code};

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

/// `VideoCaptureHelper.capture(Activity, long, int)`（static）を JNI で呼ぶ。クラスは Activity の class loader で
/// 解決する（`fandhe_platform_android::with_activity_class`）。Java が `false`（helper が無い）を返した・JNI に失敗した
/// ときは `Err` にし、結果の native が呼ばれないまま待たせない。
#[cfg(target_os = "android")]
fn call_activity(id: u64, max_secs: i32) -> Result<(), ()> {
    use fandhe_platform_android::JniCallFailed;
    let app = fandhe_platform_android::android_app().ok_or(())?;
    let accepted = fandhe_platform_android::with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.VideoCaptureHelper"),
        |env, activity, class| -> Result<bool, JniCallFailed> {
            let r = env.call_static_method(
                class,
                jni::jni_str!("capture"),
                jni::jni_sig!((android.app.Activity, long, int) -> boolean),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(id as jni::sys::jlong),
                    jni::objects::JValue::Int(max_secs),
                ],
            );
            if r.is_err() && env.exception_check() {
                env.exception_clear();
            }
            Ok(r?.z()?)
        },
    );
    match accepted {
        Ok(true) => Ok(()),
        _ => Err(()),
    }
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
        Arc::new(crate::contract::UnsupportedVideoCapturer)
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_VideoCaptureHelper_nativeTryBeginPresent<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
) -> jni::sys::jboolean {
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let outcome = unowned_env.with_env(|_env| -> Result<bool, jni::errors::Error> {
        Ok(crate::driver::try_begin_present(request_id as u64))
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_VideoCaptureHelper_nativeOnVideoCaptureResult<
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
    crate::driver::complete_from_os(
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
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{VideoCaptureFailure, VideoCaptureOutcome, result_from_code};

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
