//! Android 向け LAN のサービス探索（Issue #489。親 #109 の b-android）。
//!
//! 呼び出し元: `device/lan_discovery.rs` の Android 経路が `present` を呼ぶ。`present` は
//! `MainActivity.requestLanDiscovery(long, int)` を JNI で呼ぶだけで即座に戻る。探索そのもの
//! （`NsdManager.discoverServices` / `resolveService`、窓の管理、onPause / onDestroy での中断）は
//! Java 側 `LanDiscoveryHelper` が main Handler 1 本で行い、結果を本ファイルの JNI エクスポートで
//! `OBSERVED`（サービス 1 件ごと）と `finish`（窓が閉じたとき 1 回）へ届ける。
//!
//! 窓の長さは `BROWSE_DURATION_MS`（SSOT）を `present` が Java へ渡し、Java が JNI 受付時点から
//! 単調時計で測る（Rust 側に別の期限は持たない）。完了コードの写像は
//! `fandhe_device::lan_discovery::decode_code` に一元化し、Java は値を合わせるだけで分岐しない。
//! Rust 側の要求を経ない native 呼び出しは起きないため、no-op スタブは置かない。
//! サービス名・アドレスはログへ出さない（security.md A09）。

use std::time::Duration;

use fandhe_device::lan_discovery::{decode_code, service_from_raw};
use fandhe_device::DeviceError;

/// サービス名の UTF-16 単位数の上限（`MAX_NAME_BYTES` と同じ。超過は捨てる）。
const NAME_MAX_UNITS: i32 = 255;
/// アドレス文字列の UTF-16 単位数の上限（IPv6 + スコープ名でも十分な長さ）。
const ADDR_MAX_UNITS: i32 = 64;

/// 探索を依頼する。完了は `OBSERVED` / `finish` へ届く（契約は `device/lan_discovery.rs`）。
/// JNI 呼び出しに失敗したら `Err(Internal)`（dispatch が同期失敗として確定する）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_ble_scan`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, duration: Duration) -> Result<(), DeviceError> {
    let duration_ms = i32::try_from(duration.as_millis())
        .map_err(|_| DeviceError::Internal("lan_discovery duration"))?;
    let app = crate::android::android_app()
        .ok_or(DeviceError::Internal("lan_discovery app unavailable"))?;

    // SAFETY: `request_biometric` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), jni::errors::Error> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: `request_biometric` と同一の根拠。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestLanDiscovery"),
            jni::jni_sig!((long, int) -> ()),
            &[
                jni::objects::JValue::Long(request_id as jni::sys::jlong),
                jni::objects::JValue::Int(duration_ms),
            ],
        )?;
        Ok(())
    })
    .map_err(|_| DeviceError::Internal("lan_discovery request"))
}

/// `LanDiscoveryHelper.nativeOnLanService`（Java 側）から呼ばれる JNI エクスポート関数。
/// 発見・解決ごとに 1 件を `OBSERVED` へ足す（要求 id 不一致は `Observer` が捨てる）。
/// `addr` / `port` は解決前なら null / 0 で、契約の形への整形は `service_from_raw` が担う。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnBleScanResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_LanDiscoveryHelper_nativeOnLanService<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    name: jni::objects::JString<'caller>,
    addr: jni::objects::JString<'caller>,
    port: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let name = super::android::read_ble_string(env, &name, NAME_MAX_UNITS);
        let addr = super::android::read_ble_string(env, &addr, ADDR_MAX_UNITS);
        if let Some(service) = service_from_raw(name, addr.as_deref(), port) {
            super::lan_discovery::OBSERVED.observe(request_id as u64, service);
        }
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

/// `LanDiscoveryHelper.nativeOnLanFinished`（Java 側）から呼ばれる JNI エクスポート関数。
/// 窓が閉じたとき・中断・失敗で 1 回だけ呼ばれ、コードは `decode_code` で写す。
/// 要求 id が古ければ `finish` が捨てる。次フレームを起こして画面を更新する（契約表 4 節）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnBleScanFinished` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_LanDiscoveryHelper_nativeOnLanFinished(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::lan_discovery::finish(request_id as u64, decode_code(code));
    crate::android::request_repaint();
}
