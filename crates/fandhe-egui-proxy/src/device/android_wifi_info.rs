//! Android 向け Wi-Fi 情報実装（Issue #516、親 #111）。`device/wifi_info.rs::implementation`
//! の Android 経路から呼ばれ、現在の接続の SSID・dBm と OS が持つ直近のスキャン結果を
//! 期限付きで 1 回読む。Java 側は `WifiInfoHelper`（許可確認・状態判定・読み取り・取り消し）。
//! 契約の SSOT は `fandhe_device::wifi_info`。
//!
//! 流れ: `WifiInfo::read`（ワーカースレッド）→ [`RequestSlot::begin`] で id 採番 →
//! JNI `MainActivity.requestWifiInfo(id)` → Java が結果コードと値を `nativeOnWifiInfoResult`
//! で返す → [`decode`] で契約の失敗種別へ写像し、SSID は `Ssid::from_bytes`・dBm は
//! `rssi_from_dbm`・周辺は `normalize_nearby` に通す → ワーカーが slot を見て受け取る。
//! 期限 [`READ_TIMEOUT`]（許可ダイアログの応答待ちを含む）が切れたら `cancelWifiInfo(id)` で
//! 取り消し `Internal(READ_FAILED)`（0 件の成功にしない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・0 件と失敗の区別（位置情報オフ・許可なし・電波オフは
//! 読む前に Java が判定）・壊れた入力を成功に畳まないこと。守らない範囲: スキャン結果の
//! 鮮度、読み取り後の変化、非 UTF-8 の SSID（「非公開」と同じ表示）。
//!
//! #510 で実機確認する未確認事項: API 31 以降の `getConnectionInfo` が位置情報の許可で SSID を
//! 返すか、スキャンキャッシュが空のときの出方。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestWifiInfo` を経た後でしか呼ばれない。

use std::sync::Arc;
use std::time::{Duration, Instant};

use fandhe_device::wifi_info::{
    normalize_nearby, rssi_from_dbm, Ssid, WifiInfo, WifiNetwork, WifiReport, FEATURE,
    LOCATION_OFF, RADIO_OFF, READ_FAILED,
};
use fandhe_device::DeviceError;

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 要求の期限（呼び出し開始から。許可ダイアログの応答待ちを含む）。
/// trait の `read()` は期限を持たないため、Java が応答しない場合に備える Android 側の上限。
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Java が送る周辺の最大件数。SSID ごとの集約と上位 5 件への絞り込みは `normalize_nearby` が行う。
const NEARBY_CAP: usize = 64;

/// 契約の失敗種別（`DeviceError` は `Clone` でないため slot 用に写した形）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fail {
    Unsupported,
    PermissionDenied,
    RadioOff,
    LocationOff,
    Failed,
}

impl From<Fail> for DeviceError {
    fn from(f: Fail) -> Self {
        match f {
            Fail::Unsupported => DeviceError::Unsupported(FEATURE),
            Fail::PermissionDenied => DeviceError::PermissionDenied(FEATURE),
            Fail::RadioOff => DeviceError::Disabled(RADIO_OFF),
            Fail::LocationOff => DeviceError::Disabled(LOCATION_OFF),
            Fail::Failed => DeviceError::Internal(READ_FAILED),
        }
    }
}

type Outcome = Result<WifiReport, Fail>;

/// 結果コード（SSOT。`WifiInfoHelper.RESULT_*` と同じ番号）。`0`（成功）は `None`。
/// 未知の値は `Failed`（成功へ畳まない）。
fn decode_code(code: i32) -> Option<Fail> {
    match code {
        0 => None,
        1 => Some(Fail::Unsupported),
        2 => Some(Fail::PermissionDenied),
        3 => Some(Fail::RadioOff),
        4 => Some(Fail::LocationOff),
        _ => Some(Fail::Failed),
    }
}

fn network(ssid: Option<String>, dbm: i32) -> WifiNetwork {
    WifiNetwork {
        ssid: ssid.and_then(|s| Ssid::from_bytes(s.as_bytes())),
        rssi_dbm: rssi_from_dbm(dbm),
    }
}

/// Java の結果コードと値を契約の結果へ写像する。`current` は `Some((ssid, dbm))` で接続中。
/// 成功時に配列が無い・長さ不一致・上限超過のときは、組がずれるため捨てずに `Failed`
/// （範囲の検証は変換より先に行う）。
fn decode(
    code: i32,
    current: Option<(Option<String>, i32)>,
    ssids: Option<Vec<Option<String>>>,
    rssis: Option<Vec<i32>>,
) -> Outcome {
    if let Some(f) = decode_code(code) {
        return Err(f);
    }
    let (ssids, rssis) = ssids.zip(rssis).ok_or(Fail::Failed)?;
    if ssids.len() != rssis.len() || ssids.len() > NEARBY_CAP {
        return Err(Fail::Failed);
    }
    Ok(WifiReport {
        current: current.map(|(s, d)| network(s, d)),
        nearby: normalize_nearby(ssids.into_iter().zip(rssis).map(|(s, d)| network(s, d))),
    })
}

