//! Android 向けファイル選択（Issue #874、Phase 11 型サンプル「ファイル選択」の Android 経路、案 B）。`crate::driver` の Android 経路から呼ばれ、
//! 提示は `FilePickerHelper.request(Activity, long)`（static）、結果は `nativeOnFilePickerResult` で 1 度だけ届く pull 型 bridge。
//!
//! iOS/Android は `FilePicker` trait を通らない（`crate::contract` の doc 参照）。Java 側の責務は「提示 → requestId 付きで 1 回だけ通知 →
//! 後始末」の 3 つだけで、進行中・中断済みを表す static フラグは持たない。8 MiB 判定・読み取りは
//! `fandhe_device::file_picker::picked_from_document`（`read_picked_from` に委譲）に一本化し、Java 側で読み取りループを書かない。
//! fd はパス文字列（`/proc/self/fd/N`）ではなく `ParcelFileDescriptor.detachFd()` の生 fd で受け取る（pipe 型プロバイダで
//! `/proc/self/fd/N` の再 open が権限再確認に失敗するのを避けるため）。
//!
//! `unsafe` は fd の所有権を取る `OwnedFd::from_raw_fd` の 1 行と JNI 受け口の `#[unsafe(no_mangle)]` だけで、JNI 呼び出しは共通層
//! `fandhe_platform_android::with_activity_class` に閉じている。no-op スタブは置かない（どの native も Rust の `request` を経た後でしか
//! 呼ばれない）。選んだファイルの内容・名前・Uri はログに出さない（security.md A09）。

use std::os::fd::{FromRawFd, OwnedFd};

use fandhe_device::file_picker::picked_from_document;
use fandhe_platform_android::{JniCallFailed, with_activity_class};

use crate::contract::PickedFile;
use crate::error::FilePickerError;

/// Java 側 `FilePickerHelper` が「追跡中の要求がある間の新しい要求」に返す結果コード（busy）。
const BUSY_CODE: i32 = 3;

/// Java 側 `FilePickerHelper.request(Activity, long)` を呼び、ダイアログの提示だけを要求する。結果を待たずに即座に戻る点は
/// カメラ撮影（`fandhe-cap-camera-capture`）と異なり、結果の受け皿は `crate::driver` の `bridge::begin`/`bridge::deliver` が持つ。
/// 渡す `request_id` は画面状態の generation ではなく `bridge::begin` が発行するプロセス内一意の連番。
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元が
/// `Err(FilePickerError::Internal("file_picker present"))` を `deliver` へ渡して畳む）。Helper が無い（生成前・破棄後）要求は
/// Java 側が `nativeOnFilePickerResult` で失敗（code 2）を返して確定する。
pub(crate) fn request_file_pick(request_id: u64) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.FilePickerHelper"),
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

/// Java 側から届いた `code`/`name`/`fd` を `PickedFile` へ変換する（`nativeOnFilePickerResult` 本体）。`code` の意味は Java 側
/// `FilePickerHelper` の宣言コメントと対で決める: 0=選択（name/fd 必須）／1=キャンセル／2=失敗（Java 側で名前解決不能・fd 取得失敗・
/// `launch()` 例外等を一律ここへ畳んでいる。詳細分類は行わない）。
///
/// `fd` の所有権を受け取り、この関数内で読み切って close する。名前の検証より **先に** fd の所有権を取る（後続のどの失敗経路でも
/// `file` の drop で必ず 1 回 close され、detach 済み fd がリークしない。codex P1）。検証と読み取りは
/// `picked_from_document`（`fandhe-device`）が担う。
#[allow(unsafe_code)]
fn decode_file_pick_result(
    env: &jni::Env,
    code: jni::sys::jint,
    name: &jni::objects::JString,
    fd: jni::sys::jint,
) -> Result<Option<PickedFile>, FilePickerError> {
    if code == BUSY_CODE {
        return Err(FilePickerError::Internal("file_picker busy"));
    }
    let file = if code == 0 && fd >= 0 {
        // SAFETY: Java 側 `FilePickerHelper.deliverPickedDocument` が `ParcelFileDescriptor.detachFd()` で所有権を手放した直後に
        // 渡す有効な fd で（`code == 0` のときだけ有効）、detach 後は Java 側で close しない契約（detach 後の
        // `ParcelFileDescriptor.close()` は no-op になる Android の仕様でもあり、万一 Java 側の契約が崩れても二重 close にはならない）。
        // `OwnedFd` に包むことで、この関数を抜ける際（成功なら `picked_from_document` が読み切った直後、失敗なら早期 return 時の
        // `File` の drop）に確実に 1 回だけ close される。呼び出し元は他のどこにもこの fd を渡さないため、二重 close・
        // use-after-close は発生しない。
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
    picked_from_document(code, file, raw_name.as_deref()).map_err(FilePickerError::from)
}

/// `FilePickerHelper.nativeOnFilePickerResult`（Java 側）から呼ばれる JNI エクスポート関数（Issue #874、案 B）。
///
/// [`decode_file_pick_result`] で `PickedFile` へ変換し、`crate::driver::bridge::deliver` へ届ける。`request_id` が現在追跡中の
/// 要求と不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる。受け取った内容はログ出力・保存を一切行わない
/// （選択されたファイルは他アプリの機密文書になり得る）。
///
/// pull 型（改訂 4）: `bridge` は `CompletionSender` を持たないため、結果を置くだけでは次フレームの描画が保証されない
/// （アイドル中は `update` 自体が呼ばれない）。`deliver` の成否に関わらず無条件で `fandhe_runtime::request_repaint` を呼び、
/// `drive_mobile_with` が次フレームで `bridge::snapshot` を観測できるようにする（`deliver` が `false`（不一致・重複）を返した場合でも
/// repaint 自体は無害なので条件分岐しない）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`fd` の `unsafe` な受け取りは [`decode_file_pick_result`] に閉じる）。
/// `EnvUnowned::with_env` が JNI アタッチメントの安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。
/// `#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_FilePickerHelper_nativeOnFilePickerResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_file_pick_result(env, code, &name, fd);
        crate::driver::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    // pull 型（改訂 4）: ここで明示的に再描画を要求しないと `drive_mobile` が次フレームで結果を観測できない（doc 参照）。
    fandhe_runtime::request_repaint();
}
