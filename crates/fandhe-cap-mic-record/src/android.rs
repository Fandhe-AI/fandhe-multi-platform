//! Android 向けマイク録音（Issue #957）。`crate::driver` の Android 経路から呼ばれ、提示は `MicRecordHelper.requestStart(Activity, long, int)` /
//! `requestStop(Activity, long)`（static、即時 return）、結果は `nativeOnMicRecordStarted`・`nativeOnMicRecordResult` で非同期に届く
//! pull 型 bridge（`crate::driver::bridge`）。コードの写像は `crate::driver::error_from_code` に一本化する（Java 側 `MIC_RESULT_*`
//! が SSOT）。`MicRecordHelper` が無い（生成前・破棄後）要求は Java 側が内部エラー（code 2）で確定し、結果が来ないまま待たせない。
//!
//! `unsafe` は JNI 受け口の `#[unsafe(no_mangle)]` だけで、JNI 呼び出しは共通層 `fandhe_platform_android::with_activity_class` に
//! 閉じている。no-op スタブは置かない（どの native も Rust の `requestStart` を経た後でしか呼ばれない）。録音データ・例外メッセージは
//! ログに出さない（security.md A09）。

use fandhe_platform_android::{JniCallFailed, with_activity_class};

use crate::error::MicRecordError;

