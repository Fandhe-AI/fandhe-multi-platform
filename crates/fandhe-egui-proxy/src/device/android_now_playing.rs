//! Android 向けロック画面・OS のメディア操作実装（Issue #383、親 #98）。
//! `device/now_playing.rs::platform_now_playing` の Android 経路から呼ばれ、契約は
//! `fandhe_device::now_playing`。Java 側の SSOT は `NowPlayingHelper`（コード表・UI スレッドへの
//! 橋渡し・`MediaSession` と通知）で、egui スレッドから `MainActivity.nowPlayingPublish` /
//! `nowPlayingClear` を JNI で同期呼び出しする。
//!
//! OS からの再生・一時停止は `NowPlayingHelper.nativeOnNowPlayingCommand` で届き、
//! [`device::now_playing::commands`](super::now_playing::commands) へ `push` するだけにして、
//! 続けて `request_repaint` で次フレームを起こす（Android は 250ms ポーリングを使わない）。
//! メディアキーのトグルは `MediaSession` の既定処理が Play / Pause に振り分けるため `Toggle` は来ない。
//!
//! 同期呼び出し: 待ちは Java 側で最長 1 秒。期限切れは `Internal`（未開始なら UI 側は何も変えない）。
//! `Unsupported` / `PermissionDenied` は返さない（API 21+ の機能で minSdk は 24、権限不要）。
//!
//! Activity 再生成: Java の `onDestroy` が session を解放し、新インスタンスの
//! `reset_pending_tracking` が `disarm` → `clear` する。旧インスタンスの操作は新しい再生に効かない。
//!
//! 守る範囲: 1 プロセス内の操作順、取り下げ後の古い操作で再開しないこと、嘘の成功の禁止、
//! session と通知の解放。守らない範囲: 通知を消した後の再表示、通知を切った端末でのシェード表示、
//! API 24〜32 の通知ボタン、背景中の操作、UI スレッドが塞がっている間の取り下げの遅れ、複数プロセス。
//!
//! no-op スタブは置かない: Java の native は Rust の `publish` で session を作った後でしか呼ばれない。

use std::sync::Arc;

use fandhe_device::now_playing::{NowPlaying, OsMediaCommand};
use fandhe_device::DeviceError;

/// 操作コード（SSOT は Java 側 `NowPlayingHelper.CMD_*`）。
const CMD_PLAY: i32 = 0;
const CMD_PAUSE: i32 = 1;

/// 操作を受け入れる session id と、publish の結果を受け取る前に届いた失敗通知の id。
/// 判定と更新は必ず同じロックの内側で行う（結果の受理と失敗通知の順序を一意にする）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
#[derive(Default)]
struct SessionState {
    /// 操作を受け入れる session id（0 = なし）。取り下げ・失敗で 0 に戻す。
    accepted: i64,
    /// 受け入れ前（publish の戻りを処理する前）に失敗通知が届いた id。受理時に照合して捨てる。
    early_failed: Vec<i64>,
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
impl SessionState {
    /// publish の戻りを受理する。失敗通知が先に届いていた id は受け入れず `false`。
    fn accept(&mut self, id: i64) -> bool {
        let failed = self.early_failed.contains(&id);
        self.early_failed.clear();
        self.accepted = if failed { 0 } else { id };
        !failed
    }

    /// 失敗通知を受ける。現在の session なら受付を止めて `true`、未受理の id は覚えて `false`。
    fn fail(&mut self, id: i64) -> bool {
        if id == self.accepted {
            self.accepted = 0;
            true
        } else {
            // 古い世代の id も入るが、受理時の照合と clear でしか使わない。
            if self.early_failed.len() >= 8 {
                self.early_failed.remove(0);
            }
            self.early_failed.push(id);
            false
        }
    }
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
static STATE: std::sync::Mutex<SessionState> = std::sync::Mutex::new(SessionState {
    accepted: 0,
    early_failed: Vec::new(),
});

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn state() -> std::sync::MutexGuard<'static, SessionState> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// publish の戻り値（SSOT は Java 側 `NowPlayingHelper`）を結果へ写す。負（失敗・期限切れ）と
/// 0（id なし）は `Internal`、正は成功で受け入れる id を更新する。受理時に、この publish の間に届いた
/// 失敗通知（受け取り前の同 id、および解放済みの古い session の通知）を整理する。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_result(code: i64) -> Result<(), DeviceError> {
    let mut st = state();
    if code > 0 && st.accept(code) {
        // 解放済みの古い session の失敗通知が立てた印は、新しい session に持ち越さない。
        super::now_playing::clear_os_failure();
        Ok(())
    } else {
        st.accepted = 0;
        Err(DeviceError::Internal("now_playing"))
    }
}

/// 操作コードを契約の操作へ写す純関数。未知の値は捨てる。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_command(code: i32) -> Option<OsMediaCommand> {
    match code {
        CMD_PLAY => Some(OsMediaCommand::Play),
        CMD_PAUSE => Some(OsMediaCommand::Pause),
        _ => None,
    }
}

