//! Android 向け「無線の状態と切り替え」実装（Issue #974、親 #189）。`device/radio_control.rs` の
//! Android 経路から [`implementation`] が呼ばれ、Java 側 `RadioControlHelper` を JNI で橋渡しする。
//! 契約の SSOT は `fandhe_device::radio_control`。
//!
//! 流れ:
//! - 読み取り: `MainActivity.radioControlRead(int) -> int` を同期で呼び、戻り値を [`decode_read`] で写す
//! - 切り替え: `MainActivity.requestRadioControlSet(long, int, boolean)` で依頼し、Java が
//!   `nativeOnRadioControlResult(id, code)` で返した値を [`decode_switch`] で写す。ワーカースレッドは
//!   [`set_with`] で結果を待つ（trait の契約どおり期限は持たない）
//!
//! 結果コードの SSOT は Java 側 `RadioControlHelper` の `READ_*` / `SET_*` / `KIND_*`。
//!
//! OS への写像: Wi-Fi の切り替えと Bluetooth のオフは設定画面を開いて `Opened`、Bluetooth のオンは
//! `ACTION_REQUEST_ENABLE` で `RESULT_OK` が `Applied`、`RESULT_CANCELED` が `NotChanged`（失敗にしない）。
//! Android に `Disabled` に相当する OS 値はなく、返さない。未知のコード・JNI の失敗は `Internal`
//! （`Off` や成功に畳まない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・失敗を成功に畳まないこと・Activity 再生成時の保留中の要求の破棄
//! （Java の `onDestroy` が INTERRUPTED で確定し、Rust の [`reset_pending_tracking`] が待機中のワーカーを
//! 失敗で戻す）。守らない範囲: 設定画面での利用者の操作、読み直し時点の切り替わり途中、ダイアログや
//! 設定画面の待ち時間。id・コード・OS の例外はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestRadioControlSet` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（wallpaper と同じ）。

use fandhe_device::radio_control::{RadioKind, RadioState, SwitchOutcome, FAILED, FEATURE};
use fandhe_device::DeviceError;

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 種類コード（Java 側 `RadioControlHelper.KIND_*` と同値）。
const KIND_WIFI: i32 = 0;
const KIND_BLUETOOTH: i32 = 1;

/// 読み取りの戻り値（SSOT は Java 側 `RadioControlHelper.READ_*`）。
const READ_ON: i32 = 0;
const READ_OFF: i32 = 1;
const READ_TRANSITIONING: i32 = 2;
const READ_UNKNOWN: i32 = 3;
const READ_NOT_PRESENT: i32 = 4;
const READ_PERMISSION_DENIED: i32 = 5;

/// 切り替えの結果コード（SSOT は Java 側 `RadioControlHelper.SET_*`）。
const SET_APPLIED: i32 = 0;
const SET_OPENED: i32 = 1;
const SET_NOT_CHANGED: i32 = 2;
const SET_UNSUPPORTED: i32 = 3;
const SET_PERMISSION_DENIED: i32 = 4;

/// 無線の種類を Java へ渡すコードへ写す。
fn kind_code(kind: RadioKind) -> i32 {
    match kind {
        RadioKind::Wifi => KIND_WIFI,
        RadioKind::Bluetooth => KIND_BLUETOOTH,
    }
}

