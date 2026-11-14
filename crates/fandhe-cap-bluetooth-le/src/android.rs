//! Android 向け BLE スキャンの橋渡し（Issue #1062）。本 crate の `driver` の Android 経路から、スキャンの開始（権限確認込み）を
//! Java 側 `BluetoothLeHelper.requestBleScan(Activity, long, int)` へ JNI の static 呼び出しで依頼する
//! （`fandhe_platform_android::with_activity_class`。第 1 引数は Activity）。結果を待たずに即座に戻り、広告ごとに
//! `nativeOnBleScanResult`、終了時に 1 回 `nativeOnBleScanFinished`（下の JNI エクスポート）が `crate::OBSERVED` / `crate::bridge`
//! （macOS / Windows と共通）へ pull 型で橋渡しする。完了コードの写像は `crate::error_from_android_code`（SSOT、ホストでテスト済み）に
//! 一元化し、Java 側では分岐しない。周辺機器の名前・アドレスはログへ出さない（security.md A09）。
//!
//! 守る範囲: 要求 id の照合（古い要求の通知は捨てる）、失敗時に部分結果を捨てること（0 台の偽の成功を出さない）。
//! 守らない範囲: 権限ダイアログの回答待ちへの期限、スキャン結果の網羅性。

use fandhe_platform_android::JniCallFailed;

/// BLE の識別子（アドレス）の UTF-16 単位数の上限（MAC は 17。これを超える値は捨てる）。
const BLE_ID_MAX_UNITS: i32 = 64;
/// BLE の広告名の UTF-16 単位数の上限（GAP の Local Name は最大 248 バイト）。
const BLE_NAME_MAX_UNITS: i32 = 248;

/// Java 側 `BluetoothLeHelper.requestBleScan(Activity, long, int)` を呼び、スキャン開始（権限確認込み）を要求する
/// （Issue #1062）。結果を待たず即座に戻る。`duration_ms` は呼び出し元が `SCAN_DURATION_MS`（SSOT）から変換済みで、
/// Java はそのまま期限に使う（期限を複製しない、device-bridge-contracts.md 3 節）。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` なら呼び出し元が `Internal` で完了させる）。`unsafe` は共通層
/// `fandhe_platform_android::with_activity_class` に閉じている。
pub(crate) fn request_ble_scan(request_id: u64, duration_ms: i32) -> bool {
    let Some(app) = fandhe_platform_android::android_app() else {
        return false;
    };
    fandhe_platform_android::with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.BluetoothLeHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            env.call_static_method(
                class,
                jni::jni_str!("requestBleScan"),
                jni::jni_sig!((android.app.Activity, long, int) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Int(duration_ms),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `BluetoothLeHelper.nativeOnBleScanResult`（Java 側）から呼ばれる JNI エクスポート関数（Issue #1062）。
/// 広告 1 件を `OBSERVED` へ足す（要求 id 不一致は `Observer` が捨てる）。`address` が読めない・長すぎる広告は捨てる。
/// `rssi` は Java の生値（取得不能の番兵も 127 のまま）を `rssi_from_apple` で `None` へ写す。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの安全な取り扱い
/// （`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は JNI 命名規約上の固定シンボル名であり、
/// 本 crate は cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_BluetoothLeHelper_nativeOnBleScanResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    address: jni::objects::JString<'caller>,
    name: jni::objects::JString<'caller>,
    rssi: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        if let Some(id) =
            fandhe_platform_android::read_bounded_string(env, &address, BLE_ID_MAX_UNITS)
        {
            let name = fandhe_platform_android::read_bounded_string(env, &name, BLE_NAME_MAX_UNITS);
            crate::OBSERVED.observe(
                request_id as u64,
                crate::contract::BleDevice {
                    id,
                    name,
                    rssi_dbm: crate::rssi_from_apple(rssi),
                },
            );
        }
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

/// `BluetoothLeHelper.nativeOnBleScanFinished`（Java 側）から呼ばれる JNI エクスポート関数（Issue #1062）。
/// `code == 0` は成功、それ以外は `error_from_android_code` で写す。要求 id が古ければ `finish` が捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` の根拠は上と同じ。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_BluetoothLeHelper_nativeOnBleScanFinished(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let result = if code == 0 {
        Ok(())
    } else {
        Err(crate::error_from_android_code(code))
    };
    crate::finish(request_id as u64, result);
    fandhe_runtime::request_repaint();
}
