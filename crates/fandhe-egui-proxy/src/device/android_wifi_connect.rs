//! Android 向け「指定した Wi-Fi への接続」実装（Issue #1065、親 #1060）。
//! `device/wifi_connect.rs` の dispatch（Android 経路）から呼ばれ、契約の SSOT は
//! `fandhe_device::wifi_connect`。Java 側は `WifiConnectHelper`（Specifier の生成・
//! `requestNetwork`・callback・解放）。
//!
//! 流れ: `WifiConnect::connect`（ワーカースレッド）→ [`RequestSlot::begin`] で id 採番 →
//! JNI `MainActivity.requestWifiConnect(id, ssid, password, hexKey)` → Java が結果コードを
//! `nativeOnWifiConnectResult` で返す → [`decode`] で契約の失敗種別へ写像 → ワーカーが slot を見て受け取る。
//!
//! 期限は持たない。待ちを終える保証は、Java の `onDestroy` が保留中の要求を FAILED で返すこと
//! （trait doc の「ポーリングを持たない」は OS への問い直しのこと。slot の監視は内部の待ち）。
//! 16 進 64 文字の鍵の判定はここだけで行い、JNI へは bool で渡す（定数を Java に複製しない）。
//!
//! 守る範囲・守らない範囲は trait doc を参照。SSID・パスワードはログ・エラー・`Debug` に出さず、
//! trim しない。no-op スタブは置かない: Java の native は Rust の要求を経た後でしか呼ばれない。

use std::sync::Arc;

use fandhe_device::wifi_connect::{
    ConnectRequest, WifiConnect, CONNECT_FAILED, FEATURE, RADIO_OFF,
};
use fandhe_device::DeviceError;

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 契約の失敗種別（`DeviceError` は `Clone` でないため slot 用に写した形）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fail {
    Unsupported,
    RadioOff,
    Failed,
}

impl From<Fail> for DeviceError {
    fn from(f: Fail) -> Self {
        match f {
            Fail::Unsupported => DeviceError::Unsupported(FEATURE),
            Fail::RadioOff => DeviceError::Disabled(RADIO_OFF),
            Fail::Failed => DeviceError::Internal(CONNECT_FAILED),
        }
    }
}

type Outcome = Result<(), Fail>;

/// 結果コード（SSOT。`WifiConnectHelper.RESULT_*` と同じ番号）。未知の値は `Failed`（成功へ畳まない）。
fn decode(code: i32) -> Outcome {
    match code {
        0 => Ok(()),
        1 => Err(Fail::Unsupported),
        2 => Err(Fail::RadioOff),
        _ => Err(Fail::Failed),
    }
}

/// JNI へ渡す引数 `(ssid, password, hex_key)`。パスワードは trim しない。
/// SSID が UTF-8 に戻せない場合は `Failed`（`validate` が保証するため通常は到達しない）。
fn java_args(req: &ConnectRequest) -> Result<(&str, Option<&str>, bool), Fail> {
    let ssid = std::str::from_utf8(req.ssid.as_bytes()).map_err(|_| Fail::Failed)?;
    let password = req.password.as_ref().map(|p| p.expose());
    // `validate` を通した 64 文字は 16 進の鍵だけ。意図を明示するため全文字も見る。
    let hex_key =
        password.is_some_and(|p| p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit()));
    Ok((ssid, password, hex_key))
}

/// 要求 1 件を出して結果を待つ（期限なし）。`start` は OS へ要求を渡す（false は呼び出し失敗）。
fn connect_with(slot: &RequestSlot<Outcome>, start: impl FnOnce(u64) -> bool) -> Outcome {
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(Fail::Failed);
    }
    loop {
        match slot.snapshot() {
            Some((cur, Some(outcome))) if cur == id => {
                slot.clear(id);
                return outcome;
            }
            Some((cur, None)) if cur == id => {}
            // 差し替え済み（他人の slot は消さない）。
            _ => return Err(Fail::Failed),
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Outcome> = RequestSlot::new();

#[cfg(target_os = "android")]
struct AndroidWifiConnect;

#[cfg(target_os = "android")]
impl WifiConnect for AndroidWifiConnect {
    fn connect(&self, request: &ConnectRequest) -> Result<(), DeviceError> {
        let (ssid, password, hex_key) = java_args(request)?;
        connect_with(&SLOT, |id| call_activity(id, ssid, password, hex_key))
            .map_err(DeviceError::from)
    }
}

/// `MainActivity.requestWifiConnect(long, String, String, boolean)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_geocoding_search.rs::call_request` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, ssid: &str, password: Option<&str>, hex_key: bool) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_ssid = env.new_string(ssid)?;
        let j_password = match password {
            Some(p) => jni::objects::JObject::from(env.new_string(p)?),
            None => jni::objects::JObject::null(),
        };
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestWifiConnect"),
            jni::jni_sig!((long, java.lang.String, java.lang.String, boolean) -> ()),
            &[
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Object(&j_ssid),
                jni::objects::JValue::Object(&j_password),
                jni::objects::JValue::Bool(hex_key),
            ],
        )?;
        Ok(())
    })
    .is_ok()
}

