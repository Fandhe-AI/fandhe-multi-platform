//! Android 向け音声セッション実装（Issue #842、親 #172）。`device/audio_session.rs` の Android 経路
//! （`platform_session`）から呼ばれ、Java の `AudioSessionHelper`（`AudioFocusRequest` と
//! `ACTION_AUDIO_BECOMING_NOISY` の動的レシーバー）を `MainActivity.audioSessionStart` / `Stop` / `Read` 経由の
//! JNI で呼ぶ。契約は `fandhe_device::audio_session` のモジュール doc（方式・結果の種類・解放・守る範囲 /
//! 守らない範囲）で、本ファイルは重複して書かない。
//!
//! # 流れとコードの SSOT
//!
//! `start` が id を採番し、**Java を呼ぶ前に**購読の登録表（`SLOT`）へ `OnSignal` を入れてから
//! `audioSessionStart(id, kind)` を呼ぶ（フォーカス要求の中で届く通知も落とさない）。Java は id を捕捉し、
//! `nativeOnAudioSessionEvent(id, code)` で届ける。受け口は id を照合して配送する。停止は登録解除 →
//! Java の `audioSessionStop(id)` の順で、`onDestroy` が先に解除済みなら Java は何もせず 0 を返す。
//! Java の呼び出し中は `SLOT` のロックを持たない。コード（start / stop / read / 出来事）の SSOT は
//! `AudioSessionHelper.java` のクラス冒頭コメントで、純関数の写像はそれと一致させる。
//!
//! 写像と読み戻しの判定は純関数で、ホストでもテストできるようモジュールは `cfg(any(android, test))`。
//!
//! # 既知の天井
//!
//! - 選べるのは StopOthers（`AUDIOFOCUS_GAIN`）と DuckOthers（`GAIN_TRANSIENT_MAY_DUCK`）。MixWithOthers は
//!   「フォーカスを要求しない」ことにあたり、別の種別へ寄せず `KindUnsupported` にする
//! - API 24・25 は `AudioFocusRequest` が無く、Java が -1 を返して `Unsupported` になる（`getMode` /
//!   `isMusicActive` の読み戻しは動く）
//! - Android 8 以降の自動ダッキングではコールバックが来ない。Android 12 以降は他アプリの GAIN で
//!   フェードアウトした後に LOSS が届く
//! - Android 15 以上で背景から要求すると FAILED になり、`Rejected` として見える
//! - 実行中の失敗（`Signal::Failed`）は出さない。今の種別・フォーカス保持を OS に問う公開 API は無い

#[cfg(any(target_os = "android", test))]
use fandhe_device::audio_session::Support;
use fandhe_device::audio_session::{
    audio_mode_from_android, CurrentState, RestoreFailed, SessionKind,
};
use fandhe_device::DeviceError;

/// Android の対応範囲。種別は 2 つ、出来事はフォーカス 4 種と経路変更の合図、モードと音楽再生中を読める。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn android_support() -> Support {
    // 順序は `EventKind::index`: 先頭 3 つ（割り込み開始 / 終了・経路変更）は iOS の概念。
    let mut events = [false; 8];
    for e in events.iter_mut().skip(3) {
        *e = true;
    }
    Support {
        kinds: [true, true, false],
        events,
        kind_readable: false,
        other_playing_readable: false,
        audio_mode_readable: true,
        music_active_readable: true,
    }
}

/// 種別 → Java の kind コード。Mix は選べない（`None`）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn java_kind(kind: SessionKind) -> Option<i32> {
    match kind {
        SessionKind::StopOthers => Some(0),
        SessionKind::DuckOthers => Some(1),
        SessionKind::MixWithOthers => None,
    }
}

/// Java の stop の戻り値を結果へ写す。0（フォーカスとレシーバーの解除が両方成功）だけを成功にする。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn stop_result(code: i32) -> Result<(), RestoreFailed> {
    if code == 0 {
        Ok(())
    } else {
        Err(RestoreFailed)
    }
}

