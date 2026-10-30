//! Android 向けヘルスデータ（今日の歩数）実装（Issue #1100、親 #149）。
//! `device/health_data.rs` の Android 経路から呼ばれ、Health Connect の歩数の合計
//! （API 34 以上のフレームワーク API `HealthConnectManager.aggregate`）を 1 回だけ期限付きで
//! 取得する。Java 側は `HealthDataHelper`（許可の確認・集計・id の照合）。
//!
//! 流れ: `HealthData::read_today_steps`（ワーカースレッド）→ [`RequestSlot::begin`] で id 採番 →
//! JNI `MainActivity.requestHealthDataRead(id)` → Java が結果コードを
//! `nativeOnHealthDataResult` で返す → `fandhe_device::health_data::decode_code` で契約の結果へ写像 →
//! ワーカーが 100ms 周期で slot を見て受け取る。期限は契約の `READ_TIMEOUT` 1 つだけで、
//! 起点はこの入口（単調時計）。切れたら `cancelHealthDataRead(id)` で Java の追跡 id を消し
//! `Timeout`（0 歩にしない）。`aggregate` に取り消し API は無いため、遅れて届いた結果は
//! Java の `RequestTracker` と slot の id 照合で捨てる。
//!
//! 写像（API 33 以下は `Unsupported`、許可なしは `NoVisibleData` など）の正は
//! `fandhe_device::health_data` の doc「Android の写像」。ここでは複製しない。
//!
//! 守る範囲: 1 プロセス内の要求順序・0 歩と未取得の区別・拒否/破棄/例外を成功に畳まないこと。
//! 守らない範囲: 期限後に届く `aggregate` の結果、既定で許可時点の 30 日前までしか読めないこと。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestHealthDataRead` を経た後でしか
//! 呼ばれず、feature `device` 無効ビルドではその経路に到達しない（pedometer と同じ）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use fandhe_device::health_data::HealthData;
use fandhe_device::health_data::{HealthDataFailure, HealthSteps};

use super::request_slot::{RequestSlot, POLL_INTERVAL};

type Outcome = Result<HealthSteps, HealthDataFailure>;

/// 要求 1 件を出して期限まで結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）、
/// `cancel` は期限切れ・差し替え時に Java 側の追跡を消させる。
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
        return Err(HealthDataFailure::Failed);
    }
    loop {
        // 結果の受理より先に期限を確認する（期限後に届いた値は捨てる）。
        let now = Instant::now();
        if now >= deadline {
            slot.clear(id);
            cancel(id);
            return Err(HealthDataFailure::Timeout);
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
                return Err(HealthDataFailure::Failed);
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Outcome> = RequestSlot::new();

/// 要求の実行中フラグ。使用中に来た新要求は slot に触れず `Failed` で拒否する
/// （先行要求の slot を `begin` で上書きして正常結果を失わせない）。
#[cfg(target_os = "android")]
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `BUSY` を Drop で必ず下ろす。
#[cfg(target_os = "android")]
struct BusyGuard;

#[cfg(target_os = "android")]
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(target_os = "android")]
struct AndroidHealthData;

#[cfg(target_os = "android")]
impl HealthData for AndroidHealthData {
    fn read_today_steps(&self, timeout: Duration) -> Outcome {
        if BUSY.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return Err(HealthDataFailure::Failed);
        }
        let _guard = BusyGuard;
        read_with(
            &SLOT,
            timeout,
            |id| call_activity(false, id),
            |id| {
                let _ = call_activity(true, id);
            },
        )
    }
}

/// `MainActivity.requestHealthDataRead(long)`（`cancel=false`）または
/// `cancelHealthDataRead(long)`（`cancel=true`）を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `device/android.rs::request_calendar_insert` と同一の根拠。
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
                jni::jni_str!("cancelHealthDataRead"),
                jni::jni_sig!((long) -> ()),
                &args,
            )?;
        } else {
            env.call_method(
                &activity,
                jni::jni_str!("requestHealthDataRead"),
                jni::jni_sig!((long) -> ()),
                &args,
            )?;
        }
        Ok(())
    })
    .is_ok()
}

/// `HealthDataHelper.nativeOnHealthDataResult`（Java 側）から呼ばれる JNI エクスポート。
/// 歩数・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クラスは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_HealthDataHelper_nativeOnHealthDataResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    steps: jni::sys::jlong,
) {
    SLOT.deliver(
        request_id as u64,
        fandhe_device::health_data::decode_code(code, steps as f64),
    );
}

/// Android の `HealthData` 実装を返す（`device/health_data.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn HealthData> {
    Arc::new(AndroidHealthData)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn HealthData> {
    Arc::new(fandhe_device::health_data::UnsupportedHealthData)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);

    #[test]
    fn non_android_stub_is_not_a_fake_success() {
        assert_eq!(
            implementation().read_today_steps(fandhe_device::health_data::READ_TIMEOUT),
            Err(HealthDataFailure::Unsupported)
        );
    }

    #[test]
    fn delivers_result() {
        let slot = Arc::new(RequestSlot::<Outcome>::new());
        let s = slot.clone();
        let r = read_with(
            &slot,
            T,
            move |id| {
                let s = s.clone();
                std::thread::spawn(move || {
                    s.deliver(id, Ok(HealthSteps::Steps(7)));
                });
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert_eq!(r, Ok(HealthSteps::Steps(7)));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_cancels_once_and_is_timeout() {
        let slot = RequestSlot::<Outcome>::new();
        let cancelled = Cell::new(0);
        let r = read_with(
            &slot,
            Duration::from_millis(150),
            |_| true,
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(r, Err(HealthDataFailure::Timeout));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_is_not_overshot_by_poll_interval() {
        let slot = RequestSlot::<Outcome>::new();
        let t0 = Instant::now();
        let _ = read_with(&slot, Duration::from_millis(150), |_| true, |_| {});
        assert!(t0.elapsed() < Duration::from_millis(190));
    }

    #[test]
    fn result_delivered_after_deadline_is_timeout() {
        let slot = RequestSlot::<Outcome>::new();
        let cancelled = Cell::new(0);
        let r = read_with(
            &slot,
            Duration::from_millis(10),
            |id| {
                std::thread::sleep(Duration::from_millis(20));
                assert!(slot.deliver(id, Ok(HealthSteps::Steps(7))));
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(r, Err(HealthDataFailure::Timeout));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn start_failure_is_failed_and_clears() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(&slot, T, |_| false, |_| panic!("no cancel"));
        assert_eq!(r, Err(HealthDataFailure::Failed));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn stale_id_is_ignored_until_timeout() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(
            &slot,
            Duration::from_millis(150),
            |id| {
                assert!(!slot.deliver(id + 100, Ok(HealthSteps::Steps(1))));
                true
            },
            |_| {},
        );
        assert_eq!(r, Err(HealthDataFailure::Timeout));
    }

    #[test]
    fn replaced_request_returns_failed_immediately_and_keeps_new_slot() {
        let slot = RequestSlot::<Outcome>::new();
        let new_id = Cell::new(0);
        let cancelled = Cell::new(0);
        let t0 = Instant::now();
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
        assert_eq!(r, Err(HealthDataFailure::Failed));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(slot.snapshot(), Some((new_id.get(), None)));
    }
}