/// 位置・長さを JNI の `long` へ。未取得と `i64` の溢れは `-1`（値域の検証は
/// `NowPlayingInfo::new` が変換前に済ませている）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn to_jni_ms(v: Option<u64>) -> i64 {
    v.and_then(|x| i64::try_from(x).ok()).unwrap_or(-1)
}

#[cfg(target_os = "android")]
struct AndroidNowPlaying;

/// `MainActivity.nowPlayingPublish(String, boolean, long, long)`（`Some`）または
/// `nowPlayingClear()`（`None`）を JNI で呼ぶ。`publish` の戻りは session id（負は失敗）、`clear` は 0。
/// `None` は呼び出し失敗。`unsafe` 2 箇所は `android_screen_wake_lock.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(publish: Option<(bool, i64, i64)>) -> Option<i64> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i64, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        match publish {
            Some((playing, position, duration)) => {
                let title = env.new_string(fandhe_device::now_playing::DEMO_TITLE)?;
                Ok(env
                    .call_method(
                        &activity,
                        jni::jni_str!("nowPlayingPublish"),
                        jni::jni_sig!((java.lang.String, boolean, long, long) -> long),
                        &[
                            jni::objects::JValue::Object(&title),
                            jni::objects::JValue::Bool(playing),
                            jni::objects::JValue::Long(position),
                            jni::objects::JValue::Long(duration),
                        ],
                    )?
                    .j()?)
            }
            None => {
                env.call_method(
                    &activity,
                    jni::jni_str!("nowPlayingClear"),
                    jni::jni_sig!(() -> ()),
                    &[],
                )?;
                Ok(0)
            }
        }
    })
    .ok()
}

#[cfg(target_os = "android")]
impl NowPlaying for AndroidNowPlaying {
    fn publish(&self, info: fandhe_device::now_playing::NowPlayingInfo) -> Result<(), DeviceError> {
        match call_activity(Some((
            info.playing,
            to_jni_ms(info.position_ms),
            to_jni_ms(info.duration_ms),
        ))) {
            Some(code) => decode_result(code),
            None => Err(DeviceError::Internal("now_playing")),
        }
    }

    fn clear(&self) {
        // 先に受け入れ id を捨てる（Java の解放前に届く遅延した操作も止める）。
        state().accepted = 0;
        // 冪等。失敗は無視する（Java 側は Activity 破棄時に自分で解放する）。
        let _ = call_activity(None);
    }

    fn shutdown(&self) {
        state().accepted = 0;
        let _ = call_activity(None);
    }
}

/// 現在の OS 向け実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn NowPlaying> {
    Arc::new(AndroidNowPlaying)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn NowPlaying> {
    Arc::new(fandhe_device::now_playing::UnsupportedNowPlaying)
}

/// `NowPlayingHelper.nativeOnNowPlayingCommand`（Java 側）から呼ばれる JNI エクスポート。
/// 操作を共有状態へ置くだけで、画面への反映は driver が毎フレーム行う。値はログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_NowPlayingHelper_nativeOnNowPlayingCommand<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    code: jni::sys::jint,
    session_id: jni::sys::jlong,
) {
    if session_id != state().accepted {
        return;
    }
    if let Some(cmd) = decode_command(code) {
        super::now_playing::commands().push(cmd);
        // 入力が無いと egui が描画せず操作が画面へ届かないため次フレームを起こす。
        crate::android::request_repaint();
    }
}

/// `NowPlayingHelper.nativeOnNowPlayingFailed`（Java 側）から呼ばれる JNI エクスポート。publish 成功後の
/// onPause / onResume で session を失ったとき、現在の session の通知だけ受け、操作の受付を止めて
/// ドライバへ失敗を伝える（画面は次フレームで `Failed` になる）。根拠は上の JNI エクスポートと同じ。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` の根拠は上と同じ。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_NowPlayingHelper_nativeOnNowPlayingFailed<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    session_id: jni::sys::jlong,
) {
    // 判定と印は同じロックの内側（decode_result の受理と順序付ける）。
    let mut st = state();
    if st.fail(session_id) {
        super::now_playing::mark_os_failure();
        drop(st);
        crate::android::request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_rejects_id_whose_failure_arrived_first() {
        let mut st = SessionState::default();
        assert!(!st.fail(7)); // 受理前の失敗通知
        assert!(!st.accept(7));
        assert_eq!(st.accepted, 0);
        assert!(st.accept(8)); // 別 id は通常どおり
        assert_eq!(st.accepted, 8);
        assert!(!st.fail(7)); // 古い世代の通知は無視
        assert_eq!(st.accepted, 8);
        assert!(st.fail(8));
        assert_eq!(st.accepted, 0);
    }

    #[test]
    fn decode_command_maps_known_codes_only() {
        assert_eq!(decode_command(0), Some(OsMediaCommand::Play));
        assert_eq!(decode_command(1), Some(OsMediaCommand::Pause));
        assert_eq!(decode_command(2), None);
        assert_eq!(decode_command(-1), None);
    }

    #[test]
    fn to_jni_ms_uses_minus_one_for_missing_or_overflow() {
        assert_eq!(to_jni_ms(None), -1);
        assert_eq!(to_jni_ms(Some(5)), 5);
        assert_eq!(to_jni_ms(Some(u64::MAX)), -1);
    }
}
