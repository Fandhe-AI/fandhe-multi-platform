//! Android 向け通話 UI 実装（Issue #278、親 #87。(a) は #276）。`device/call_ui.rs::platform_call_ui`
//! の android 経路から呼ばれ、self-managed `ConnectionService` と着信通知（CallStyle + full-screen
//! intent）を Java の `CallUiHelper` 経由で動かす。応答・拒否の結果は native コールバックで
//! `CallUiSink` へ流す。
//!
//! # 結果コード（SSOT は Java 側 `CallUiHelper`）
//!
//! `MainActivity.callUiReport(long)` の戻り値: 0 = 受け付けた（確定は非同期の `Ringing`）/
//! 1 = 非対応 / 2 = 失敗。未知の値は失敗。`nativeOnCallUiEvent(id, code)` の code は
//! [`decode_event`] の写像表のとおりで、未知の値は `Failed{Failed}` に倒し成功にはしない。
//!
//! # 方式: id 照合 + 並行性の集約（契約表 2・11）
//!
//! ドライバが採番した `call_id` を Java へ渡し、native 通知は id が現在値と一致するときだけ
//! sink へ渡す。取り消し・差し替え・終端後に遅れて届いた通知は捨てる。Java 側の状態は
//! main looper 1 本、Rust 側のロックは `SLOT` 1 本。sink は `SLOT` のロックの内側で呼ぶ
//! （`end` が戻った後に sink が走らないことの保証）ため、取得順は SLOT → ドライバのロックの
//! 一方向。ドライバは自分のロックを持ったまま `report_incoming` / `end` を呼ばないので
//! デッドロックしない。`report_incoming` / `end` は SLOT を手放してから JNI を呼ぶ。
//!
//! 期限（`REPORT_DEADLINE` / `RING_TIMEOUT`）はドライバだけが持つ。期限切れ・画面からの取り消しは
//! ドライバが状態を確定してから `end` を呼ぶので、Java は LocalEnded / Unanswered を報告しない。
//!
//! 守る範囲: 1 プロセス内の順序、嘘の成功の禁止（`Ringing` は通知の投稿成功後）、
//! 再生成・取り消し後の通知の破棄、通知・Connection の解放。守らない範囲: 背景での通話の継続、
//! プロセス死、`PhoneAccount` の登録を残すこと。id・発信者名・例外内容はログへ出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `callUiReport` を経た後でしか呼ばれない。
//! プロセス死後に残った通知のボタンは Java 側で `currentId == NONE` を見て native を呼ばず消す。

use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::Mutex;

#[cfg(not(target_os = "android"))]
use fandhe_device::call_ui::UnsupportedCallUi;
use fandhe_device::call_ui::{CallUi, CallUiEvent, CallUiFailure, CallUiSink, EndReason};

/// `callUiReport` の結果コード（SSOT は Java 側 `CallUiHelper.REPORT_*`）。
const REPORT_ACCEPTED: i32 = 0;
const REPORT_UNSUPPORTED: i32 = 1;

/// Java の受理コードを契約のエラーへ写像する。未知の値は失敗。
fn decode_report(code: i32) -> Result<(), CallUiFailure> {
    match code {
        REPORT_ACCEPTED => Ok(()),
        REPORT_UNSUPPORTED => Err(CallUiFailure::Unsupported),
        _ => Err(CallUiFailure::Failed),
    }
}

/// Java の通知コード（SSOT は `CallUiHelper.EVENT_*`）を契約の通知へ写像する。
/// 未知・負の値は `Failed{Failed}` で、成功値には倒さない。
fn decode_event(call_id: u64, code: i32) -> CallUiEvent {
    let ended = |reason| CallUiEvent::Ended { call_id, reason };
    let failed = |failure| CallUiEvent::Failed { call_id, failure };
    match code {
        0 => CallUiEvent::Ringing { call_id },
        1 => CallUiEvent::Answered { call_id },
        2 => ended(EndReason::Declined),
        3 => ended(EndReason::RemoteEnded),
        4 => failed(CallUiFailure::Unsupported),
        5 => failed(CallUiFailure::PermissionDenied),
        6 => failed(CallUiFailure::Disallowed),
        _ => failed(CallUiFailure::Failed),
    }
}

/// 現在の通話 id と sink を守る純ロジック（host でテストする）。
#[derive(Default)]
struct Slot {
    current: Option<(u64, CallUiSink)>,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl Slot {
    /// 現在の要求を差し替える。
    fn begin(&mut self, id: u64, sink: CallUiSink) {
        self.current = Some((id, sink));
    }

    /// id が一致したときだけ外す。
    fn clear(&mut self, id: u64) {
        if matches!(&self.current, Some((cur, _)) if *cur == id) {
            self.current = None;
        }
    }

    /// 現在の id の通知だけ sink へ渡す。終端の通知か sink が false を返したら外す。
    fn emit(&mut self, id: u64, code: i32) {
        let Some((cur, sink)) = self.current.as_ref() else {
            return;
        };
        if *cur != id {
            return;
        }
        let ev = decode_event(id, code);
        let terminal = matches!(ev, CallUiEvent::Ended { .. } | CallUiEvent::Failed { .. });
        if !sink(ev) || terminal {
            self.current = None;
        }
    }
}

#[cfg(target_os = "android")]
static SLOT: Mutex<Option<Slot>> = Mutex::new(None);

/// ロックの poison は無視して進める（sink 内の panic で以後の `end` が効かなくなるのを避ける）。
#[cfg(target_os = "android")]
fn with_slot<R>(f: impl FnOnce(&mut Slot) -> R) -> R {
    let mut g = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Slot::default))
}