/// Java 側 `MicRecordHelper.requestStart(Activity, long, int)` を呼び、録音開始（権限確認込み）を要求する（Issue #957）。
/// 結果を待たずに即座に戻る（実結果は `nativeOnMicRecordStarted`/`nativeOnMicRecordResult` が非同期に届ける、
/// `crate::driver::bridge` 参照）。`max_duration_ms` は呼び出し元（`crate::driver` の `present`、Android 分岐）が `MAX_RECORD_MS`
/// （30 秒固定）から変換済み。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元が `false` を見て
/// `Err(MicRecordError::Internal("mic_record present"))` として `bridge::deliver` する）。
pub(crate) fn request_mic_record_start(request_id: u64, max_duration_ms: i32) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.MicRecordHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let r = env.call_static_method(
                class,
                jni::jni_str!("requestStart"),
                jni::jni_sig!((android.app.Activity, long, int) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Int(max_duration_ms),
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

/// Java 側 `MicRecordHelper.requestStop(Activity, long)` を呼び、録音停止を要求する（Issue #957）。`request_mic_record_start` と同じ形。
/// `request_id` が Java 側の追跡と不一致（権限確認中の別要求・旧要求）なら Java 側が no-op として無視する契約
/// （`MicRecordHelper.requestStop` 参照）。
pub(crate) fn request_mic_record_stop(request_id: u64) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.MicRecordHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let r = env.call_static_method(
                class,
                jni::jni_str!("requestStop"),
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

/// Java 側 `MicRecordHelper.abortPending()`（static）を呼び、保留中・実行中の要求を結果を届けずに捨てさせる。`reset_pending_tracking`（Rust 側の
/// 待ち手の破棄）と対で、Java 側だけが録音・聞き取りを続けて画面に反映されないことを防ぐ。`AndroidApp` が未設定・クラス未ロード・
/// JNI 失敗でも panic せず黙って戻る（呼び出し元は `android_main`。破棄は best-effort）。
pub(crate) fn abort_pending() {
    let Some(app) = fandhe_platform_android::android_app() else {
        return;
    };
    let _ = with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.MicRecordHelper"),
        |env, _activity, class| -> Result<(), JniCallFailed> {
            let r = env.call_static_method(
                class,
                jni::jni_str!("abortPending"),
                jni::jni_sig!(() -> ()),
                &[],
            );
            if r.is_err() && env.exception_check() {
                env.exception_clear();
            }
            r?;
            Ok(())
        },
    );
}

/// Java 側から届いた録音結果を `Recording` へ変換する（`nativeOnMicRecordResult` 本体）。
/// `code` の意味は `crate::driver::error_from_code`（Java 側 `MIC_RESULT_*`
/// 定数が SSOT、`MicRecordHelper.java` のコメント参照）を参照。`code == 0`（成功）以外は
/// `bytes`/`mime` を参照しない（Java 側は null を渡す契約、`MicRecordHelper` 参照）。
fn decode_mic_record_result(
    env: &jni::Env,
    code: jni::sys::jint,
    bytes: &jni::objects::JByteArray,
    mime: &jni::objects::JString,
    duration_ms: jni::sys::jlong,
) -> Result<crate::contract::Recording, MicRecordError> {
    if code != 0 {
        return Err(crate::driver::error_from_code(code));
    }
    if bytes.is_null() || mime.is_null() {
        // 契約違反（成功なのに null）を検出した経路。
        return Err(MicRecordError::Internal(
            "mic_record: null result on success",
        ));
    }
    // 変換前（`convert_byte_array` が Vec を確保する前）に配列長を検査する
    // （security.md A05、メモリ枯渇防止。`decode_capture_result` の `MAX_IMAGE_BYTES`
    // 検査と同型。`build_recording` 側の検査は防御の二重化として残す）。
    let len = bytes
        .len(env)
        .map_err(|_| MicRecordError::Internal("mic_record"))?;
    if len > crate::contract::MAX_RECORDING_BYTES {
        return Err(MicRecordError::Internal("mic_record size"));
    }
    let owned_bytes = env
        .convert_byte_array(bytes)
        .map_err(|_| MicRecordError::Internal("mic_record"))?;
    let mime_str = mime
        .try_to_string(env)
        .map_err(|_| MicRecordError::Internal("mic_record"))?;
    crate::contract::build_recording(owned_bytes, mime_str, duration_ms.max(0) as u64)
}

/// `MicRecordHelper.nativeOnMicRecordStarted`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #957）。シンボル名の固定規約は `nativeOnBiometricResult` と同じ。
/// `MediaRecorder.start()` が成功した瞬間に Java 側から呼ばれ、
/// [`crate::driver::bridge::mark_started`] へ橋渡しする（`crate::driver::
/// drive_callback` が経過時間の計測を開始する合図、モジュール doc 参照）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnBiometricResult` と同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_MicRecordHelper_nativeOnMicRecordStarted(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
) {
    crate::driver::bridge::mark_started(request_id as u64);
    fandhe_runtime::request_repaint();
}

/// `MicRecordHelper.nativeOnMicRecordResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #957）。シンボル名の固定規約は `nativeOnFilePickerResult` と同じ。
///
/// [`decode_mic_record_result`] で `Recording`/`MicRecordError` へ変換し、
/// [`crate::driver::bridge::deliver`] へ届ける。`request_id` が現在追跡中の要求と
/// 不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる
/// （`bridge` モジュール doc 参照）。録音データ・例外メッセージはログへ出力しない
/// （security.md A09）。
///
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_callback` が次フレームで `bridge::snapshot` を観測できるようにする
/// （`nativeOnFilePickerResult` と同じ理由）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[unsafe(no_mangle)]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnFilePickerResult` と
/// 同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_MicRecordHelper_nativeOnMicRecordResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    bytes: jni::objects::JByteArray<'caller>,
    mime: jni::objects::JString<'caller>,
    duration_ms: jni::sys::jlong,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_mic_record_result(env, code, &bytes, &mime, duration_ms);
        crate::driver::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    fandhe_runtime::request_repaint();
}

// `decode_mic_record_result`（code 表の写像）は `android.rs` 自体が Android
// ターゲットのみでコンパイルされる（`lib.rs` の `#[cfg(target_os = "android")]`
// 参照）ため host `cargo test` では検証できない。写像表の SSOT
// （`crate::driver::error_from_code`）は host で常時コンパイル・テストされる
// （`mic_record.rs::tests::error_from_code_maps_table` 参照）ため、二重にテストを
// 置かない。
