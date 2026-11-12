//! Android 向け歩数実装（Issue #1249、親 #1119）。`driver.rs::platform_pedometer` の
//! Android 経路から呼ばれ、`Sensor.TYPE_STEP_COUNTER`（再起動後の累積歩数）を 1 回だけ
//! 期限付きで取得する。Java 側は `PedometerHelper`（許可確認・リスナー登録・解除）。
//!
//! 流れ: `Pedometer::read_steps`（ワーカースレッド）→ [`RequestSlot::begin`] で id 採番 →
//! JNI `PedometerHelper.read(activity, id)`（static） → Java が結果コードを
//! `nativeOnPedometerResult` で返す → [`decode`] で契約の失敗種別へ写像 → ワーカーが
//! 100ms 周期で slot を見て受け取る。期限（`timeout`）は契約の `READ_TIMEOUT` 1 つだけで、
//! 切れたら `PedometerHelper.cancel(activity, id)` でリスナーを解除し `NoReading`（0 歩にしない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・0 歩と未取得の区別・非搭載/拒否/破棄を成功に
//! 畳まないこと・リスナーの解除。守らない範囲: センサー遅延による値の鮮度、許可ダイアログ
//! への応答に時間がかかり期限を超えた場合の「未取得」表示。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestPedometerRead` を経た後でしか
//! 呼ばれず、feature `device` 無効ビルドではその経路に到達しない（mic-record / BLE と同じ）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::contract::{Pedometer, PedometerFailure, total_steps};

use fandhe_runtime::request_slot::{POLL_INTERVAL, RequestSlot};

/// 結果コード（SSOT は Java 側 `PedometerHelper.RESULT_*`）。
const RESULT_OK: i32 = 0;
const RESULT_UNSUPPORTED: i32 = 1;
const RESULT_PERMISSION_DENIED: i32 = 2;
/// 値が届く前に Activity が破棄された（再生成を含む）。
const RESULT_INTERRUPTED: i32 = 3;

type Outcome = Result<u64, PedometerFailure>;

/// Java の結果コードと歩数を契約の結果へ写像する。未知のコードと負値は `Failed`。
fn decode(code: i32, steps: i64) -> Outcome {
    match code {
        RESULT_OK => total_steps([steps]),
        RESULT_UNSUPPORTED => Err(PedometerFailure::Unsupported),
        RESULT_PERMISSION_DENIED => Err(PedometerFailure::PermissionDenied),
        RESULT_INTERRUPTED => Err(PedometerFailure::NoReading),
        _ => Err(PedometerFailure::Failed),
    }
}

/// 要求 1 件を出して期限まで結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）、
/// `cancel` は期限切れ時に OS 側のリスナーを解除させる。
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
        return Err(PedometerFailure::Failed);
    }
    loop {
        // 結果の受理より先に期限を確認する（start や再開の遅れで期限後に届いた値は捨てる）。
        let now = Instant::now();
        if now >= deadline {
            slot.clear(id);
            cancel(id);
            return Err(PedometerFailure::NoReading);
        }
        match slot.snapshot() {
            Some((cur, Some(outcome))) if cur == id => {
                slot.clear(id);
                return outcome;
            }
            Some((cur, None)) if cur == id => {}
            // 差し替え済み（他人の slot は消さない）。start 後に追い越された場合に
            // Java 側へ残るリスナーを解除させる（旧 id なら Java 側は no-op）。
            _ => {
                cancel(id);
                return Err(PedometerFailure::NoReading);
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Outcome> = RequestSlot::new();

#[cfg(target_os = "android")]
struct AndroidPedometer;

#[cfg(target_os = "android")]
impl Pedometer for AndroidPedometer {
    fn read_steps(&self, timeout: Duration) -> Outcome {
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

/// `PedometerHelper.read(Activity, long)`（`cancel=false`）または `PedometerHelper.cancel(Activity, long)`
/// （`cancel=true`）を JNI で呼ぶ。`false` は呼び出し失敗。クラスは Activity の class loader で解決する
/// （`fandhe_platform_android::with_activity_class`）。unsafe は共通層の中に閉じ、ここには無い。
#[cfg(target_os = "android")]
fn call_activity(cancel: bool, id: u64) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.PedometerHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            let id = jni::sys::jlong::try_from(id).map_err(|_| JniCallFailed)?;
            if cancel {
                env.call_static_method(
                    class,
                    jni::jni_str!("cancel"),
                    jni::jni_sig!((android.app.Activity, long) -> ()),
                    &[
                        jni::objects::JValue::Object(activity),
                        jni::objects::JValue::Long(id),
                    ],
                )?;
            } else {
                env.call_static_method(
                    class,
                    jni::jni_str!("read"),
                    jni::jni_sig!((android.app.Activity, long) -> ()),
                    &[
                        jni::objects::JValue::Object(activity),
                        jni::objects::JValue::Long(id),
                    ],
                )?;
            }
            Ok(())
        },
    )
    .is_ok()
}

/// `PedometerHelper.nativeOnPedometerResult`（Java 側）から呼ばれる JNI エクスポート。
/// 歩数・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_PedometerHelper_nativeOnPedometerResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    steps: jni::sys::jlong,
) {
    SLOT.deliver(request_id as u64, decode(code, steps));
}

/// Android の `Pedometer` 実装を返す（`device/pedometer.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn Pedometer> {
    Arc::new(AndroidPedometer)
}

/// `lib.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn Pedometer> {
    Arc::new(crate::contract::UnsupportedPedometer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);

    #[test]
    fn decode_maps_codes() {
        assert_eq!(decode(0, 0), Ok(0));
        assert_eq!(decode(0, 42), Ok(42));
        assert_eq!(decode(0, -1), Err(PedometerFailure::Failed));
        assert_eq!(decode(1, 0), Err(PedometerFailure::Unsupported));
        assert_eq!(decode(2, 0), Err(PedometerFailure::PermissionDenied));
        assert_eq!(decode(3, 0), Err(PedometerFailure::NoReading));
        assert_eq!(decode(4, 0), Err(PedometerFailure::Failed));
        assert_eq!(decode(99, 5), Err(PedometerFailure::Failed));
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
                    s.deliver(id, Ok(7));
                });
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert_eq!(r, Ok(7));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_cancels_once_and_is_no_reading() {
        let slot = RequestSlot::<Outcome>::new();
        let cancelled = Cell::new(0);
        let r = read_with(
            &slot,
            Duration::from_millis(150),
            |_| true,
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(r, Err(PedometerFailure::NoReading));
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
    fn result_delivered_after_deadline_is_no_reading() {
        let slot = RequestSlot::<Outcome>::new();
        let cancelled = Cell::new(0);
        let r = read_with(
            &slot,
            Duration::from_millis(10),
            |id| {
                std::thread::sleep(Duration::from_millis(20));
                assert!(slot.deliver(id, Ok(7)));
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(r, Err(PedometerFailure::NoReading));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn start_failure_is_failed_and_clears() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(&slot, T, |_| false, |_| panic!("no cancel"));
        assert_eq!(r, Err(PedometerFailure::Failed));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn stale_id_is_ignored_until_timeout() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(
            &slot,
            Duration::from_millis(150),
            |id| {
                assert!(!slot.deliver(id + 100, Ok(1)));
                true
            },
            |_| {},
        );
        assert_eq!(r, Err(PedometerFailure::NoReading));
    }

    #[test]
    fn replaced_request_returns_immediately_and_keeps_new_slot() {
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
        assert_eq!(r, Err(PedometerFailure::NoReading));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(slot.snapshot(), Some((new_id.get(), None)));
    }
}
