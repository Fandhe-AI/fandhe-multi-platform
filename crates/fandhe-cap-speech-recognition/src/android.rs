//! Android 向け音声認識（Issue #1026）。`crate::driver` の Android 経路から呼ばれ、提示は
//! `SpeechRecognitionHelper.requestStart(Activity, long, int)` / `requestStop(Activity, long)`（static、即時 return）、結果は
//! `nativeOnSpeechStarted`・`nativeOnSpeechResult`・`nativeOnSpeechError` で非同期に届く pull 型 bridge（`crate::driver::bridge`）。
//! `onError` の生値は Java 側で分岐せず `nativeOnSpeechError` でそのまま渡し、写像は `crate::driver::outcome_from_android_error`
//! （SSOT、host でテスト済み）に一元化する（device-bridge-contracts.md「FFI 境界」節）。`SpeechRecognitionHelper` が無い
//! （生成前・破棄後）要求は Java 側が内部エラー（code 2）で確定し、結果が来ないまま待たせない。
//!
//! `unsafe` は JNI 受け口の `#[unsafe(no_mangle)]` だけで、JNI 呼び出しは共通層 `fandhe_platform_android::with_activity_class` に
//! 閉じている。no-op スタブは置かない（どの native も Rust の `requestStart` を経た後でしか呼ばれない）。認識結果の文字列・例外メッセージは
//! ログに出さない（security.md A09）。

use fandhe_platform_android::{JniCallFailed, with_activity_class};

use crate::error::SpeechRecognitionError;

/// Java 側 `SpeechRecognitionHelper.requestStart(Activity, long, int)` を呼び、聞き取り開始（権限確認込み）を要求する（Issue #1026）。
/// 結果を待たずに即座に戻る（実結果は `nativeOnSpeechStarted`/`nativeOnSpeechResult`/`nativeOnSpeechError` が非同期に届ける、
/// `crate::driver::bridge` 参照）。`max_listen_ms` は呼び出し元（`crate::driver` の `present`、Android 分岐）が `MAX_LISTEN_MS`
/// （30 秒固定）から変換済み。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元が `false` を見て
/// `Err(SpeechRecognitionError::Internal("speech_recognition present"))` として `bridge::deliver` する）。
pub(crate) fn request_speech_start(request_id: u64, max_listen_ms: i32) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.SpeechRecognitionHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let r = env.call_static_method(
                class,
                jni::jni_str!("requestStart"),
                jni::jni_sig!((android.app.Activity, long, int) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Int(max_listen_ms),
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

/// Java 側 `SpeechRecognitionHelper.requestStop(Activity, long)` を呼び、聞き取り停止を要求する（Issue #1026）。
/// `request_speech_start` と同じ形。`request_id` が Java 側の追跡と不一致なら Java 側が no-op として無視する契約。
pub(crate) fn request_speech_stop(request_id: u64) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.SpeechRecognitionHelper"),
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

/// Java 側から届いた聞き取り結果を `SpeechOutcome` へ変換する（`nativeOnSpeechResult`
/// 本体）。`code == 0`（成功）以外は `text` を参照せず
/// `crate::driver::error_from_code`（Java 側の独自コード体系、
/// `RecognitionListener.onError` の生値とは別の SSOT）へ委譲する（`decode_mic_record_result`
/// と同じ形）。
fn decode_speech_result(
    env: &mut jni::Env,
    code: jni::sys::jint,
    text: &jni::objects::JString,
) -> Result<crate::contract::SpeechOutcome, SpeechRecognitionError> {
    if code != 0 {
        return crate::driver::error_from_code(code);
    }
    if text.is_null() {
        // 契約違反（成功なのに null）を検出した経路。
        return Err(SpeechRecognitionError::Internal(
            "speech_recognition: null result on success",
        ));
    }
    // 全文を Rust の `String` へ複製する前に長さで弾く（PR #1030 codex 指摘）。
    // `String.length()` は UTF-16 単位数で、UTF-8 では 1 単位あたり 1 バイト以上に
    // なるため、単位数が上限を超えていれば UTF-8 でも必ず超える。上限以内なら複製は
    // 高々 3 倍で収まり、厳密なバイト数の判定は `outcome_from_transcript` が行う。
    let units = env
        .call_method(text, jni::jni_str!("length"), jni::jni_sig!(() -> int), &[])
        .and_then(|v| v.i())
        .map_err(|_| SpeechRecognitionError::Internal("speech_recognition"))?;
    if usize::try_from(units).unwrap_or(usize::MAX) > crate::contract::MAX_TRANSCRIPT_BYTES {
        return Err(SpeechRecognitionError::InvalidInput(
            "speech_recognition size",
        ));
    }
    let text_str = text
        .try_to_string(env)
        .map_err(|_| SpeechRecognitionError::Internal("speech_recognition"))?;
    crate::contract::outcome_from_transcript(text_str)
}

/// `SpeechRecognitionHelper.nativeOnSpeechStarted`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1026）。シンボル名の固定規約は `nativeOnMicRecordStarted` と同じ。
/// `SpeechRecognizer.startListening()` に対応する `onReadyForSpeech` が届いた瞬間に
/// Java 側から呼ばれ、[`crate::driver::bridge::mark_started`] へ橋渡しする
/// （`crate::driver::drive_callback` が経過時間の計測を開始する合図、
/// モジュール doc 参照）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnMicRecordStarted` と同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_SpeechRecognitionHelper_nativeOnSpeechStarted(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
) {
    crate::driver::bridge::mark_started(request_id as u64);
    fandhe_runtime::request_repaint();
}

/// `SpeechRecognitionHelper.nativeOnSpeechResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1026）。シンボル名の固定規約は `nativeOnMicRecordResult` と同じ。
///
/// [`decode_speech_result`] で `SpeechOutcome`/`SpeechRecognitionError` へ変換し、
/// [`crate::driver::bridge::deliver`] へ届ける。`request_id` が現在追跡中の
/// 要求と不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる
/// （`bridge` モジュール doc 参照）。認識結果の文字列はログへ出力しない
/// （security.md A09）。
///
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_callback` が次フレームで `bridge::snapshot` を観測できるようにする
/// （`nativeOnMicRecordResult` と同じ理由）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[unsafe(no_mangle)]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnMicRecordResult` と
/// 同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_SpeechRecognitionHelper_nativeOnSpeechResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    text: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_speech_result(env, code, &text);
        crate::driver::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    fandhe_runtime::request_repaint();
}

/// `SpeechRecognitionHelper.nativeOnSpeechError`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1026）。`RecognitionListener.onError(int)` の生値をそのまま受け取り、
/// `crate::driver::outcome_from_android_error`（SSOT、host でテスト済み）
/// で `SpeechOutcome`/`SpeechRecognitionError` へ変換してから
/// [`crate::driver::bridge::deliver`] へ届ける。`JString` を扱わないため
/// `nativeOnBiometricResult` と同じ生ポインタ形式で十分（`EnvUnowned` 不要）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnBiometricResult` と同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_SpeechRecognitionHelper_nativeOnSpeechError(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    error: jni::sys::jint,
) {
    let outcome = crate::driver::outcome_from_android_error(error);
    crate::driver::bridge::deliver(request_id as u64, outcome);
    fandhe_runtime::request_repaint();
}
