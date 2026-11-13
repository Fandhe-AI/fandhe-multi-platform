//! Android 向けアプリの前面・背面の通知実装（Issue #895、親 #178。共通コアと macOS / Windows は #893）。
//! `driver.rs::platform_app_lifecycle` の android 経路から UI スレッドで呼ばれ、
//! `onResume` / `onPause` / `onStart` / `onStop` を Java の `AppLifecycleHelper` 経由で受け、
//! 今の状態を `ActivityManager.getMyMemoryState` の `importance` で読む。契約は
//! `crate::contract`。
//!
//! # 結果コード（SSOT は Java 側 `AppLifecycleHelper`）
//!
//! - `MainActivity.appLifecycleStart(long)` の戻り値: 0 = 受け付けた / 1 = 失敗。未知の値は失敗
//! - `nativeOnAppLifecycle(id, code)` の code: `event_from_apple_code` の 0〜3（Activated / Deactivated /
//!   Hidden / Shown）。未知の値は捨てる（出来事を捏造せず、失敗にもしない）
//! - `MainActivity.appLifecycleRead()` の戻り値: `current_from_apple_bits` の符号化。負・規定外は失敗
//!
//! # OS を正にする範囲
//!
//! 知らせるのは操作の対象になった・外れた、見えるようになった・見えなくなったの 4 種類。凍結・bfcache
//! 復帰を知らせる API は無く、`support()` で false にする。今の状態は覚えず、読むたびに `importance` を
//! OS へ問う（FOREGROUND = 対象かつ見える / VISIBLE = 見えるが対象でない / それ以下 = どちらでもない /
//! 想定外 = 不明）。読み取りの失敗は前面・見えているへ畳まない。タイマー・経過時間は使わない。
//!
//! # 方式: id 照合 + 作り直し中の抑止（契約表 1・2・9・11）
//!
//! 購読は `register` で得た id を Java へ渡して数え、通知は id を添えて `fire` へ流す。OS の資源は
//! 確保しないため、開始の失敗で残るのは登録表の entry だけで `unregister` で外す。停止（[`AndroidGuard`]
//! の drop）は「登録解除 → Java の id 解除」の順で、解除後に遅れて届いた通知と、Activity 再生成前の
//! 古い id の通知は登録表が捨てる（id は単調増加）。`onPause` / `onStop` では止めない（止めると背面の
//! 出来事を受けられない）。`isChangingConfigurations()` が真の間の `onPause` / `onStop` は出来事にしない。
//! `onDestroy` は Java 側が購読を空にし、停止を「失敗」として送らない（契約表 10）。実行中の失敗
//! （`Signal::Failed`）は通知元を失う経路が無く Android では起きない。ロック取得順は Java → Rust の一方向。
//!
//! 守る範囲: 1 プロセス内の受信順、読めない状態を前面・見えているへ畳まないこと、停止後と再生成前の古い
//! id の通知の破棄。守らない範囲: `importance` が通知より遅れて更新されることによる一時的な食い違い、
//! マルチウィンドウでフォーカスが無くても FOREGROUND と読まれうる定義の差、停止中の Activity の再生成、
//! 画面を開く前・止めた後の出来事、複数プロセス。id・例外の内容・OS のコードはログ・エラーに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `appLifecycleStart` を経た後でしか呼ばれず、
//! feature `device` が無効のビルドではその経路に届かない（sleep_lock_events・midi と同じ根拠）。

use std::sync::Arc;

use crate::contract::AppLifecycle;
#[cfg(not(target_os = "android"))]
use crate::contract::UnsupportedAppLifecycle;
#[cfg(target_os = "android")]
use crate::contract::{
    CurrentState, EventSupport, OnSignal, Signal, SubscriptionGuard, current_from_apple_bits,
    event_from_apple_code,
};
use crate::error::AppLifecycleError;

/// `appLifecycleStart` の成功コード（SSOT は Java 側 `AppLifecycleHelper.START_OK`）。
const START_OK: i32 = 0;

