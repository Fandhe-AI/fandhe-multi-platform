//! Android 向け写真選択（Issue #1104、Phase 11 型サンプル「写真選択」の Android 経路）。`crate::driver` の Android 経路から呼ばれ、
//! 提示は `PhotoPickerHelper.request(Activity, long)`（static）、結果は `nativeOnPhotoPickerResult` で 1 度だけ届く pull 型 bridge。
//!
//! ファイル選択（`fandhe-cap-file-picker`）と同じ形で、`PhotoPicker` トレイトは通らない。Java 側の責務は「提示 → requestId 付きで
//! 1 回だけ通知 → 一時ファイルの後始末」と、JPEG / PNG 以外の JPEG への変換だけ。8 MiB 判定・読み取りは
//! `fandhe_device::file_picker::picked_from_document`（`read_picked_from` に委譲）に一本化し、Java 側で読み取りループを書かない。
//! fd はパス文字列ではなく `ParcelFileDescriptor.detachFd()` の生 fd で受け取る。結果は `crate::driver::bridge::deliver` が
//! 2 段検査（`validate_photo` → 復号確認）まで済ませてから置く（Java のワーカースレッド上で走り、UI スレッドでは復号しない）。
//!
//! `unsafe` は fd の所有権を取る `OwnedFd::from_raw_fd` の 1 行と JNI 受け口の `#[unsafe(no_mangle)]` だけで、JNI 呼び出しは共通層
//! `fandhe_platform_android::with_activity_class` に閉じている。選んだ写真の内容・名前・Uri はログに出さない（security.md A09）。

use std::os::fd::{FromRawFd, OwnedFd};

use fandhe_device::file_picker::{PickedFile, picked_from_document};
use fandhe_platform_android::{JniCallFailed, with_activity_class};

use crate::error::PhotoPickerError;

/// Java 側 `PhotoPickerHelper.request(Activity, long)` を呼び、ピッカーの提示だけを要求する。結果を待たずに即座に戻る。
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元が
/// `Err(PhotoPickerError::Internal("photo_picker present"))` を `deliver` へ渡して畳む）。Helper が無い（生成前・破棄後）要求は
/// Java 側が `nativeOnPhotoPickerResult` で失敗（code 2）を返して確定する。署名 `(Landroid/app/Activity;J)V` は Java 側の定義と一致させる。
pub(crate) fn request_photo_pick(request_id: u64) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.PhotoPickerHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let r = env.call_static_method(
                class,
                jni::jni_str!("request"),
                jni::jni_sig!((android.app.Activity, long) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                ],
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

/// Java 側から届いた `code`/`name`/`fd` を `PickedFile` へ変換する（`nativeOnPhotoPickerResult` 本体）。`code` の意味は
/// Java 側 `PhotoPickerHelper` の宣言コメントと対: 0=選択（name/fd 必須）／1=キャンセル／2=失敗（`launch()` 例外・fd 取得失敗・
/// JPEG への変換失敗・復号不能・長辺超過のいずれも一律ここへ畳む）。検証と読み取りは `picked_from_document` に委譲する
/// （写真選択固有の 2 段検査は `bridge::deliver` 側）。
///
/// `fd` の所有権を、名前の検証より **先に** 取る（後続のどの失敗経路でも `file` の drop で必ず 1 回 close され、detach 済み fd が
/// リークしない）。
#[allow(unsafe_code)]
fn decode_photo_pick_result(
    env: &jni::Env,
    code: jni::sys::jint,
    name: &jni::objects::JString,
    fd: jni::sys::jint,
) -> Result<Option<PickedFile>, PhotoPickerError> {
    let file = if code == 0 && fd >= 0 {
        // SAFETY: Java 側 `PhotoPickerHelper.deliverPickedPhoto` が `ParcelFileDescriptor.detachFd()` で所有権を手放した直後に渡す
        // 有効な fd で（`code == 0` のときだけ有効）、detach 後は Java 側で close しない契約。`OwnedFd` に包むことで、この関数を
        // 抜ける際（成功なら `picked_from_document` が読み切った直後、失敗なら早期 return 時の `File` の drop）に確実に 1 回だけ
        // close される。呼び出し元は他のどこにもこの fd を渡さないため、二重 close・use-after-close は発生しない。
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        Some(std::fs::File::from(owned))
    } else {
        None
    };
    // modified UTF-8 → Rust の `String` 変換（jni 0.22 の非 deprecated API）。キャンセル・失敗では名前を見ない。
    let raw_name = if code == 0 && !name.is_null() {
        name.try_to_string(env).ok()
    } else {
        None
    };
    picked_from_document(code, file, raw_name.as_deref()).map_err(PhotoPickerError::from)
}

/// `PhotoPickerHelper.nativeOnPhotoPickerResult`（Java 側）から呼ばれる JNI エクスポート関数。引数の意味は
/// `fandhe-cap-file-picker` の `nativeOnFilePickerResult` と同じ。[`decode_photo_pick_result`] で `PickedFile` にし、写真選択の
/// bridge へ届ける。`request_id` が現在の要求と不一致（再生成後の遅延到着等）なら `deliver` が捨てる。受け取った内容は
/// ログ出力・保存しない（A09）。pull 型のため `deliver` の成否に関わらず再描画を要求する。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`fd` の `unsafe` な受け取りは [`decode_photo_pick_result`] に閉じる）。
/// `#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_PhotoPickerHelper_nativeOnPhotoPickerResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_photo_pick_result(env, code, &name, fd);
        crate::driver::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    fandhe_runtime::request_repaint();
}