/// 読み取りの戻り値を契約の状態へ写す。`READ_FAILED` と未知の値は `Internal`（`Off` に畳まない）。
fn decode_read(code: i32) -> Result<RadioState, DeviceError> {
    match code {
        READ_ON => Ok(RadioState::On),
        READ_OFF => Ok(RadioState::Off),
        READ_TRANSITIONING => Ok(RadioState::Transitioning),
        READ_UNKNOWN => Ok(RadioState::Unknown),
        READ_NOT_PRESENT => Ok(RadioState::NotPresent),
        READ_PERMISSION_DENIED => Err(DeviceError::PermissionDenied(FEATURE)),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

/// 切り替えの結果コードを契約の結果へ写す。中断・失敗・未知のコードは `Internal`（嘘の成功にしない）。
fn decode_switch(code: i32) -> Result<SwitchOutcome, DeviceError> {
    match code {
        SET_APPLIED => Ok(SwitchOutcome::Applied),
        SET_OPENED => Ok(SwitchOutcome::Opened),
        SET_NOT_CHANGED => Ok(SwitchOutcome::NotChanged),
        SET_UNSUPPORTED => Err(DeviceError::Unsupported(FEATURE)),
        SET_PERMISSION_DENIED => Err(DeviceError::PermissionDenied(FEATURE)),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

/// 要求 1 件を出して結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）。
/// 期限は持たない。止めるのは Java の `onDestroy` の INTERRUPTED と [`reset_pending_tracking`] だけ。
/// 差し替え・破棄された slot は他の要求のものなので消さない。
fn set_with(
    slot: &RequestSlot<i32>,
    start: impl FnOnce(u64) -> bool,
) -> Result<SwitchOutcome, DeviceError> {
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(DeviceError::Internal(FAILED));
    }
    loop {
        match slot.snapshot() {
            Some((cur, Some(code))) if cur == id => {
                slot.clear(id);
                return decode_switch(code);
            }
            Some((cur, None)) if cur == id => std::thread::sleep(POLL_INTERVAL),
            _ => return Err(DeviceError::Internal(FAILED)),
        }
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<i32> = RequestSlot::new();

/// Activity 再生成直後に `android::reset_pending_tracking` から呼ぶ。保留中の要求を捨て、
/// 待機中のワーカーを失敗で戻し、旧要求の遅延完了を id 不一致で無視させる。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    SLOT.reset();
}

/// Android の `RadioControl` 実装。
#[cfg(target_os = "android")]
struct AndroidRadioControl;

#[cfg(target_os = "android")]
impl fandhe_device::radio_control::RadioControl for AndroidRadioControl {
    fn read(&self, kind: RadioKind) -> Result<RadioState, DeviceError> {
        let code = call_read(kind_code(kind)).ok_or(DeviceError::Internal(FAILED))?;
        decode_read(code)
    }

    fn set(&self, kind: RadioKind, on: bool) -> Result<SwitchOutcome, DeviceError> {
        set_with(&SLOT, |id| call_set(id, kind_code(kind), on))
    }
}

/// Android の実装を返す。`device/radio_control.rs` の dispatch は android の cfg 内でだけ呼ぶ。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::radio_control::RadioControl> {
    std::sync::Arc::new(AndroidRadioControl)
}

/// `MainActivity.radioControlRead(int) -> int` を同期で呼ぶ。Activity 未確立・JNI 失敗は `None`。
/// `unsafe` 2 箇所は `android_accessibility_prefs.rs::call_read` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_read(kind: i32) -> Option<i32> {
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(
        |env: &mut jni::Env| -> Result<i32, super::android::JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let value = env.call_method(
                &activity,
                jni::jni_str!("radioControlRead"),
                jni::jni_sig!((int) -> int),
                &[jni::objects::JValue::Int(kind)],
            )?;
            Ok(value.i()?)
        },
    )
    .ok()
}

/// `MainActivity.requestRadioControlSet(long, int, boolean)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `call_read` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_set(id: u64, kind: i32, on: bool) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `call_read` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(
        |env: &mut jni::Env| -> Result<(), super::android::JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `call_read` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestRadioControlSet"),
                jni::jni_sig!((long, int, boolean) -> ()),
                &[
                    jni::objects::JValue::Long(id as jni::sys::jlong),
                    jni::objects::JValue::Int(kind),
                    jni::objects::JValue::Bool(on),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `RadioControlHelper.nativeOnRadioControlResult`（Java 側）から呼ばれる JNI エクスポート。
/// 旧要求の遅延到着は `deliver` が id 不一致で捨てる。ワーカーが結果を待つため再描画は起こさない
/// （画面の更新は effect ドライバが完了を受けて行う）。値はログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クラスは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_RadioControlHelper_nativeOnRadioControlResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    SLOT.deliver(request_id as u64, code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_failed<T>(r: &Result<T, DeviceError>) -> bool {
        matches!(r, Err(DeviceError::Internal(m)) if *m == FAILED)
    }

    #[test]
    fn kind_codes_match_java() {
        assert_eq!(kind_code(RadioKind::Wifi), 0);
        assert_eq!(kind_code(RadioKind::Bluetooth), 1);
    }

    #[test]
    fn decode_read_maps_states_and_never_folds_to_off() {
        assert!(matches!(decode_read(0), Ok(RadioState::On)));
        assert!(matches!(decode_read(1), Ok(RadioState::Off)));
        assert!(matches!(decode_read(2), Ok(RadioState::Transitioning)));
        assert!(matches!(decode_read(3), Ok(RadioState::Unknown)));
        assert!(matches!(decode_read(4), Ok(RadioState::NotPresent)));
        assert!(matches!(
            decode_read(5),
            Err(DeviceError::PermissionDenied(_))
        ));
        for code in [6, 7, 99, -1] {
            assert!(is_failed(&decode_read(code)), "code {code}");
        }
    }

    #[test]
    fn decode_switch_keeps_cancel_as_success_and_failures_as_errors() {
        assert!(matches!(decode_switch(0), Ok(SwitchOutcome::Applied)));
        assert!(matches!(decode_switch(1), Ok(SwitchOutcome::Opened)));
        // RESULT_CANCELED は失敗にしない（三値）。
        assert!(matches!(decode_switch(2), Ok(SwitchOutcome::NotChanged)));
        assert!(matches!(decode_switch(3), Err(DeviceError::Unsupported(_))));
        assert!(matches!(
            decode_switch(4),
            Err(DeviceError::PermissionDenied(_))
        ));
        for code in [5, 6, 7, 99, -1] {
            assert!(is_failed(&decode_switch(code)), "code {code}");
        }
    }

    #[test]
    fn set_with_fails_and_clears_when_start_fails() {
        let slot = RequestSlot::<i32>::new();
        assert!(is_failed(&set_with(&slot, |_| false)));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn set_with_returns_delivered_result_and_clears() {
        let slot = RequestSlot::<i32>::new();
        let r = set_with(&slot, |id| {
            slot.deliver(id, SET_APPLIED);
            true
        });
        assert!(matches!(r, Ok(SwitchOutcome::Applied)));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn set_with_treats_cancel_as_not_changed() {
        let slot = RequestSlot::<i32>::new();
        let r = set_with(&slot, |id| {
            slot.deliver(id, SET_NOT_CHANGED);
            true
        });
        assert!(matches!(r, Ok(SwitchOutcome::NotChanged)));
    }

    #[test]
    fn set_with_ignores_other_ids_and_fails_after_reset() {
        let slot = RequestSlot::<i32>::new();
        let r = set_with(&slot, |id| {
            // 別 id の結果は受理されず、続く reset で待機が失敗で戻る。
            assert!(!slot.deliver(id + 1, SET_APPLIED));
            slot.reset();
            true
        });
        assert!(is_failed(&r));
    }
}
