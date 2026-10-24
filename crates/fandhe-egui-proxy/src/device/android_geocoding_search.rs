//! Android 向け住所の座標変換（Issue #693、親 #133）。`device/geocoding_search.rs::platform` の
//! Android 経路から呼ばれ、`android.location.Geocoder` で画面の固定住所 1 件を座標へ変換する。
//! Java 側は `GeocodingHelper`（API 33 以上は `GeocodeListener` 版、24〜32 は同期版）。
//!
//! 流れ: `ForwardGeocoder::geocode`（ドライバのワーカースレッド）→ `validate_address` →
//! [`RequestSlot::begin`] で id 採番 → JNI `MainActivity.requestGeocode(id, address, maxResults)` →
//! Java が結果コードと先頭候補の座標・候補数を `nativeOnGeocodeResult` で返す → [`decode`] で
//! 契約の失敗種別へ写像 → ワーカーが周期的に slot を見て受け取る。期限は引数 `remaining`
//! （ドライバが `GEOCODE_DEADLINE` を渡す）だけで、切れたら `cancelGeocode(id)` で追跡を外して
//! `Internal(TIMEOUT)`。
//!
//! 写像（SSOT は `fandhe_device::geocoding_search`）: `Geocoder.isPresent()` が false は
//! `Unsupported`、空リストは `NOT_FOUND`、`hasLatitude/hasLongitude` が偽や範囲外は `NO_COORDINATE`
//! （いずれも `check_result`）、`IOException`・`onError`・中断・未知のコードは
//! `Internal("geocoding_search")`（通信不可と回数上限は区別しない）。`maxResults` は
//! `MAX_CANDIDATES` を JNI の引数で渡す。位置情報の権限・Gradle 依存は足さない。
//!
//! ライフサイクル: Activity 再生成（`onDestroy`）で保留中の要求を Java が中断扱いで確定する。
//! `SLOT` の id は単調増加で、新しい `GeocodingHelper` は未追跡から始まるため旧結果が新しい
//! 要求を消費しない。Rust 側の Session は `android.rs` から `reset_pending_tracking` で破棄する。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・期限後の結果の破棄。守らない範囲: 期限後も
//! OS 側の変換が走り切ること（Geocoder に取り消し API は無い）、同期版が詰まったときの後続要求の
//! 遅れ（Rust の期限で失敗が確定する）、結果の精度と候補の選択、provider の有無による端末差。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestGeocode` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（translation と同じ）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use fandhe_device::geocoding_search::{check_result, ForwardGeocoder, GeocodeResult, TIMEOUT};
#[cfg(target_os = "android")]
use fandhe_device::geocoding_search::{validate_address, MAX_CANDIDATES};
use fandhe_device::DeviceError;

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 結果コード（SSOT は Java 側 `GeocodingHelper.RESULT_*`）。
const RESULT_OK: i32 = 0;
const RESULT_UNSUPPORTED: i32 = 1;
/// Activity 破棄による中断。Rust 側では未知のコードと同じ失敗へ写すため、値の記録とテスト用。
#[allow(dead_code)]
const RESULT_INTERRUPTED: i32 = 2;
#[allow(dead_code)]
const RESULT_FAILED: i32 = 3;

type Outcome = Result<GeocodeResult, DeviceError>;
/// slot に置く生の結果（結果コード, 緯度, 経度, 座標あり, 候補数）。
type Raw = (i32, f64, f64, bool, i32);

/// Java の結果を契約の結果へ写像する。0 件・座標なし・範囲外は `check_result` が判定する。
fn decode(raw: Raw) -> Outcome {
    let (code, lat, lon, has, count) = raw;
    match code {
        RESULT_OK => match u32::try_from(count) {
            Ok(n) => check_result(lat, lon, has, n),
            Err(_) => Err(DeviceError::Internal("geocoding_search")),
        },
        RESULT_UNSUPPORTED => Err(DeviceError::Unsupported("geocoding_search")),
        // 中断（RESULT_INTERRUPTED）・OS の失敗・未知のコード。
        _ => Err(DeviceError::Internal("geocoding_search")),
    }
}