#[cfg(target_os = "android")]
struct AndroidCallUi;

#[cfg(target_os = "android")]
impl CallUi for AndroidCallUi {
    fn report_incoming(&self, call_id: u64, sink: CallUiSink) -> Result<(), CallUiFailure> {
        with_slot(|s| s.begin(call_id, sink));
        // SLOT を手放してから JNI を呼ぶ。
        let result = match call_activity(call_id, true) {
            Some(code) => decode_report(code),
            None => Err(CallUiFailure::Failed),
        };
        if result.is_err() {
            with_slot(|s| s.clear(call_id));
        }
        result
    }

    fn end(&self, call_id: u64) {
        with_slot(|s| s.clear(call_id));
        // Java の onDestroy が先に片付けている場合の失敗は無視してよい（冪等）。
        let _ = call_activity(call_id, false);
    }
}

/// `MainActivity.callUiReport(long)`（`report == true`、戻り値は結果コード）または
/// `callUiEnd(long)`（`false`）を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_midi.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(call_id: u64, report: bool) -> Option<i32> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        let arg = [jni::objects::JValue::Long(call_id as jni::sys::jlong)];
        if report {
            Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("callUiReport"),
                    jni::jni_sig!((long) -> int),
                    &arg,
                )?
                .i()?)
        } else {
            env.call_method(
                &activity,
                jni::jni_str!("callUiEnd"),
                jni::jni_sig!((long) -> ()),
                &arg,
            )?;
            Ok(0)
        }
    })
    .ok()
}

/// `CallUiHelper.nativeOnCallUiEvent`（Java 側）から呼ばれる JNI エクスポート。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_CallUiHelper_nativeOnCallUiEvent<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    call_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    with_slot(|s| s.emit(call_id as u64, code));
}

/// Android の `CallUi` 実装を返す（`device/call_ui.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn CallUi> {
    Arc::new(AndroidCallUi)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn CallUi> {
    Arc::new(UnsupportedCallUi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Log = Arc<Mutex<Vec<CallUiEvent>>>;

    fn sink(log: &Log, ret: bool) -> CallUiSink {
        let log = log.clone();
        Arc::new(move |ev| {
            log.lock().unwrap().push(ev);
            ret
        })
    }

    fn setup(ret: bool) -> (Slot, Log) {
        let log = Log::default();
        let mut s = Slot::default();
        s.begin(7, sink(&log, ret));
        (s, log)
    }

    #[test]
    fn decode_report_maps_codes() {
        assert_eq!(decode_report(0), Ok(()));
        assert_eq!(decode_report(1), Err(CallUiFailure::Unsupported));
        assert_eq!(decode_report(2), Err(CallUiFailure::Failed));
        assert_eq!(decode_report(-1), Err(CallUiFailure::Failed));
        assert_eq!(decode_report(99), Err(CallUiFailure::Failed));
    }

    #[test]
    fn decode_event_maps_all_codes_and_unknown_fails() {
        let f = |failure| CallUiEvent::Failed {
            call_id: 1,
            failure,
        };
        let e = |reason| CallUiEvent::Ended { call_id: 1, reason };
        assert_eq!(decode_event(1, 0), CallUiEvent::Ringing { call_id: 1 });
        assert_eq!(decode_event(1, 1), CallUiEvent::Answered { call_id: 1 });
        assert_eq!(decode_event(1, 2), e(EndReason::Declined));
        assert_eq!(decode_event(1, 3), e(EndReason::RemoteEnded));
        assert_eq!(decode_event(1, 4), f(CallUiFailure::Unsupported));
        assert_eq!(decode_event(1, 5), f(CallUiFailure::PermissionDenied));
        assert_eq!(decode_event(1, 6), f(CallUiFailure::Disallowed));
        assert_eq!(decode_event(1, 7), f(CallUiFailure::Failed));
        assert_eq!(decode_event(1, -1), f(CallUiFailure::Failed));
        assert_eq!(decode_event(1, 1000), f(CallUiFailure::Failed));
    }

    #[test]
    fn stale_id_is_dropped() {
        let (mut s, log) = setup(true);
        s.emit(6, 0);
        s.emit(8, 0);
        assert!(log.lock().unwrap().is_empty());
        s.emit(7, 0);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn nothing_after_terminal_or_clear() {
        let (mut s, log) = setup(true);
        s.emit(7, 0);
        s.emit(7, 2);
        s.emit(7, 1);
        assert_eq!(log.lock().unwrap().len(), 2);
        let (mut s, log) = setup(true);
        s.clear(6);
        s.emit(7, 0);
        s.clear(7);
        s.emit(7, 1);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn answered_keeps_slot_until_end() {
        let (mut s, log) = setup(true);
        s.emit(7, 1);
        s.emit(7, 3);
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[test]
    fn sink_false_detaches() {
        let (mut s, log) = setup(false);
        s.emit(7, 0);
        s.emit(7, 1);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn begin_replaces_previous_request() {
        let (mut s, log) = setup(true);
        let log2 = Log::default();
        s.begin(8, sink(&log2, true));
        s.emit(7, 0);
        s.emit(8, 0);
        assert!(log.lock().unwrap().is_empty());
        assert_eq!(log2.lock().unwrap().len(), 1);
    }
}
