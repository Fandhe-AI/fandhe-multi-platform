//! Android 向けスリープ・ロックの通知実装（Issue #811、親 #167。共通コアと macOS / Windows は #810）。
//! `device/sleep_lock_events.rs::platform_sleep_lock_events` の android 経路から UI スレッドで呼ばれ、
//! 画面の消灯・点灯と解除を Java の `SleepLockEventsHelper`（`BroadcastReceiver`）経由で受け、
//! 今の状態を `PowerManager.isInteractive` / `KeyguardManager.isKeyguardLocked` で読む。契約は
//! `fandhe_device::sleep_lock_events`。
//!
//! # 結果コード（SSOT は Java 側 `SleepLockEventsHelper`）
//!
//! - `MainActivity.sleepLockEventsStart(long)` の戻り値: 0 = 受け付けた / 1 = 失敗。未知の値は失敗
//! - `nativeOnSleepLockEvent(id, code)` の code: 0 = `ACTION_SCREEN_OFF` / 1 = `ACTION_SCREEN_ON` /
//!   2 = `ACTION_USER_PRESENT`。未知の値は捨てる（出来事を捏造せず、失敗にもしない）
//! - `MainActivity.sleepLockEventsRead()` の戻り値: 負 = 失敗。bit 0-1 が画面（0 = 消灯 / 1 = 点灯 /
//!   2 = 不明）、bit 2-3 がロック（0 = 解除 / 1 = ロック中 / 2 = 不明）。規定外の値は失敗
//!
//! # OS を正にする範囲
//!
//! 知らせるのは画面の消灯・点灯と解除。システムのスリープ・復帰とロックした瞬間を知らせる
//! ブロードキャストは無く、`support()` で false にする。今の状態は覚えず読むたびに OS へ問う。
//! 読み取りの失敗・サービス不在は「ロックされていない」「点灯」へ畳まない。タイマー・経過時間・
//! 時刻は使わない。
//!
//! # 方式: 動的登録 + id 照合 + 解除の集約（契約表 1・2・9・11）
//!
//! Java は Application Context へ `registerReceiver` する（マニフェストに宣言しても
//! `SCREEN_OFF` / `ON` は届かない。システムだけが送るブロードキャストなので export フラグは付けない）。
//! 購読は `register` で得た id を Java へ渡して数え、通知は id を添えて `fire` へ流す。
//! 停止（[`AndroidGuard`] の drop）は「登録解除 → Java のレシーバー解除」の順で、解除後に
//! 遅れて届いた通知と、Activity 再生成前の古い id の通知は登録表が捨てる（id は単調増加）。
//! 開始は同期で、失敗したら登録表に何も残さない。`onPause` では止めない（画面を消すと Activity が
//! `onPause` し、止めると消灯を受けられない）。`onDestroy` は Java 側がレシーバーを解除し、
//! 停止を「失敗」として送らない（正常終了と失敗を混ぜない。契約表 10）。ロック取得順は
//! Java → Rust の一方向（Rust は登録表のロックを手放してから Java を呼ぶ）。
//!
//! 守る範囲: 1 プロセス内の受信順、失敗・非対応を成功へ畳まないこと、全終了経路でのレシーバー解除、
//! 解除後の通知の破棄。守らない範囲: Android 14 以上でキャッシュ状態の間に溜められた通知の
//! 順序と畳み込み、鍵なしのキーガード表示でもロック中と読む OS の定義、画面を開く前・止めた後の
//! 出来事、複数プロセス、OS 側の遅れ。id・例外の内容・OS のコードはログ・エラーに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `sleepLockEventsStart` を経た後でしか呼ばれず、
//! feature `device` が無効のビルドではその経路に届かない（midi・call_ui と同じ根拠）。

use std::sync::Arc;

#[cfg(not(target_os = "android"))]
use fandhe_device::sleep_lock_events::UnsupportedSleepLockEvents;
use fandhe_device::sleep_lock_events::{
    CurrentState, LockState, ScreenState, SleepLockEvent, SleepLockEvents,
};
#[cfg(target_os = "android")]
use fandhe_device::sleep_lock_events::{EventSupport, OnSignal, Signal, SubscriptionGuard};
use fandhe_device::DeviceError;

/// `sleepLockEventsStart` の成功コード（SSOT は Java 側 `SleepLockEventsHelper.START_OK`）。
const START_OK: i32 = 0;

/// 開始の結果コードを写像する。未知の値は失敗で、成功へは倒さない。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_start(code: i32) -> Result<(), DeviceError> {
    if code == START_OK {
        Ok(())
    } else {
        Err(DeviceError::Internal("sleep_lock_events start"))
    }
}

/// 出来事コード（SSOT は `SleepLockEventsHelper.EVENT_*`）を契約の出来事へ写像する。
/// 未知の値は `None`（捨てる）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_event(code: i32) -> Option<SleepLockEvent> {
    match code {
        0 => Some(SleepLockEvent::ScreenOff),
        1 => Some(SleepLockEvent::ScreenOn),
        2 => Some(SleepLockEvent::Unlocked),
        _ => None,
    }
}

/// `sleepLockEventsRead` の戻り値を今の状態へ写像する。負の値・規定外のフィールドは失敗。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_current(raw: i32) -> Result<CurrentState, DeviceError> {
    let fail = DeviceError::Internal("sleep_lock_events read");
    if raw < 0 {
        return Err(fail);
    }
    let screen = match raw & 0b11 {
        0 => ScreenState::Off,
        1 => ScreenState::On,
        2 => ScreenState::Unknown,
        _ => return Err(fail),
    };
    let lock = match (raw >> 2) & 0b11 {
        0 => LockState::Unlocked,
        1 => LockState::Locked,
        2 => LockState::Unknown,
        _ => return Err(fail),
    };
    Ok(CurrentState {
        lock: Some(lock),
        screen: Some(screen),
    })
}

