//! Android 向けカメラ実装。`MainActivity.requestCameraCapture(long)` を JNI で呼び、結果を
//! `CameraCaptureHelper.nativeOnCameraResult`（Java 側）から受ける。
//!
//! 呼び出し元は `crate::platform_camera`（`cfg(target_os = "android")` 分岐）。`AndroidApp` の取得と
//! JavaVM への attach・Activity の取得は `fandhe-platform-android` の `with_activity` に任せ、
//! 本モジュールに `unsafe` を残すのは JNI エクスポート関数 1 つだけにする。

use std::time::Duration;

use fandhe_platform_android::{JniCallFailed, android_app, with_activity};

use crate::mailbox::CaptureMailbox;
use crate::{Camera, CameraError, Image, MAX_IMAGE_BYTES};

/// 撮影 1 件を仲介する mailbox（プロセス内で単一飛行、`CaptureMailbox` 参照）。
/// Activity 再生成時に届く完了は旧 `ProxyApp` の `CompletionSender` へ送られ捨てられるが、
/// Java 側が status 7 で中断決着させるため新画面が引き継ぐべき実結果は存在しない
/// （#881、`lib.rs` のモジュール doc 参照）。
static CAPTURE_MAILBOX: CaptureMailbox = CaptureMailbox::new();

/// JNI コールバック待ちの上限（ponytail: カメラアプリの起動・撮影・戻りは通常数秒
/// 〜数十秒で終わる。ユーザーがカメラアプリを開いたまま放置するケースへの安全弁で、
/// 値自体に強い実測根拠はない。実機で `Capturing` 固着の報告が出たら見直す）。
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(300);

/// `Camera::capture()` を Java 側 `MainActivity.requestCameraCapture()` へ委譲する実装（Issue #750）。
pub struct AndroidCamera;

impl AndroidCamera {
    /// `AndroidCamera` を構築する。内部状態は持たず、`capture()` の都度 `android_app()` から
    /// 最新の `AndroidApp` を取得する（Activity 再生成で差し替わるため）。
    pub fn new() -> Self {
        Self
    }
}

impl Default for AndroidCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera for AndroidCamera {
    fn capture(&self) -> Result<Image, CameraError> {
        // 要求 ID を発行し、JNI 往復（`requestCameraCapture(long)` → Java 側が保持 →
        // `nativeOnCameraResult` で返却）に乗せて運ぶ。`deliver` はこの ID が一致する結果のみ
        // 受理するため、タイムアウト後に届いた古い要求のコールバックが新しい要求の結果と
        // 取り違えられない（`CaptureMailbox` doc 参照、codex-review 指摘 PR #790）。
        let Some(request_id) = CAPTURE_MAILBOX.begin() else {
            // UI 側は Requested → Capturing の二段階状態機械のため通常はここへ
            // 到達しないが、多重呼び出しに対する防御として単一飛行を守る。
            return Err(CameraError::Unsupported("camera_busy"));
        };

        let Some(app) = android_app() else {
            CAPTURE_MAILBOX.cancel(request_id);
            return Err(CameraError::Unsupported("android_app_unavailable"));
        };

        let request_result = with_activity(&app, |env, activity| -> Result<(), JniCallFailed> {
            env.call_method(
                activity,
                jni::jni_str!("requestCameraCapture"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
            )?;
            Ok(())
        });

        if request_result.is_err() {
            // JNI 呼び出し自体が失敗した（メソッド未検出・例外送出等）。コールバックは
            // 来ないため `deliver` を待たず即座にスロットを解放する。
            CAPTURE_MAILBOX.cancel(request_id);
            return Err(CameraError::Unsupported("jni_call_failed"));
        }

        CAPTURE_MAILBOX.wait(request_id, CAPTURE_TIMEOUT)
    }
}

/// Java 側から届いた撮影結果を `Image` へ変換する（`nativeOnCameraResult` 本体）。
/// `status` の意味は `CameraCaptureHelper`（Java 側実装）と対で決める:
/// 0 = 成功、1 = 権限拒否、2 = カメラアプリなし、3 = キャンセル、
/// 4 = 旧要求が未完了のため Java 側が新要求を拒否（`awaitingCameraResult` doc 参照）、
/// 5 = 画像サイズが上限超過（Java 側 `deliverBitmap` の配列確保前検査で検出）、
/// 6 = `launch()` 自体が想定外の実行時例外で失敗、7 = Activity 再生成により中断
/// （進行中の撮影要求が再生成前の Activity インスタンスに紐づいていたため結果を引き継げない）。
/// それ以外は未知の失敗。
fn decode_capture_result(
    env: &jni::Env,
    rgba: &jni::objects::JByteArray,
    width: jni::sys::jint,
    height: jni::sys::jint,
    status: jni::sys::jint,
) -> Result<Image, CameraError> {
    let fallback_size = (width.max(0) as u32, height.max(0) as u32);
    match status {
        0 => {}
        1 => return Err(CameraError::PermissionDenied("camera")),
        2 => return Err(CameraError::Unsupported("camera_app_not_found")),
        3 => return Err(CameraError::Unsupported("camera_cancelled")),
        4 => return Err(CameraError::Unsupported("camera_busy")),
        5 => {
            return Err(CameraError::InvalidImageSize(
                fallback_size.0,
                fallback_size.1,
            ));
        }
        6 => return Err(CameraError::Unsupported("camera_launch_failed")),
        7 => return Err(CameraError::Unsupported("camera_interrupted_by_recreation")),
        _ => return Err(CameraError::Unsupported("camera_unknown_status")),
    }

    let (width, height) = match (u32::try_from(width), u32::try_from(height)) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
        _ => {
            return Err(CameraError::InvalidImageSize(
                fallback_size.0,
                fallback_size.1,
            ));
        }
    };