/// 要求 1 件を出して期限まで結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）、
/// `cancel` は期限切れ・差し替え時に Java 側の追跡を外させる。
fn geocode_with(
    slot: &RequestSlot<Raw>,
    remaining: Duration,
    start: impl FnOnce(u64) -> bool,
    cancel: impl FnOnce(u64),
) -> Outcome {
    // 期限は呼び出し開始時点から数える（JNI 呼び出しの所要時間も含める）。単調時計を使う。
    let deadline = Instant::now() + remaining;
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(DeviceError::Internal("geocoding_search"));
    }
    loop {
        // 結果の受理より先に期限を確認する（期限後に届いた結果は捨てる）。
        let now = Instant::now();
        if now >= deadline {
            slot.clear(id);
            cancel(id);
            return Err(DeviceError::Internal(TIMEOUT));
        }
        match slot.snapshot() {
            Some((cur, Some(raw))) if cur == id => {
                slot.clear(id);
                return decode(raw);
            }
            Some((cur, None)) if cur == id => {}
            // 差し替え済み（他人の slot は消さない）。Java 側の追跡は外させる。
            _ => {
                cancel(id);
                return Err(DeviceError::Internal("geocoding_search"));
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Raw> = RequestSlot::new();

#[cfg(target_os = "android")]
struct AndroidForwardGeocoder;

#[cfg(target_os = "android")]
impl ForwardGeocoder for AndroidForwardGeocoder {
    fn geocode(&self, address: &str, remaining: Duration) -> Outcome {
        validate_address(address)?;
        geocode_with(
            &SLOT,
            remaining,
            |id| call_request(id, address, MAX_CANDIDATES as i32),
            call_cancel,
        )
    }
}

/// `MainActivity.requestGeocode(long, String, int)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `device/android_pedometer.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_request(id: u64, address: &str, max_results: i32) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_address = env.new_string(address)?;
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestGeocode"),
            jni::jni_sig!((long, java.lang.String, int) -> ()),
            &[
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Object(&j_address),
                jni::objects::JValue::Int(max_results),
            ],
        )?;
        Ok(())
    })
    .is_ok()
}

/// `MainActivity.cancelGeocode(long)` を JNI で呼ぶ。失敗しても期限切れの確定は変わらない。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_cancel(id: u64) {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return;
    };
    // SAFETY: `call_request` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let _ = vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: `call_request` と同一の根拠。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("cancelGeocode"),
            jni::jni_sig!((long) -> ()),
            &[jni::objects::JValue::Long(id as jni::sys::jlong)],
        )?;
        Ok(())
    });
}

/// `GeocodingHelper.nativeOnGeocodeResult`（Java 側）から呼ばれる JNI エクスポート。
/// 住所・座標は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_GeocodingHelper_nativeOnGeocodeResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    latitude: jni::sys::jdouble,
    longitude: jni::sys::jdouble,
    has_coordinate: jni::sys::jboolean,
    count: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|_env| -> Result<(), jni::errors::Error> {
        SLOT.deliver(
            request_id as u64,
            (code, latitude, longitude, has_coordinate, count),
        );
        Ok(())
    });
    if !matches!(outcome.into_outcome(), jni::Outcome::Ok(_)) {
        // JNI 例外などで確定できなかった。要求を放置せず失敗で確定する。
        SLOT.deliver(request_id as u64, (RESULT_FAILED, 0.0, 0.0, false, 0));
    }
}