/// `WifiConnectHelper.nativeOnWifiConnectResult`（Java 側）から呼ばれる JNI エクスポート。
/// 結果は整数コードだけで、SSID・パスワードは受け取らない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。引数はプリミティブのみで JNI 環境も使わない。
/// `#[no_mangle]` は JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_WifiConnectHelper_nativeOnWifiConnectResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    SLOT.deliver(request_id as u64, decode(code));
}

/// Android の `WifiConnect` 実装を返す（`device/wifi_connect.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn WifiConnect> {
    Arc::new(AndroidWifiConnect)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn WifiConnect> {
    Arc::new(fandhe_device::wifi_connect::UnsupportedWifiConnect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::wifi_connect::validate;
    use std::cell::Cell;

    #[test]
    fn decode_maps_codes_and_unknown_is_failed() {
        assert_eq!(decode(0), Ok(()));
        assert_eq!(decode(1), Err(Fail::Unsupported));
        assert_eq!(decode(2), Err(Fail::RadioOff));
        assert_eq!(decode(3), Err(Fail::Failed));
        assert_eq!(decode(99), Err(Fail::Failed));
        assert_eq!(decode(-1), Err(Fail::Failed));
    }

    #[test]
    fn fail_maps_to_contract_errors() {
        assert!(matches!(
            DeviceError::from(Fail::Unsupported),
            DeviceError::Unsupported(FEATURE)
        ));
        assert!(matches!(
            DeviceError::from(Fail::RadioOff),
            DeviceError::Disabled(RADIO_OFF)
        ));
        assert!(matches!(
            DeviceError::from(Fail::Failed),
            DeviceError::Internal(CONNECT_FAILED)
        ));
    }

    #[test]
    fn java_args_cases() {
        let open = validate("home", "").unwrap();
        assert_eq!(java_args(&open), Ok(("home", None, false)));
        let wpa = validate(" home ", " pass word ").unwrap();
        assert_eq!(java_args(&wpa), Ok((" home ", Some(" pass word "), false)));
        let hex = "f".repeat(64);
        let key = validate("home", &hex).unwrap();
        assert_eq!(java_args(&key), Ok(("home", Some(hex.as_str()), true)));
    }

    #[test]
    fn delivers_result() {
        let slot = Arc::new(RequestSlot::<Outcome>::new());
        let sl = slot.clone();
        let r = connect_with(&slot, move |id| {
            let sl = sl.clone();
            std::thread::spawn(move || {
                sl.deliver(id, Ok(()));
            });
            true
        });
        assert_eq!(r, Ok(()));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn failed_result_is_not_success() {
        let slot = Arc::new(RequestSlot::<Outcome>::new());
        let sl = slot.clone();
        let r = connect_with(&slot, move |id| {
            sl.deliver(id, decode(3));
            true
        });
        assert_eq!(r, Err(Fail::Failed));
    }

    #[test]
    fn start_failure_is_failed() {
        let slot = RequestSlot::<Outcome>::new();
        assert_eq!(connect_with(&slot, |_| false), Err(Fail::Failed));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn replaced_request_fails_and_keeps_new_slot() {
        let slot = RequestSlot::<Outcome>::new();
        let new_id = Cell::new(0);
        let r = connect_with(&slot, |_| {
            new_id.set(slot.begin());
            true
        });
        assert_eq!(r, Err(Fail::Failed));
        assert_eq!(slot.snapshot(), Some((new_id.get(), None)));
    }

    #[test]
    fn non_android_is_not_a_fake_success() {
        let req = validate("home", "").expect("検査を通る");
        assert!(matches!(
            implementation().connect(&req),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
