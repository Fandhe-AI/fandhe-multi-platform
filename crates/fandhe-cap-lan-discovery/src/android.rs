//! Android 向け LAN のサービス探索（Issue #489。親 #109 の b-android）。
//!
//! 呼び出し元: `driver.rs` の Android 経路が `present` を呼ぶ。`present` は
//! `LanDiscoveryHelper.request(Activity, long, int)` を JNI で呼ぶだけで即座に戻る。探索そのもの
//! （`NsdManager.discoverServices` / `resolveService`、窓の管理、onPause / onDestroy での中断）は
//! Java 側 `LanDiscoveryHelper` が main Handler 1 本で行い、結果を本ファイルの JNI エクスポートで
//! `OBSERVED`（サービス 1 件ごと）と `finish`（窓が閉じたとき 1 回）へ届ける。
//!
//! 窓の長さは `BROWSE_DURATION_MS`（SSOT）を `present` が Java へ渡し、Java が JNI 受付時点から
//! 単調時計で測る（Rust 側に別の期限は持たない）。完了コードの写像は
//! `crate::contract::decode_code` に一元化し、Java は値を合わせるだけで分岐しない。
//! Rust 側の要求を経ない native 呼び出しは起きないため、no-op スタブは置かない。
//! サービス名・アドレスはログへ出さない（security.md A09）。

use std::time::Duration;

use crate::contract::{decode_code, service_from_raw};
use crate::error::LanDiscoveryError;

/// サービス名の UTF-16 単位数の上限（`MAX_NAME_BYTES` と同じ。超過は捨てる）。
const NAME_MAX_UNITS: i32 = 255;
/// アドレス文字列の UTF-16 単位数の上限（IPv6 + スコープ名でも十分な長さ）。
const ADDR_MAX_UNITS: i32 = 64;

/// 探索を依頼する。完了は `OBSERVED` / `finish` へ届く（契約は `contract.rs`）。
/// `LanDiscoveryHelper.request(Activity, long, int)` を JNI の static 呼び出しで呼ぶだけで即座に戻る。
/// JavaVM と Activity の取得は `with_activity_class` に任せ、ここは `unsafe` を持たない。
/// JNI 呼び出しに失敗したら `Err(Internal)`（dispatch が同期失敗として確定する）。
pub(crate) fn present(request_id: u64, duration: Duration) -> Result<(), LanDiscoveryError> {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    use jni::objects::JValue;
    let duration_ms = i32::try_from(duration.as_millis())
        .map_err(|_| LanDiscoveryError::Internal("lan_discovery duration"))?;
    let app = android_app().ok_or(LanDiscoveryError::Internal("lan_discovery app unavailable"))?;
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.LanDiscoveryHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            env.call_static_method(
                class,
                jni::jni_str!("request"),
                jni::jni_sig!((android.app.Activity, long, int) -> ()),
                &[
                    JValue::Object(activity),
                    JValue::Long(request_id as jni::sys::jlong),
                    JValue::Int(duration_ms),
                ],
            )?;
            Ok(())
        },
    )
    .map_err(|_| LanDiscoveryError::Internal("lan_discovery request"))
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_LanDiscoveryHelper_nativeOnLanService<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    name: jni::objects::JString<'caller>,
    addr: jni::objects::JString<'caller>,
    port: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let name = fandhe_platform_android::read_bounded_string(env, &name, NAME_MAX_UNITS);
        let addr = fandhe_platform_android::read_bounded_string(env, &addr, ADDR_MAX_UNITS);
        if let Some(service) = service_from_raw(name, addr.as_deref(), port) {
            crate::driver::OBSERVED.observe(request_id as u64, service);
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_LanDiscoveryHelper_nativeOnLanFinished(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    crate::driver::finish(request_id as u64, decode_code(code));
    fandhe_runtime::request_repaint();
}
