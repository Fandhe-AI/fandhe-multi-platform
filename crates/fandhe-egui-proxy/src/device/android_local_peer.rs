//! Android 向け近くの端末との直接通信（Issue #513。親 #110 の b-android）。
//!
//! 呼び出し元: `device/local_peer.rs` の Android 経路が `present` を呼ぶ。`present` は
//! `MainActivity.requestLocalPeer(long, String, String, int)` を JNI で呼ぶだけで即座に戻る。
//! 通信そのもの（Wi-Fi Aware の attach / publish / subscribe / メッセージ送受信、窓の管理、許可、
//! onPause / onDestroy での中断）は Java 側 `LocalPeerHelper` が main Handler 1 本で行い、結果を
//! 本ファイルの JNI エクスポートで `PENDING.offer`（相手の名前と受信データ）と `finish`（窓が
//! 閉じたとき 1 回）へ届ける。データパスと GMS 依存の Nearby Connections は使わない。
//!
//! 窓の長さは `EXCHANGE_WINDOW_MS`（SSOT）を `present` が Java へ渡し、Java が JNI 受付時点から
//! 単調時計で測る（Rust 側に別の期限は持たない）。完了コードの写像は
//! `fandhe_device::local_peer::decode_code` に一元化し、Java は値を合わせるだけで分岐しない。
//! 受信データの検証は `PENDING.offer`（名前の形式・UTF-8・長さ・制御文字）に一本化し、Java では
//! 二重に検証しない。Rust 側の要求を経ない native 呼び出しは起きないため、no-op スタブは置かない。
//! 表示名・受信文字列はログへ出さない（security.md A09）。

use std::time::Duration;

use fandhe_device::local_peer::{decode_code, MAX_TEXT_BYTES};
use fandhe_device::DeviceError;

/// 表示名の UTF-16 単位数（`fandhe-` + 小文字 hex 8 桁 = 15）。超過は捨てる。
const NAME_UNITS: i32 = 15;

/// 交換を依頼する。完了は `PENDING` / `finish` へ届く（契約は `device/local_peer.rs`）。
/// JNI 呼び出しに失敗したら `Err(Internal)`（dispatch が同期失敗として確定する）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `android_lan_discovery.rs`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn present(
    request_id: u64,
    own_name: &str,
    outgoing: &str,
    window: Duration,
) -> Result<(), DeviceError> {
    let window_ms = i32::try_from(window.as_millis())
        .map_err(|_| DeviceError::Internal("local_peer window"))?;
    let app =
        crate::android::android_app().ok_or(DeviceError::Internal("local_peer app unavailable"))?;

    // SAFETY: `request_biometric` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), jni::errors::Error> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: `request_biometric` と同一の根拠。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        let name = env.new_string(own_name)?;
        let text = env.new_string(outgoing)?;
        env.call_method(
            &activity,
            jni::jni_str!("requestLocalPeer"),
            jni::jni_sig!((long, java.lang.String, java.lang.String, int) -> ()),
            &[
                jni::objects::JValue::Long(request_id as jni::sys::jlong),
                jni::objects::JValue::Object(&name),
                jni::objects::JValue::Object(&text),
                jni::objects::JValue::Int(window_ms),
            ],
        )?;
        Ok(())
    })
    .map_err(|_| DeviceError::Internal("local_peer request"))
}

/// `LocalPeerHelper.nativeOnLocalPeerMessage`（Java 側）から呼ばれる JNI エクスポート関数。
/// 受け取った相手の名前と本文を `PENDING.offer` へ渡し、受理したら `true`（要求 id 不一致・
/// 検証不合格・2 件目以降は `false`）。本文は長さを検査してから読む（メモリ枯渇を防ぐ）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnLanService` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_LocalPeerHelper_nativeOnLocalPeerMessage<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    name: jni::objects::JString<'caller>,
    text: jni::objects::JByteArray<'caller>,
) -> jni::sys::jboolean {
    let outcome = unowned_env.with_env(|env| -> Result<bool, jni::errors::Error> {
        let Some(name) = super::android::read_ble_string(env, &name, NAME_UNITS) else {
            return Ok(false);
        };
        if text.is_null() || text.len(env)? > MAX_TEXT_BYTES {
            return Ok(false);
        }
        let bytes = env.convert_byte_array(&text)?;
        Ok(super::local_peer::PENDING.offer(request_id as u64, &name, &bytes))
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    matches!(outcome.into_outcome(), jni::Outcome::Ok(true))
}

/// `LocalPeerHelper.nativeOnLocalPeerFinished`（Java 側）から呼ばれる JNI エクスポート関数。
/// 窓が閉じたとき・中断・失敗で 1 回だけ呼ばれ、コードは `decode_code` で写す。
/// 要求 id が古ければ `finish` が捨てる。次フレームを起こして画面を更新する（契約表 4 節）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnLanFinished` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_LocalPeerHelper_nativeOnLocalPeerFinished(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::local_peer::finish(request_id as u64, decode_code(code));
    crate::android::request_repaint();
}
