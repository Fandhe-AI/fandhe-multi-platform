//! Android 向けヘルスデータ（今日の歩数）実装（Issue #1100、親 #149）。
//! `driver.rs` の Android 経路から呼ばれ、Health Connect の歩数の合計
//! （API 34 以上のフレームワーク API `HealthConnectManager.aggregate`）を 1 回だけ期限付きで
//! 取得する。Java 側は `HealthDataHelper`（許可の確認・集計・id の照合）。
//!
//! 流れ: `HealthData::read_today_steps`（ワーカースレッド）→ [`RequestSlot::begin`] で id 採番 →
//! JNI `HealthDataHelper.read(activity, id)`（static）→ Java が結果コードを
//! `nativeOnHealthDataResult` で返す → `crate::contract::decode_code` で契約の結果へ写像 →
//! ワーカーが 100ms 周期で slot を見て受け取る。期限は契約の `READ_TIMEOUT` 1 つだけで、
//! 起点はこの入口（単調時計）。切れたら `HealthDataHelper.cancel(activity, id)` で Java の追跡 id を消し
//! `Timeout`（0 歩にしない）。`aggregate` に取り消し API は無いため、遅れて届いた結果は
//! Java の `RequestTracker` と slot の id 照合で捨てる。
//!
//! 写像（API 33 以下は `Unsupported`、許可なしは `NoVisibleData` など）の正は
//! `contract.rs` の doc「Android の写像」。ここでは複製しない。
//!
//! 守る範囲: 1 プロセス内の要求順序・0 歩と未取得の区別・拒否/破棄/例外を成功に畳まないこと。
//! 守らない範囲: 期限後に届く `aggregate` の結果、既定で許可時点の 30 日前までしか読めないこと。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestHealthDataRead` を経た後でしか
//! 呼ばれず、この crate を選ばないアプリではその経路に到達しない（pedometer と同じ）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::contract::HealthData;
use crate::contract::{HealthDataFailure, HealthSteps};

use fandhe_runtime::request_slot::{POLL_INTERVAL, RequestSlot};

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

/// `HealthDataHelper.read(Activity, long)`（`cancel=false`）または `cancel(Activity, long)`（`cancel=true`）
/// （どちらも static）を JNI で呼ぶ。クラスは Activity の class loader で解決する
/// （`fandhe_platform_android::with_activity_class`）。戻りは `read` では Helper が要求を受理したか
/// （Helper が無い＝生成前・破棄後は `false`。呼び出し元が `Failed` で確定する）、`cancel` では常に `true`。
/// `false` は呼び出し失敗。
#[cfg(target_os = "android")]
fn call_activity(cancel: bool, id: u64) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.HealthDataHelper"),
        |env, activity, class| -> Result<bool, JniCallFailed> {
            let args = [
                jni::objects::JValue::Object(activity),
                jni::objects::JValue::Long(id as jni::sys::jlong),
            ];
            if cancel {
                env.call_static_method(
                    class,
                    jni::jni_str!("cancel"),
                    jni::jni_sig!((android.app.Activity, long) -> ()),
                    &args,
                )?;
                Ok(true)
            } else {
                let accepted = env.call_static_method(
                    class,
                    jni::jni_str!("read"),
                    jni::jni_sig!((android.app.Activity, long) -> boolean),
                    &args,
                )?;
                Ok(accepted.z()?)
            }
        },
    )
    .unwrap_or(false)
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_HealthDataHelper_nativeOnHealthDataResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    steps: jni::sys::jlong,
) {
    SLOT.deliver(
        request_id as u64,
        crate::contract::decode_code(code, steps as f64),
    );
}

/// Android の `HealthData` 実装を返す（`driver.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn HealthData> {
    Arc::new(AndroidHealthData)
}

/// `lib.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn HealthData> {
    Arc::new(crate::contract::UnsupportedHealthData)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);

    #[test]
    fn non_android_stub_is_not_a_fake_success() {
        assert_eq!(
            implementation().read_today_steps(crate::contract::READ_TIMEOUT),
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