/// 実行中の購読。drop で停止する。
#[cfg(target_os = "android")]
struct AndroidGuard {
    id: u64,
}

#[cfg(target_os = "android")]
impl SubscriptionGuard for AndroidGuard {}

#[cfg(target_os = "android")]
impl Drop for AndroidGuard {
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れた通知を捨てさせてから Java のレシーバーを外す。
        super::sleep_lock_events::unregister(self.id);
        // Java の onDestroy が先に片付けている場合の失敗は無視してよい（冪等）。
        let _ = call_activity(Call::Stop(self.id));
    }
}

#[cfg(target_os = "android")]
struct AndroidSleepLockEvents;

#[cfg(target_os = "android")]
impl SleepLockEvents for AndroidSleepLockEvents {
    fn support(&self) -> EventSupport {
        // 順序は ALL_EVENTS: 消灯・点灯・解除だけ取れる。スリープ・復帰・ロックした瞬間は無い。
        EventSupport {
            events: [false, false, true, true, false, true],
            lock: true,
            screen: true,
        }
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        let id = super::sleep_lock_events::register(on_signal);
        let result = match call_activity(Call::Start(id)) {
            Some(code) => decode_start(code),
            None => Err(DeviceError::Internal("sleep_lock_events start")),
        };
        if let Err(e) = result {
            super::sleep_lock_events::unregister(id);
            return Err(e);
        }
        Ok(Box::new(AndroidGuard { id }))
    }

    fn read_current(&self) -> Result<CurrentState, DeviceError> {
        // 起動直後の一時状態を恒久的な「非対応」にしないため、Unsupported にはしない。
        match call_activity(Call::Read) {
            Some(raw) => decode_current(raw),
            None => Err(DeviceError::Internal("sleep_lock_events read")),
        }
    }
}

/// Java へ渡す呼び出しの種類。
#[cfg(target_os = "android")]
enum Call {
    Start(u64),
    Stop(u64),
    Read,
}

/// `MainActivity.sleepLockEventsStart(long)`（戻り値は結果コード）/ `sleepLockEventsStop(long)` /
/// `sleepLockEventsRead()`（戻り値は状態）を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_midi.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(call: Call) -> Option<i32> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        match call {
            Call::Start(id) => Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("sleepLockEventsStart"),
                    jni::jni_sig!((long) -> int),
                    &[jni::objects::JValue::Long(id as jni::sys::jlong)],
                )?
                .i()?),
            Call::Stop(id) => {
                env.call_method(
                    &activity,
                    jni::jni_str!("sleepLockEventsStop"),
                    jni::jni_sig!((long) -> ()),
                    &[jni::objects::JValue::Long(id as jni::sys::jlong)],
                )?;
                Ok(0)
            }
            Call::Read => Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("sleepLockEventsRead"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?),
        }
    })
    .ok()
}

/// `SleepLockEventsHelper.nativeOnSleepLockEvent`（Java 側）から呼ばれる JNI エクスポート。
/// 未知のコードと登録表に無い id（停止後・再生成前の古い id）の通知は捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_SleepLockEventsHelper_nativeOnSleepLockEvent<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    if let Some(event) = decode_event(code) {
        super::sleep_lock_events::fire(id as u64, Signal::Event(event));
    }
}

/// Android の `SleepLockEvents` 実装を返す（`device/sleep_lock_events.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn SleepLockEvents> {
    Arc::new(AndroidSleepLockEvents)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn SleepLockEvents> {
    Arc::new(UnsupportedSleepLockEvents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_implementation_is_unsupported() {
        let imp = implementation();
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("sleep_lock_events"))
        ));
        assert!(matches!(
            imp.read_current(),
            Err(DeviceError::Unsupported("sleep_lock_events"))
        ));
    }

    #[test]
    fn decode_start_maps_codes() {
        assert!(decode_start(0).is_ok());
        assert!(matches!(
            decode_start(1),
            Err(DeviceError::Internal("sleep_lock_events start"))
        ));
        assert!(decode_start(-1).is_err());
        assert!(decode_start(99).is_err());
    }

    #[test]
    fn decode_event_maps_codes_and_drops_unknown() {
        assert_eq!(decode_event(0), Some(SleepLockEvent::ScreenOff));
        assert_eq!(decode_event(1), Some(SleepLockEvent::ScreenOn));
        assert_eq!(decode_event(2), Some(SleepLockEvent::Unlocked));
        assert_eq!(decode_event(3), None);
        assert_eq!(decode_event(-1), None);
    }

    #[test]
    fn decode_current_maps_fields() {
        let c = decode_current(1).unwrap();
        assert_eq!(c.screen, Some(ScreenState::On));
        assert_eq!(c.lock, Some(LockState::Unlocked));
        let c = decode_current(1 << 2).unwrap();
        assert_eq!(c.screen, Some(ScreenState::Off));
        assert_eq!(c.lock, Some(LockState::Locked));
        let c = decode_current(2 | (2 << 2)).unwrap();
        assert_eq!(c.screen, Some(ScreenState::Unknown));
        assert_eq!(c.lock, Some(LockState::Unknown));
    }

    #[test]
    fn decode_current_rejects_negative_and_out_of_range() {
        assert!(decode_current(-1).is_err());
        assert!(decode_current(3).is_err());
        assert!(decode_current(3 << 2).is_err());
        assert!(decode_current(1 | (3 << 2)).is_err());
    }
}
