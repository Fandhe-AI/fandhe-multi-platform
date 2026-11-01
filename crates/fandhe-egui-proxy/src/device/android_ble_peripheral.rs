//! Android 向け BLE 周辺機器役の実装（Issue #1123、親 #164 の b-android。骨格は #1122）。
//! `device/ble_peripheral.rs` の Android 経路から [`AndroidPeripheral`]
//! （`fandhe_device::ble_peripheral::BlePeripheral` 実装）が使われる。契約と守る範囲は
//! `fandhe_device::ble_peripheral` のモジュール doc が SSOT。
//!
//! # 役割分担
//!
//! アドバタイズ・GATT サーバー・許可・`onStop` 中断・Bluetooth の state 監視は Java 側
//! `BlePeripheralHelper`（`HandlerThread` 1 本の直列キュー）が行う。本モジュールは JNI 越しの
//! 橋渡しと、事象の世代照合（`SessionSlot`）、コードの写像（`fandhe_device` の純関数）だけを担う。
//!
//! # token と Activity 再生成
//!
//! 画面の session は画面状態ごとに数え直されるため、Activity 再生成後の新しい画面の session が
//! 旧 Activity の session と衝突しうる。Java とのやりとりと `SessionSlot` の照合には、プロセス単調の
//! [`next_token`] を使う。再生成時は `android_main` が [`reset_pending_tracking`] で通知先を捨てる。
//!
//! # JNI 署名（Java 側と 2 か所で一致させる。native 宣言は Rust の要求を経ないと呼ばれないので no-op スタブは置かない）
//!
//! - `int blePeripheralStart(long, String, String, String)`（0 = 受理）
//! - `int blePeripheralStop()`（0 = 停止・1 = 動いていなかった・2 = 解放失敗）
//! - `BlePeripheralHelper.nativeOnBlePeripheralEvent(long token, int code)`
//! - `BlePeripheralHelper.nativeOnBlePeripheralStartFailure(long token, int errorCode)`
//!
//! 中心役のアドレス・名前・読み取りの中身・OS のエラーコードはログへ出さない（security.md A09）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use fandhe_device::ble_peripheral::BlePeripheral;
#[cfg(target_os = "android")]
use fandhe_device::ble_peripheral::{
    end_reason_from_android_advertise_error, event_from_apple_code, stop_outcome_from_apple,
    OnEvent, PeripheralEvent, StartOutcome, StopOutcome, CHARACTERISTIC_UUID, READ_VALUE,
    SERVICE_UUID,
};
#[cfg(target_os = "android")]
use fandhe_device::DeviceError;

use super::ble_peripheral::SessionSlot;

/// 現在の公開の token と通知先（プロセスに 1 つ）。
static SESSION: SessionSlot = SessionSlot::new();

/// Java とのやりとりに使うプロセス単調の token の次の値（0 は「動いていない」なので 1 から）。
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// token を払い出す。画面の session と違い、Activity 再生成をまたいでも衝突しない。
fn next_token() -> u64 {
    NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
}

/// Activity 再生成時に通知先を捨てる（`android_main` から呼ばれる）。旧 Activity の遅延した通知は
/// token 不一致（slot が空）で捨てられる。
pub(crate) fn reset_pending_tracking() {
    SESSION.clear();
}

/// Android の `BlePeripheral` 実装。
pub(crate) fn implementation() -> Arc<dyn BlePeripheral> {
    #[cfg(target_os = "android")]
    return Arc::new(AndroidPeripheral);
    #[cfg(not(target_os = "android"))]
    return Arc::new(fandhe_device::ble_peripheral::UnsupportedBlePeripheral);
}

/// 状態は Java 側と [`SESSION`] だけにあり、ここには持たない。
#[cfg(target_os = "android")]
struct AndroidPeripheral;

#[cfg(target_os = "android")]
impl AndroidPeripheral {
    /// Activity インスタンスへ `f` を実行する共通処理（唯一の `unsafe` 集約点）。
    /// `android_home_widget` と同形。`android_app()` が `None` なら `Unsupported`、JNI 失敗は `Internal`
    /// （Java 例外は clear し文言は読まない）。
    #[allow(unsafe_code)]
    fn with_activity<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &jni::objects::JObject,
        ) -> Result<T, super::android::JniCallFailed>,
    ) -> Result<T, DeviceError> {
        let Some(app) = crate::android::android_app() else {
            return Err(DeviceError::Unsupported("ble_peripheral"));
        };
        // SAFETY: `AndroidHomeWidget::with_activity` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` を返し、`android_app()` は最新の
        // `AndroidApp` を複製して返すため、`app` は呼び出し時点で有効な `Activity` を指す。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 同上。`activity_as_ptr()` は `AndroidApp` 生存中有効な未所有の
            // グローバル参照で、`Cast` は所有権を持たないため delete を試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let result = f(env, &activity);
            if result.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            result
        })
        .map_err(|_| DeviceError::Internal("ble_peripheral"))
    }

    fn call_start(token: u64) -> Result<i32, DeviceError> {
        Self::with_activity(|env, activity| {
            let service = env.new_string(SERVICE_UUID)?;
            let characteristic = env.new_string(CHARACTERISTIC_UUID)?;
            let value = env.new_string(READ_VALUE)?;
            let ret = env.call_method(
                activity,
                jni::jni_str!("blePeripheralStart"),
                jni::jni_sig!((long, java.lang.String, java.lang.String, java.lang.String) -> int),
                &[
                    jni::objects::JValue::Long(token as jni::sys::jlong),
                    jni::objects::JValue::Object(&service),
                    jni::objects::JValue::Object(&characteristic),
                    jni::objects::JValue::Object(&value),
                ],
            )?;
            Ok(ret.i()?)
        })
    }

    fn call_stop() -> Result<i32, DeviceError> {
        Self::with_activity(|env, activity| {
            let ret = env.call_method(
                activity,
                jni::jni_str!("blePeripheralStop"),
                jni::jni_sig!(() -> int),
                &[],
            )?;
            Ok(ret.i()?)
        })
    }
}