/// 要求 1 件を出して期限まで結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）、
/// `cancel` は期限切れ・差し替え時に Java 側の要求を取り消させる。
fn read_with(
    slot: &RequestSlot<Outcome>,
    timeout: Duration,
    start: impl FnOnce(u64) -> bool,
    cancel: impl FnOnce(u64),
) -> Outcome {
    // 期限は呼び出し開始時点から数える（JNI 呼び出しの所要時間も含める）。
    let deadline = Instant::now() + timeout;
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(Fail::Failed);
    }
    loop {
        // 結果の受理より先に期限を確認する（期限後に届いた値は捨てる）。
        let now = Instant::now();
        if now >= deadline {
            slot.clear(id);
            cancel(id);
            return Err(Fail::Failed);
        }
        match slot.snapshot() {
            Some((cur, Some(outcome))) if cur == id => {
                slot.clear(id);
                return outcome;
            }
            Some((cur, None)) if cur == id => {}
            // 差し替え済み（他人の slot は消さない）。
            _ => {
                cancel(id);
                return Err(Fail::Failed);
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Outcome> = RequestSlot::new();

#[cfg(target_os = "android")]
struct AndroidWifiInfo;

#[cfg(target_os = "android")]
impl WifiInfo for AndroidWifiInfo {
    fn read(&self) -> Result<WifiReport, DeviceError> {
        read_with(
            &SLOT,
            READ_TIMEOUT,
            |id| call_activity(false, id),
            |id| {
                let _ = call_activity(true, id);
            },
        )
        .map_err(DeviceError::from)
    }
}

/// `MainActivity.requestWifiInfo(long)`（`cancel=false`）または
/// `cancelWifiInfo(long)`（`cancel=true`）を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_pedometer.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(cancel: bool, id: u64) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        let args = [jni::objects::JValue::Long(id as jni::sys::jlong)];
        if cancel {
            env.call_method(
                &activity,
                jni::jni_str!("cancelWifiInfo"),
                jni::jni_sig!((long) -> ()),
                &args,
            )?;
        } else {
            env.call_method(
                &activity,
                jni::jni_str!("requestWifiInfo"),
                jni::jni_sig!((long) -> ()),
                &args,
            )?;
        }
        Ok(())
    })
    .is_ok()
}

/// `WifiInfoHelper.nativeOnWifiInfoResult`（Java 側）から呼ばれる JNI エクスポート。
/// SSID は一切ログへ出さない。値が読めなければ全体を `Failed` にする。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_WifiInfoHelper_nativeOnWifiInfoResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    connected: jni::sys::jboolean,
    current_ssid: jni::objects::JString<'caller>,
    current_rssi: jni::sys::jint,
    ssids: jni::objects::JObjectArray<'caller>,
    rssis: jni::objects::JIntArray<'caller>,
) {
    use jni::objects::JString;
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let current = if connected {
            let s = if current_ssid.is_null() {
                None
            } else {
                Some(current_ssid.try_to_string(env)?)
            };
            Some((s, current_rssi))
        } else {
            None
        };
        let (ss, rs) = if ssids.is_null() || rssis.is_null() {
            (None, None)
        } else {
            let n = ssids.len(env)?;
            let m = rssis.len(env)?;
            // 上限超過・長さ不一致は読まずに壊れた入力として確定する。
            if n > NEARBY_CAP || n != m {
                SLOT.deliver(request_id as u64, Err(Fail::Failed));
                return Ok(());
            }
            {
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    let obj = ssids.get_element(env, i)?;
                    if obj.is_null() {
                        v.push(None);
                        continue;
                    }
                    let s = env.cast_local::<JString>(obj)?;
                    v.push(Some(s.try_to_string(env)?));
                }
                let mut r = vec![0i32; m];
                rssis.get_region(env, 0, &mut r)?;
                (Some(v), Some(r))
            }
        };
        SLOT.deliver(request_id as u64, decode(code, current, ss, rs));
        Ok(())
    });
    if !matches!(outcome.into_outcome(), jni::Outcome::Ok(_)) {
        // JNI 例外などで値を読めなかった。要求を放置せず Failed で確定する。
        SLOT.deliver(request_id as u64, Err(Fail::Failed));
    }
}

