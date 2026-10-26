//! Android 向けメモリ逼迫の通知実装（Issue #913、親 #179。共通コアと macOS / Windows は #911）。
//! `device/memory_pressure.rs::platform_memory_pressure` の android 経路から UI スレッドで呼ばれ、
//! `onTrimMemory(level)` を Java の `MemoryPressureHelper` 経由で受け、今の状態を
//! `ActivityManager.MemoryInfo.lowMemory` で読む。契約は `fandhe_device::memory_pressure`。
//!
//! # 結果コード（SSOT は Java 側 `MemoryPressureHelper`）
//!
//! - `MainActivity.memoryPressureStart(long)` の戻り値: 0 = 受け付けた / 1 = 失敗。未知の値は失敗
//! - `nativeOnMemoryTrim(id, level)` の level: `onTrimMemory` の値をそのまま `PressureEvent::Trim` に持つ
//!   （段階は今後増えうるため判定せず、未知値も捨てない）
//! - `MainActivity.memoryPressureRead()` の戻り値: 1 = Low / 0 = NotLow / それ以外は失敗
//!
//! # OS を正にする範囲
//!
//! 知らせるのは trim の 1 種類だけで、メモリ警告・段階・Low / High・CPU は `support()` で false にする。
//! `onLowMemory` は API 34 以上で呼ばれず対応する種類も無いため使わない。今の状態は覚えず、読むたびに
//! `lowMemory` を OS へ問う。読み取りの失敗は「不足ではない」へ畳まない。タイマーは使わない。
//!
//! # 方式: id 照合 + onDestroy での解除（契約表 1・2・9・11）
//!
//! 購読は `register` で得た id を Java へ渡して数え、通知は id を添えて `fire` へ流す。Application Context
//! への登録は Activity より長生きするため、停止（`AndroidGuard` の drop）は「登録表から外す → Java の解除」
//! の順で、Java の `onDestroy` も必ず解除する。解除後に遅れて届いた通知と Activity 再生成前の古い id の
//! 通知は登録表が捨てる（id は単調増加）。`Signal::Failed` は通知元を失う経路が無く Android では使わない。
//! ロック取得順は Java → Rust の一方向。
//!
//! 守る範囲: 1 プロセス内の受信順、読めない状態を「不足ではない」へ畳まないこと、停止後と再生成前の古い
//! id の通知の破棄、登録の解放。守らない範囲: 画面を開く前・止めた後・背景で停止中の合図、OS 間の基準差、
//! システム全体の値であること、API 34 以上は 2 段階しか届かないこと、複数プロセス。id・例外の内容・
//! OS のコードはログ・エラーに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `memoryPressureStart` を経た後でしか呼ばれず、
//! feature `device` が無効のビルドではその経路に届かない（app_lifecycle・sleep_lock_events と同じ根拠）。

use std::sync::Arc;

#[cfg(not(target_os = "android"))]
use fandhe_device::memory_pressure::UnsupportedMemoryPressure;
#[cfg(target_os = "android")]
use fandhe_device::memory_pressure::{
    EventSupport, OnSignal, PressureEvent, Signal, SubscriptionGuard,
};
use fandhe_device::memory_pressure::{MemoryLevel, MemoryPressure};
use fandhe_device::DeviceError;

/// `memoryPressureStart` の成功コード（SSOT は Java 側 `MemoryPressureHelper.START_OK`）。
const START_OK: i32 = 0;

/// 開始の結果コードを写像する。未知の値は失敗で、成功へは倒さない。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_start(code: i32) -> Result<(), DeviceError> {
    if code == START_OK {
        Ok(())
    } else {
        Err(DeviceError::Internal("memory_pressure start"))
    }
}

