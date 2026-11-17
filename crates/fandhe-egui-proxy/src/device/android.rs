//! Phase 8（端末機能、Issue #750）: Android 向け `DataDir` 解決と各機能の JNI 経由呼び出し
//! （カメラ撮影は機能 crate `fandhe-cap-camera-capture` へ、読み上げ・ファイル選択・マイク録音・音声認識は機能 crate
//! `fandhe-cap-tts`・`fandhe-cap-file-picker`・`fandhe-cap-mic-record`・`fandhe-cap-speech-recognition` へ移した、
//! Issue #1170・#1354。ここに残るのは写真選択の JNI だけ）。呼び出し元は `crate::device`（`storage_note_state` の Android 分岐）などで、
//! `feature = "device"` 有効時のみコンパイル対象になる（`device.rs` の `mod` 宣言参照）。
//!
//! `AndroidApp` の取得は `crate::android::android_app()`（`android_main` が `eframe` へ
//! move する前に static へ複製したもの、`android.rs` 参照）を通す。`fandhe-core` は
//! 本モジュールを一切参照しない（依存方向はプロキシ → コアの一方向、coding-rust.md 準拠）。

use std::os::fd::{FromRawFd, OwnedFd};

use fandhe_device::file_picker::{picked_from_document, PickedFile};
use fandhe_device::DeviceError;

/// JNI 呼び出し中のエラーを一時的にまとめる内部専用の型。`jni::errors::Error` から
/// `From` 変換できる必要がある（`JavaVM::attach_current_thread` の型制約）が、
/// `fandhe_device::DeviceError` はこの変換を持たない（`fandhe-device` を変更しない方針）
/// ため、本モジュール内に閉じた変換先として用意する。`pub(crate)`: `android_background_task.rs`
/// も同じ JNI 呼び出し形（`fandhe-cap-camera-capture` の `AndroidCamera::capture` と同型）で再利用する
/// （Issue #1046、二重実装を避ける）。
pub(crate) use fandhe_platform_android::JniCallFailed;

/// Java 側から届いた `code`/`name`/`fd` を `PickedFile` へ変換する（`nativeOnPhotoPickerResult` 本体）。`code` の意味は
/// `fandhe-cap-file-picker` の `decode_file_pick_result` と同じで、検証と読み取りは
/// `fandhe_device::file_picker::picked_from_document` に委譲する（写真選択固有の 2 段検査は `photo_pick::bridge::deliver` 側）。
///
/// `fd` の所有権を、名前の検証より **先に** 取る（後続のどの失敗経路でも `file` の drop で必ず 1 回 close され、detach 済み fd が
/// リークしない。codex P1）。
#[allow(unsafe_code)]
fn decode_photo_pick_result(
    env: &jni::Env,
    code: jni::sys::jint,
    name: &jni::objects::JString,
    fd: jni::sys::jint,
) -> Result<Option<PickedFile>, DeviceError> {
    let file = if code == 0 && fd >= 0 {
        // SAFETY: Java 側 `MainActivity.deliverPickedPhoto` が `ParcelFileDescriptor.detachFd()` で所有権を手放した直後に渡す
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
    picked_from_document(code, file, raw_name.as_deref())
}

// --------------------------------------------------------------------------
// 写真選択（Issue #1104、Phase 11 型サンプル「写真選択」の Android 経路）
// --------------------------------------------------------------------------
//
// ファイル選択（`fandhe-cap-file-picker`）と同じ pull 型 bridge。提示は `requestPhotoPick(long)`、結果は
// `nativeOnPhotoPickerResult` で 1 度だけ届く。fd 受け取り・読み取り・上限判定は
// [`decode_photo_pick_result`] / `fandhe_device::file_picker::picked_from_document` を共有する（name は Java 側の固定値
// `"photo"`。画面にもログにも出さない）。結果は `device/photo_pick.rs::bridge::deliver`
// が 2 段検査（`validate_photo` → 復号確認）まで済ませてから置く（Java のワーカー
// スレッド上で走り、UI スレッドでは復号しない）。

/// Java 側 `MainActivity.requestPhotoPick(long)` を呼び、ピッカーの提示だけを要求する。
/// `fandhe-cap-file-picker` の `request_file_pick` と同じ JNI 呼び出しの形（結果を待たず即 return）。戻り値は JNI
/// 呼び出し自体の成否のみ（`false` は呼び出し元が
/// `Err(DeviceError::Internal("photo_picker present"))` として畳む）。署名 `(J)V` は
/// Java 側の定義と一致させる。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `fandhe-cap-camera-capture` の `AndroidCamera::capture` と同一の根拠。
#[allow(unsafe_code)]
pub(crate) fn request_photo_pick(request_id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `fandhe-cap-camera-capture` の `AndroidCamera::capture` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*`。ローカル変数 `app` が呼び出し中の生存を保つ。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `fandhe-cap-camera-capture` の `AndroidCamera::capture` と同一の根拠。`activity_as_ptr()` は `AndroidApp`
            // が保持する未所有のグローバル参照で、`Cast` は所有権を持たず delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestPhotoPick"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `MainActivity.nativeOnPhotoPickerResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// 引数の意味は `fandhe-cap-file-picker` の `nativeOnFilePickerResult` と同じ（`code`: 0=選択（name/fd 有効）／
/// 1=キャンセル／2=失敗。JPEG / PNG 以外の変換失敗は Java 側で 2 に畳む）。
/// [`decode_photo_pick_result`] で `PickedFile` にし、写真選択の bridge へ届ける。`request_id`
/// が現在の要求と不一致（再生成後の遅延到着等）なら `deliver` が捨てる。受け取った内容は
/// ログ出力・保存しない（A09）。pull 型のため `deliver` の成否に関わらず再描画を要求する。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`fd` の `unsafe` な受け取りは
/// [`decode_photo_pick_result`] に閉じる）。`#[no_mangle]` は JNI 命名規約上の固定シンボル名
/// で、他シンボルと衝突しない（本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_platform_MainActivity_nativeOnPhotoPickerResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_photo_pick_result(env, code, &name, fd);
        crate::device::photo_pick::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    crate::android::request_repaint();
}