/// Android の `WifiInfo` 実装を返す（`device/wifi_info.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn WifiInfo> {
    Arc::new(AndroidWifiInfo)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn WifiInfo> {
    Arc::new(fandhe_device::wifi_info::UnsupportedWifiInfo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    fn ok_empty() -> Outcome {
        decode(0, None, Some(vec![]), Some(vec![]))
    }

    #[test]
    fn decode_maps_codes_and_unknown_is_failed() {
        let f = |c| decode(c, None, None, None);
        assert_eq!(f(1), Err(Fail::Unsupported));
        assert_eq!(f(2), Err(Fail::PermissionDenied));
        assert_eq!(f(3), Err(Fail::RadioOff));
        assert_eq!(f(4), Err(Fail::LocationOff));
        assert_eq!(f(5), Err(Fail::Failed));
        assert_eq!(f(99), Err(Fail::Failed));
        // 成功コードなのに配列が無い入力は成功にしない。
        assert_eq!(f(0), Err(Fail::Failed));
    }

    #[test]
    fn fail_maps_to_contract_errors() {
        assert!(matches!(
            DeviceError::from(Fail::LocationOff),
            DeviceError::Disabled(LOCATION_OFF)
        ));
        assert!(matches!(
            DeviceError::from(Fail::Failed),
            DeviceError::Internal(READ_FAILED)
        ));
    }

    #[test]
    fn zero_results_is_success() {
        let r = ok_empty().unwrap();
        assert!(r.current.is_none() && r.nearby.is_empty());
    }

    #[test]
    fn decode_rejects_mismatch_oversize() {
        assert_eq!(
            decode(0, None, Some(vec![s("a")]), Some(vec![])),
            Err(Fail::Failed)
        );
        assert_eq!(
            decode(
                0,
                None,
                Some(vec![None; NEARBY_CAP + 1]),
                Some(vec![-50; NEARBY_CAP + 1])
            ),
            Err(Fail::Failed)
        );
    }

    #[test]
    fn dbm_sentinels_and_bad_ssid_become_none() {
        let r = decode(
            0,
            Some((s("home"), -127)),
            Some(vec![s("a"), s("")]),
            Some(vec![0, -60]),
        )
        .unwrap();
        let cur = r.current.unwrap();
        assert!(cur.ssid.is_some() && cur.rssi_dbm.is_none());
        assert_eq!(r.nearby.len(), 2);
        assert_eq!(r.nearby[0].rssi_dbm, Some(-60));
        assert!(r.nearby[0].ssid.is_none());
    }

    #[test]
    fn nearby_is_capped_to_five() {
        let ssids = (0..10).map(|i| s(&format!("n{i}"))).collect();
        let r = decode(0, None, Some(ssids), Some((1..=10).map(|i| -i).collect())).unwrap();
        assert_eq!(r.nearby.len(), 5);
    }

    #[test]
    fn ssid_is_not_in_debug() {
        let r = decode(0, Some((s("secret-net"), -40)), Some(vec![]), Some(vec![])).unwrap();
        assert!(!format!("{r:?}").contains("secret-net"));
    }

    #[test]
    fn delivers_result() {
        let slot = Arc::new(RequestSlot::<Outcome>::new());
        let sl = slot.clone();
        let r = read_with(
            &slot,
            T,
            move |id| {
                let sl = sl.clone();
                std::thread::spawn(move || {
                    sl.deliver(id, ok_empty());
                });
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert!(r.is_ok());
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_cancels_once_and_fails() {
        let slot = RequestSlot::<Outcome>::new();
        let cancelled = Cell::new(0);
        let r = read_with(
            &slot,
            Duration::from_millis(150),
            |_| true,
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(r, Err(Fail::Failed));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn result_after_deadline_is_failed() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(
            &slot,
            Duration::from_millis(10),
            |id| {
                std::thread::sleep(Duration::from_millis(20));
                assert!(slot.deliver(id, ok_empty()));
                true
            },
            |_| {},
        );
        assert_eq!(r, Err(Fail::Failed));
    }

    #[test]
    fn start_failure_is_failed() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(&slot, T, |_| false, |_| panic!("no cancel"));
        assert_eq!(r, Err(Fail::Failed));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn replaced_request_cancels_and_keeps_new_slot() {
        let slot = RequestSlot::<Outcome>::new();
        let new_id = Cell::new(0);
        let cancelled = Cell::new(0);
        let r = read_with(
            &slot,
            Duration::from_secs(5),
            |_| {
                new_id.set(slot.begin());
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(cancelled.get(), 1);
        assert_eq!(r, Err(Fail::Failed));
        assert_eq!(slot.snapshot(), Some((new_id.get(), None)));
    }
}