/// 読み取りの結果コードを写像する。負・規定外は失敗で、「不足ではない」へは畳まない。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(code: i32) -> Result<Option<MemoryLevel>, DeviceError> {
    match code {
        1 => Ok(Some(MemoryLevel::Low)),
        0 => Ok(Some(MemoryLevel::NotLow)),
        _ => Err(DeviceError::Internal("memory_pressure read")),
    }
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
        // 先に登録を外し、以後の遅れた通知を捨てさせてから Java の登録を解除する。
        super::memory_pressure::unregister(self.id);
        // Java の onDestroy が先に片付けている場合の失敗は無視してよい（冪等）。
        let _ = call_activity(Call::Stop(self.id));
    }
}

#[cfg(target_os = "android")]
struct AndroidMemoryPressure;

#[cfg(target_os = "android")]
impl MemoryPressure for AndroidMemoryPressure {
    fn support(&self) -> EventSupport {
        // 順序は PressureEvent::index: trim だけ取れる。今の状態は lowMemory で読める。
        let mut events = [false; 11];
        events[PressureEvent::Trim(0).index()] = true;
        EventSupport {
            events,
            current: true,
        }
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        let id = super::memory_pressure::register(on_signal);
        let result = match call_activity(Call::Start(id)) {
            Some(code) => decode_start(code),
            None => Err(DeviceError::Internal("memory_pressure start")),
        };
        if let Err(e) = result {
            super::memory_pressure::unregister(id);
            return Err(e);
        }
        Ok(Box::new(AndroidGuard { id }))
    }

    fn read_current(&self) -> Result<Option<MemoryLevel>, DeviceError> {
        // 起動直後の一時状態を恒久的な「非対応」にしないため、Unsupported にはしない。
        match call_activity(Call::Read) {
            Some(raw) => decode_read(raw),
            None => Err(DeviceError::Internal("memory_pressure read")),
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

/// `MainActivity.memoryPressureStart(long)`（戻り値は結果コード）/ `memoryPressureStop(long)` /
/// `memoryPressureRead()`（戻り値は状態）を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_app_lifecycle.rs::call_activity` と同一の根拠。
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
                    jni::jni_str!("memoryPressureStart"),
                    jni::jni_sig!((long) -> int),
                    &[jni::objects::JValue::Long(id as jni::sys::jlong)],
                )?
                .i()?),
            Call::Stop(id) => {
                env.call_method(
                    &activity,
                    jni::jni_str!("memoryPressureStop"),
                    jni::jni_sig!((long) -> ()),
                    &[jni::objects::JValue::Long(id as jni::sys::jlong)],
                )?;
                Ok(0)
            }
            Call::Read => Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("memoryPressureRead"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?),
        }
    })
    .ok()
}

/// `MemoryPressureHelper.nativeOnMemoryTrim`（Java 側）から呼ばれる JNI エクスポート。
/// level は値のまま流す。登録表に無い id（停止後・再生成前の古い id）の通知は `fire` が捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MemoryPressureHelper_nativeOnMemoryTrim<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    id: jni::sys::jlong,
    level: jni::sys::jint,
) {
    super::memory_pressure::fire(id as u64, Signal::Event(PressureEvent::Trim(level)));
}

/// Android の `MemoryPressure` 実装を返す（`device/memory_pressure.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn MemoryPressure> {
    Arc::new(AndroidMemoryPressure)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn MemoryPressure> {
    Arc::new(UnsupportedMemoryPressure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_implementation_is_unsupported() {
        let imp = implementation();
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("memory_pressure"))
        ));
        assert!(matches!(
            imp.read_current(),
            Err(DeviceError::Unsupported("memory_pressure"))
        ));
    }

    #[test]
    fn decode_start_maps_codes() {
        assert!(decode_start(0).is_ok());
        assert!(decode_start(1).is_err());
        assert!(decode_start(-1).is_err());
        assert!(decode_start(99).is_err());
    }

    #[test]
    fn decode_read_maps_codes() {
        assert_eq!(decode_read(1).unwrap(), Some(MemoryLevel::Low));
        assert_eq!(decode_read(0).unwrap(), Some(MemoryLevel::NotLow));
        for bad in [-1, 2, 99] {
            assert!(matches!(
                decode_read(bad),
                Err(DeviceError::Internal("memory_pressure read"))
            ));
        }
    }
}