    // 変換前（`convert_byte_array` が Vec を確保する前）に配列長を検査する
    // （security.md A05、メモリ枯渇防止）。
    let len = rgba
        .len(env)
        .map_err(|_| CameraError::InvalidImageSize(width, height))?;
    if len > MAX_IMAGE_BYTES {
        return Err(CameraError::InvalidImageSize(width, height));
    }

    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(CameraError::InvalidImageSize(width, height))?;
    if len != expected_len {
        return Err(CameraError::InvalidImageSize(width, height));
    }

    let bytes = env
        .convert_byte_array(rgba)
        .map_err(|_| CameraError::InvalidImageSize(width, height))?;

    Ok(Image {
        width,
        height,
        rgba: bytes,
    })
}

/// `CameraCaptureHelper.nativeOnCameraResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// シンボル名はパッケージ名 `com.fandhe.platform` とクラス名 `CameraCaptureHelper` に
/// 固定で紐づく（JNI の命名規約。クラス移動・rename 時は Java 側も追随が必要）。
///
/// [`decode_capture_result`] で `Image`/`CameraError` へ変換し、[`CAPTURE_MAILBOX`] へ
/// 届ける（`AndroidCamera::capture` の `wait` がこれを受け取る）。`request_id` は
/// `AndroidCamera::capture` が発行し `requestCameraCapture(long)` へ渡した値を Java 側が
/// そのまま持ち回って返したもの。`deliver` が ID 不一致を無視するため、タイムアウト後に
/// 届いた古い要求のコールバックが新しい要求の結果を上書きしない。
///
/// 機能 crate の rlib 内のシンボルは最終成果物（cdylib）へ出る必要があるため
/// `#[unsafe(no_mangle)]` で名前を固定する（`scripts/android/verify-android-artifacts.sh` が
/// `.so` に存在することを確かめる）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`no_mangle` は JNI 命名規約上の
/// 固定シンボル名であり、アプリは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（`.claude/rules/coding-rust.md` の FFI 例外規定準拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_CameraCaptureHelper_nativeOnCameraResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    rgba: jni::objects::JByteArray<'caller>,
    width: jni::sys::jint,
    height: jni::sys::jint,
    status: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_capture_result(env, &rgba, width, height, status);
        CAPTURE_MAILBOX.deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。本関数は
    // Java 側へ戻り値も例外も返さない契約のため、結果は握りつぶす。
    let _ = outcome.into_outcome();
}