/// 開始の結果コードを写像する。未知の値は失敗で、成功へは倒さない。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_start(code: i32) -> Result<(), AppLifecycleError> {
    if code == START_OK {
        Ok(())
    } else {
        Err(AppLifecycleError::Internal("app_lifecycle start"))
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
        // 先に登録を外し、以後の遅れた通知を捨てさせてから Java の id を外す。
        crate::driver::unregister(self.id);
        // Java の onDestroy が先に片付けている場合の失敗は無視してよい（冪等）。
        let _ = call_activity(Call::Stop(self.id));
    }
}

#[cfg(target_os = "android")]
struct AndroidAppLifecycle;

#[cfg(target_os = "android")]
impl AppLifecycle for AndroidAppLifecycle {
    fn support(&self) -> EventSupport {
        // 順序は LifecycleEvent::index: 対象・外れ・隠れ・見えるだけ取れる。凍結 3 種は取れない。
        EventSupport {
            events: [true, true, true, true, false, false, false],
            active: true,
            visible: true,
        }
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, AppLifecycleError> {
        let id = crate::driver::register(on_signal);
        let result = match call_activity(Call::Start(id)) {
            Some(code) => decode_start(code),
            None => Err(AppLifecycleError::Internal("app_lifecycle start")),
        };
        if let Err(e) = result {
            crate::driver::unregister(id);
            return Err(e);
        }
        Ok(Box::new(AndroidGuard { id }))
    }

    fn read_current(&self) -> Result<CurrentState, AppLifecycleError> {
        // 起動直後の一時状態を恒久的な「非対応」にしないため、Unsupported にはしない。
        match call_activity(Call::Read) {
            Some(raw) => current_from_apple_bits(raw),
            None => Err(AppLifecycleError::Internal("app_lifecycle read")),
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

/// `MainActivity.appLifecycleStart(long)`（戻り値は結果コード）/ `appLifecycleStop(long)` /
/// `appLifecycleRead()`（戻り値は状態）を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_sleep_lock_events.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
fn call_activity(call: Call) -> Option<i32> {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let app = android_app()?;

    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.AppLifecycleHelper"),
        |env, activity, class| -> Result<i32, JniCallFailed> {
            match call {
                Call::Start(id) => Ok(env
                    .call_static_method(
                        class,
                        jni::jni_str!("start"),
                        jni::jni_sig!((android.app.Activity, long) -> int),
                        &[
                            jni::objects::JValue::Object(activity),
                            jni::objects::JValue::Long(id as jni::sys::jlong),
                        ],
                    )?
                    .i()?),
                Call::Stop(id) => {
                    env.call_static_method(
                        class,
                        jni::jni_str!("stop"),
                        jni::jni_sig!((android.app.Activity, long) -> ()),
                        &[
                            jni::objects::JValue::Object(activity),
                            jni::objects::JValue::Long(id as jni::sys::jlong),
                        ],
                    )?;
                    Ok(0)
                }
                Call::Read => Ok(env
                    .call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.app.Activity) -> int),
                        &[jni::objects::JValue::Object(activity)],
                    )?
                    .i()?),
            }
        },
    )
    .ok()
}

/// `AppLifecycleHelper.nativeOnAppLifecycle`（Java 側）から呼ばれる JNI エクスポート。
/// 未知のコードと登録表に無い id（停止後・再生成前の古い id）の通知は捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_AppLifecycleHelper_nativeOnAppLifecycle<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    if let Some(event) = event_from_apple_code(code) {
        crate::driver::fire(id as u64, Signal::Event(event));
    }
}

/// Android の `AppLifecycle` 実装を返す（`driver.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn AppLifecycle> {
    Arc::new(AndroidAppLifecycle)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn AppLifecycle> {
    Arc::new(UnsupportedAppLifecycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_implementation_is_unsupported() {
        let imp = implementation();
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(AppLifecycleError::Unsupported("app_lifecycle"))
        ));
        assert!(matches!(
            imp.read_current(),
            Err(AppLifecycleError::Unsupported("app_lifecycle"))
        ));
    }

    #[test]
    fn decode_start_maps_codes() {
        assert!(decode_start(0).is_ok());
        assert!(matches!(
            decode_start(1),
            Err(AppLifecycleError::Internal("app_lifecycle start"))
        ));
        assert!(decode_start(-1).is_err());
        assert!(decode_start(99).is_err());
    }
}