/// Java の read の戻り値を現在の状態へ写す。範囲の検証は変換の前に行い、失敗に OS のコードは載せない。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(raw: i32) -> Result<CurrentState, DeviceError> {
    if !(0..(1 << 9)).contains(&raw) {
        return Err(DeviceError::Internal("audio_session read"));
    }
    let mode = raw & 0xFF;
    Ok(CurrentState {
        kind: None,
        other_audio_playing: None,
        // 0xFF は「不明」（Java が範囲外のモードを寄せる値）。
        audio_mode: Some(audio_mode_from_android(if mode == 0xFF {
            -1
        } else {
            mode
        })),
        music_active: Some(raw & (1 << 8) != 0),
    })
}

#[cfg(target_os = "android")]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use fandhe_device::audio_session::{
        event_from_android_code, start_result_from_android, AudioSession, AudioSessionGuard,
        OnSignal, RestoreFailed, SessionKind, Signal, StartFailure, Support,
    };
    use fandhe_device::DeviceError;

    use super::{android_support, decode_read, java_kind, stop_result, CurrentState};

    /// 購読の登録表。1 プロセスで画面は 1 つ、という守る範囲に合わせて 1 枠。
    static SLOT: Mutex<Option<(u64, OnSignal)>> = Mutex::new(None);
    /// 1 から採番する（0 は Java で「なし」）。
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    /// 枠が `id` のときだけ消す（後から入った別の購読を消さない）。
    fn clear_slot(id: u64) {
        let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|(i, _)| *i == id) {
            *slot = None;
        }
    }

    /// Java へ渡す呼び出しの種類。
    enum Call {
        Start(u64, i32),
        Stop(u64),
        Read,
    }

    /// `MainActivity.audioSessionStart(long, int)` / `audioSessionStop(long)` / `audioSessionRead()`
    /// （戻りはいずれも int）を JNI で呼ぶ。`None` は呼び出し失敗。
    /// `unsafe` 2 箇所は `android_sleep_lock_events.rs::call_activity` と同一の根拠。
    #[allow(unsafe_code)]
    fn call_activity(call: Call) -> Option<i32> {
        use crate::device::android::JniCallFailed;
        let app = crate::android::android_app()?;
        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
        // `app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let v = match call {
                Call::Start(id, kind) => env.call_method(
                    &activity,
                    jni::jni_str!("audioSessionStart"),
                    jni::jni_sig!((long, int) -> int),
                    &[
                        jni::objects::JValue::Long(id as jni::sys::jlong),
                        jni::objects::JValue::Int(kind),
                    ],
                )?,
                Call::Stop(id) => env.call_method(
                    &activity,
                    jni::jni_str!("audioSessionStop"),
                    jni::jni_sig!((long) -> int),
                    &[jni::objects::JValue::Long(id as jni::sys::jlong)],
                )?,
                Call::Read => env.call_method(
                    &activity,
                    jni::jni_str!("audioSessionRead"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?,
            };
            Ok(v.i()?)
        })
        .ok()
    }

    /// `AudioSessionHelper.nativeOnAudioSessionEvent`（Java 側）から呼ばれる JNI エクスポート。
    /// 任意のスレッドから呼ばれ、panic しない。解除後に遅れて届いた通知（id 不一致）・未知のコード
    /// （`GAIN_TRANSIENT` 系等）は捨てる。ロックの外で配送する。
    ///
    /// # Safety
    /// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
    /// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
    /// （既存の JNI エクスポートと同じ根拠）。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "system" fn Java_com_fandhe_androidapp_AudioSessionHelper_nativeOnAudioSessionEvent<
        'caller,
    >(
        _env: jni::EnvUnowned<'caller>,
        _class: jni::objects::JClass<'caller>,
        id: jni::sys::jlong,
        code: jni::sys::jint,
    ) {
        let cb = {
            let slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_ref() {
                Some((i, cb)) if *i == id as u64 => Arc::clone(cb),
                _ => return,
            }
        };
        if let Some(event) = event_from_android_code(code) {
            cb(Signal::Event(event));
        }
    }

    struct AndroidAudioSession;

    pub(crate) fn implementation() -> Arc<dyn AudioSession> {
        Arc::new(AndroidAudioSession)
    }

    impl AudioSession for AndroidAudioSession {
        fn support(&self) -> Support {
            android_support()
        }

        fn start(
            &self,
            kind: SessionKind,
            on_signal: OnSignal,
        ) -> Result<Box<dyn AudioSessionGuard>, StartFailure> {
            let Some(kind_code) = java_kind(kind) else {
                return Err(StartFailure::KindUnsupported);
            };
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            {
                // 枠が埋まっている = 有効な購読が既にある。上書きすると既存の通知が捨てられるため、
                // Java を呼ばずに失敗で返す（何も変えていないので restored: true）。
                let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
                if slot.is_some() {
                    return Err(StartFailure::Failed { restored: true });
                }
                *slot = Some((id, on_signal));
            }
            let result = match call_activity(Call::Start(id, kind_code)) {
                Some(code) => start_result_from_android(code),
                None => {
                    // 呼べたかどうかが分からないので、解除を試して戻せたかを確かめる。
                    let restored = call_activity(Call::Stop(id)) == Some(0);
                    Err(StartFailure::Failed { restored })
                }
            };
            match result {
                Ok(()) => Ok(Box::new(AndroidAudioSessionGuard { id, done: false })),
                Err(e) => {
                    clear_slot(id);
                    Err(e)
                }
            }
        }

        fn read_current(&self) -> Result<CurrentState, DeviceError> {
            // 起動直後の一時状態を恒久的な「非対応」にしないため、Unsupported にはしない。
            match call_activity(Call::Read) {
                Some(raw) => decode_read(raw),
                None => Err(DeviceError::Internal("audio_session read")),
            }
        }
    }

    struct AndroidAudioSessionGuard {
        id: u64,
        done: bool,
    }

    impl AndroidAudioSessionGuard {
        /// 登録解除 → Java のフォーカス・レシーバー解除。一度だけ行う。JNI の失敗は解除失敗として見せる。
        fn teardown(&mut self) -> Result<(), RestoreFailed> {
            self.done = true;
            clear_slot(self.id);
            match call_activity(Call::Stop(self.id)) {
                Some(code) => stop_result(code),
                None => Err(RestoreFailed),
            }
        }
    }

    impl AudioSessionGuard for AndroidAudioSessionGuard {
        fn stop(mut self: Box<Self>) -> Result<(), RestoreFailed> {
            self.teardown()
        }
    }

    impl Drop for AndroidAudioSessionGuard {
        fn drop(&mut self) {
            if !self.done {
                let _ = self.teardown();
            }
        }
    }
}