#[cfg(target_os = "android")]
impl BlePeripheral for AndroidPeripheral {
    fn start(&self, _session: u64, on_event: OnEvent) -> Result<StartOutcome, DeviceError> {
        let token = next_token();
        // 先に通知先を置く（Java の最初の事象を落とさない）。
        SESSION.set(token, on_event);
        match Self::call_start(token) {
            Ok(0) => Ok(StartOutcome::Accepted),
            // Activity が無いときは `Unsupported` をそのまま返す。
            Err(e @ DeviceError::Unsupported(_)) => {
                SESSION.clear();
                Err(e)
            }
            Ok(_) | Err(_) => {
                SESSION.clear();
                Err(DeviceError::Internal("ble_peripheral start"))
            }
        }
    }

    fn stop(&self) -> Result<StopOutcome, DeviceError> {
        // Rust 側のロックを離してから Java の直列キューへ同期呼び出しする（契約表 11）。
        SESSION.clear();
        match Self::call_stop() {
            Ok(code) => stop_outcome_from_apple(code),
            Err(DeviceError::Unsupported(_)) => Ok(StopOutcome::NotRunning),
            Err(_) => Err(DeviceError::Internal("ble_peripheral stop")),
        }
    }
}

/// `BlePeripheralHelper.nativeOnBlePeripheralEvent`（Java 側）から呼ばれる JNI エクスポート関数。
/// コードの写像は `fandhe_device::ble_peripheral::event_from_apple_code`。次フレームを起こす
/// （契約表 4 節）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（プリミティブ引数だけで panic しない）。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnLocalPeerFinished` と同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_BlePeripheralHelper_nativeOnBlePeripheralEvent(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    token: jni::sys::jlong,
    code: jni::sys::jint,
) {
    SESSION.deliver(token as u64, event_from_apple_code(code));
    crate::android::request_repaint();
}

/// `BlePeripheralHelper.nativeOnBlePeripheralStartFailure`（Java 側）から呼ばれる JNI エクスポート関数。
/// `AdvertiseCallback.onStartFailure` の errorCode を
/// `fandhe_device::ble_peripheral::end_reason_from_android_advertise_error` で写す。
///
/// # Safety
/// [`Java_com_fandhe_androidapp_BlePeripheralHelper_nativeOnBlePeripheralEvent`] と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_BlePeripheralHelper_nativeOnBlePeripheralStartFailure(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    token: jni::sys::jlong,
    error_code: jni::sys::jint,
) {
    SESSION.deliver(
        token as u64,
        PeripheralEvent::Ended(end_reason_from_android_advertise_error(error_code)),
    );
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::ble_peripheral::PeripheralEvent;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn tokens_are_monotonic() {
        let a = next_token();
        let b = next_token();
        assert!(b > a && a >= 1);
    }

    #[test]
    fn reset_drops_stale_notifications() {
        // 同じ static を使うテストはこの 1 本だけ。
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        let token = next_token();
        SESSION.set(
            token,
            Arc::new(move |_| {
                h.fetch_add(1, Ordering::SeqCst);
            }),
        );
        SESSION.deliver(token, PeripheralEvent::Read);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // 再生成: 通知先を捨てた後は旧 token の通知が届かない。
        reset_pending_tracking();
        SESSION.deliver(token, PeripheralEvent::Read);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // 新しい token は旧 token と衝突せず、旧 token の通知は新しい通知先へ届かない。
        let h = Arc::clone(&hits);
        let fresh = next_token();
        assert_ne!(fresh, token);
        SESSION.set(
            fresh,
            Arc::new(move |_| {
                h.fetch_add(10, Ordering::SeqCst);
            }),
        );
        SESSION.deliver(token, PeripheralEvent::Read);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        SESSION.deliver(fresh, PeripheralEvent::Read);
        assert_eq!(hits.load(Ordering::SeqCst), 11);
        reset_pending_tracking();
    }
}