/// Android の `ForwardGeocoder` 実装を返す（`device/geocoding_search.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn ForwardGeocoder> {
    Arc::new(AndroidForwardGeocoder)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn ForwardGeocoder> {
    Arc::new(fandhe_device::geocoding_search::UnsupportedForwardGeocoder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::geocoding_search::{MAX_CANDIDATES, NOT_FOUND, NO_COORDINATE};
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);
    const OK_RAW: Raw = (RESULT_OK, 35.68, 139.76, true, 3);

    fn is_internal(r: &Outcome, s: &str) -> bool {
        matches!(r, Err(DeviceError::Internal(x)) if *x == s)
    }

    #[test]
    fn decode_maps_codes() {
        let ok = decode(OK_RAW).unwrap();
        assert_eq!(
            (ok.latitude, ok.longitude, ok.candidates),
            (35.68, 139.76, 3)
        );
        assert!(is_internal(
            &decode((RESULT_OK, 0.0, 0.0, false, 0)),
            NOT_FOUND
        ));
        assert!(is_internal(
            &decode((RESULT_OK, 0.0, 0.0, false, 2)),
            NO_COORDINATE
        ));
        assert!(is_internal(
            &decode((RESULT_OK, 91.0, 0.0, true, 1)),
            NO_COORDINATE
        ));
        assert!(is_internal(
            &decode((RESULT_OK, 0.0, 0.0, true, -1)),
            "geocoding_search"
        ));
        assert!(matches!(
            decode((RESULT_UNSUPPORTED, 0.0, 0.0, false, 0)),
            Err(DeviceError::Unsupported("geocoding_search"))
        ));
        for code in [RESULT_INTERRUPTED, RESULT_FAILED, 99] {
            assert!(is_internal(
                &decode((code, 1.0, 1.0, true, 1)),
                "geocoding_search"
            ));
        }
    }

    #[test]
    fn decode_caps_candidates() {
        let r = decode((RESULT_OK, 0.0, 0.0, true, 50)).unwrap();
        assert_eq!(r.candidates, MAX_CANDIDATES);
    }

    #[test]
    fn delivers_result() {
        let slot = Arc::new(RequestSlot::<Raw>::new());
        let s = slot.clone();
        let r = geocode_with(
            &slot,
            T,
            move |id| {
                let s = s.clone();
                std::thread::spawn(move || {
                    s.deliver(id, OK_RAW);
                });
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert_eq!(r.unwrap().candidates, 3);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_cancels_once_and_is_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let cancelled = Cell::new(0);
        let r = geocode_with(
            &slot,
            Duration::from_millis(150),
            |_| true,
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert!(is_internal(&r, TIMEOUT));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_is_not_overshot_by_poll_interval() {
        let slot = RequestSlot::<Raw>::new();
        let t0 = Instant::now();
        let _ = geocode_with(&slot, Duration::from_millis(150), |_| true, |_| {});
        assert!(t0.elapsed() < Duration::from_millis(190));
    }

    #[test]
    fn result_delivered_after_deadline_is_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let cancelled = Cell::new(0);
        let r = geocode_with(
            &slot,
            Duration::from_millis(10),
            |id| {
                std::thread::sleep(Duration::from_millis(20));
                assert!(slot.deliver(id, OK_RAW));
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert!(is_internal(&r, TIMEOUT));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn start_failure_is_failed_and_clears() {
        let slot = RequestSlot::<Raw>::new();
        let r = geocode_with(&slot, T, |_| false, |_| panic!("no cancel"));
        assert!(is_internal(&r, "geocoding_search"));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn stale_id_is_ignored_until_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let r = geocode_with(
            &slot,
            Duration::from_millis(150),
            |id| {
                assert!(!slot.deliver(id + 100, OK_RAW));
                true
            },
            |_| {},
        );
        assert!(is_internal(&r, TIMEOUT));
    }

    #[test]
    fn replaced_request_returns_immediately_and_keeps_new_slot() {
        let slot = RequestSlot::<Raw>::new();
        let new_id = Cell::new(0);
        let cancelled = Cell::new(0);
        let t0 = Instant::now();
        let r = geocode_with(
            &slot,
            Duration::from_secs(5),
            |_| {
                new_id.set(slot.begin());
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(cancelled.get(), 1);
        assert!(is_internal(&r, "geocoding_search"));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(
            slot.snapshot().map(|(i, o)| (i, o.is_none())),
            Some((new_id.get(), true))
        );
    }

    #[test]
    fn old_result_does_not_consume_new_request() {
        // Activity 再生成の模擬: 旧 id の要求の後に新しい要求が begin される。
        let slot = RequestSlot::<Raw>::new();
        let old = slot.begin();
        let new = slot.begin();
        assert!(!slot.deliver(old, OK_RAW));
        assert_eq!(
            slot.snapshot().map(|(i, o)| (i, o.is_none())),
            Some((new, true))
        );
    }
}