#[cfg(target_os = "android")]
pub(crate) use imp::implementation;

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::audio_session::AudioSession> {
    std::sync::Arc::new(fandhe_device::audio_session::UnsupportedAudioSession)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::audio_session::AudioMode;

    #[test]
    fn host_implementation_is_unsupported() {
        assert_eq!(implementation().support(), Support::NONE);
        assert!(implementation().read_current().is_err());
    }

    #[test]
    fn support_is_android_shape() {
        let s = android_support();
        assert_eq!(s.kinds, [true, true, false]);
        assert_eq!(
            s.events,
            [false, false, false, true, true, true, true, true]
        );
        assert!(!s.kind_readable && !s.other_playing_readable);
        assert!(s.audio_mode_readable && s.music_active_readable);
    }

    #[test]
    fn java_kind_rejects_mix() {
        assert_eq!(java_kind(SessionKind::StopOthers), Some(0));
        assert_eq!(java_kind(SessionKind::DuckOthers), Some(1));
        assert_eq!(java_kind(SessionKind::MixWithOthers), None);
    }

    #[test]
    fn stop_result_only_zero_is_ok() {
        assert!(stop_result(0).is_ok());
        assert!(stop_result(1).is_err());
        assert!(stop_result(-1).is_err());
    }

    #[test]
    fn decode_read_maps_and_fails_closed() {
        let s = decode_read(3 | (1 << 8)).unwrap();
        assert_eq!(s.audio_mode, Some(AudioMode::InCommunication));
        assert_eq!(s.music_active, Some(true));
        assert_eq!((s.kind, s.other_audio_playing), (None, None));
        assert_eq!(
            decode_read(0xFF).unwrap().audio_mode,
            Some(AudioMode::Unknown)
        );
        assert_eq!(decode_read(0).unwrap().music_active, Some(false));
        assert!(decode_read(-1).is_err());
        assert!(decode_read(1 << 9).is_err());
    }
}
