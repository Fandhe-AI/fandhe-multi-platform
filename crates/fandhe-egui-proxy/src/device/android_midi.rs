//! Android 向け MIDI 入力実装（Issue #1400、親 #1137）。`device/midi.rs::platform_midi` の
//! Android 経路から呼ばれ、`MidiManager` が見せる出力ポートの受信バイト列を
//! `fandhe_device::midi::MidiParser` でノートへ組み立てて sink へ流す。Java 側は `MidiHelper`。
//!
//! # 方式: push 型 + 世代（epoch）照合
//!
//! Rust が epoch（プロセス内で単調増加。リセットでも 0 へ戻さない）を採番して
//! `MainActivity.midiStart(epoch)` へ渡し、Java は開始確定・入力数・受信バイト列を epoch 付きの
//! native コールバックで返す。Rust は「epoch が現在値と一致し sink がある」ときだけ sink を呼ぶ。
//! 停止後・Activity 再生成後に遅れて届いた通知は epoch 不一致で捨てる。
//!
//! # 結果コード（SSOT は Java 側 `MidiHelper.START_*`）
//!
//! `midiStart` の戻り値: 0 = 受け付けた（確定は非同期の `Started`）/ 1 = 非対応（`FEATURE_MIDI` なし）
//! / 2 = 失敗。未知の値は失敗。0 台の成功は `Started{sources: 0}` で、非対応・失敗とは別。
//!
//! # 並行性の集約（契約表 2・11）
//!
//! ロックは Java の制御ロック 1 本（`MidiHelper.lock`）と、本ファイルの `ROUTER` 1 本だけ。
//! sink は `ROUTER` のロックの内側で呼ぶ（`stop` が戻った後に sink が走らないことの保証。
//! sink は mpsc 送信と atomic 操作だけでブロックしない）。取得順は「Java → Rust」の一方向で、
//! Rust の `start` / `stop` は `ROUTER` を手放してから JNI を呼ぶ。
//!
//! 守る範囲: 1 プロセス内の順序、非対応・失敗を成功へ畳まないこと、停止・再生成後の通知の破棄、
//! 開いたポートの解放。守らない範囲: `onPause` 後の背景受信の停止制御、BLE MIDI の接続
//! （`openBluetoothDevice` は範囲外）、抜き差し確定との時間差による入力数表示の一時的な食い違い。
//! 演奏内容・デバイス名・例外メッセージはログへ出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `midiStart` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（pedometer / mic-record と同じ）。

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::Mutex;

use fandhe_device::midi::{MidiEvent, MidiInput, MidiParser, MidiSink};
use fandhe_device::DeviceError;

/// `midiStart` の結果コード（SSOT は Java 側 `MidiHelper.START_*`）。
const START_OK: i32 = 0;
const START_UNSUPPORTED: i32 = 1;

/// JNI の受信バイト列の上限。`convert_byte_array` が Vec を確保する前に検査し、超えたら
/// 丸めず捨てる（メモリ枯渇防止。MIDI 1.0 の USB パケットより十分大きい）。
const MAX_DATA_LEN: usize = 4096;

/// Java の開始コードを契約のエラーへ写像する。未知の値は失敗。
fn decode_start(code: i32) -> Result<(), DeviceError> {
    match code {
        START_OK => Ok(()),
        START_UNSUPPORTED => Err(DeviceError::Unsupported("midi")),
        _ => Err(DeviceError::Internal("midi start")),
    }
}

/// epoch・sink・入力ソースごとの running status を守る純ロジック（host でテストする）。
#[derive(Default)]
struct Router {
    epoch: u64,
    sink: Option<MidiSink>,
    parsers: HashMap<i64, MidiParser>,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl Router {
    /// 新しい購読を始める。epoch を進めて旧通知を無効にし、新しい epoch を返す。
    fn begin(&mut self, sink: MidiSink) -> u64 {
        self.epoch += 1;
        self.sink = Some(sink);
        self.parsers.clear();
        self.epoch
    }

    /// 購読を外す。以後の通知は epoch 不一致で捨てる。
    fn end(&mut self) {
        self.epoch += 1;
        self.sink = None;
        self.parsers.clear();
    }

    /// 開始呼び出しの失敗で、その epoch の sink だけを外す。
    fn fail(&mut self, epoch: u64) {
        if epoch == self.epoch {
            self.sink = None;
            self.parsers.clear();
        }
    }

    /// 現在の epoch の sink へ通知を渡す。false を返されたら受け手なしとして外す。
    fn emit(&mut self, epoch: u64, ev: MidiEvent) {
        if epoch != self.epoch {
            return;
        }
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        if !sink(ev) {
            self.sink = None;
            self.parsers.clear();
        }
    }

    /// 負値は `usize` へ変換する前に弾く（弾いたら通知しない）。
    fn counts(sources: i32, failed: i32) -> Option<(usize, usize)> {
        Some((
            usize::try_from(sources).ok()?,
            usize::try_from(failed).ok()?,
        ))
    }

    fn started(&mut self, epoch: u64, sources: i32, failed: i32) {
        if let Some((sources, failed)) = Self::counts(sources, failed) {
            self.emit(epoch, MidiEvent::Started { sources, failed });
        }
    }

    fn sources(&mut self, epoch: u64, sources: i32, failed: i32) {
        if let Some((sources, failed)) = Self::counts(sources, failed) {
            self.emit(epoch, MidiEvent::Sources { sources, failed });
        }
    }

    /// 入力ソース `key` の running status と未完成メッセージを捨てる（flush・ポート抜去）。
    fn flush(&mut self, epoch: u64, key: i64) {
        if epoch == self.epoch {
            self.parsers.remove(&key);
        }
    }

    /// 入力ソース `key` のバイト列を組み立て、完成したノートをまとめて 1 回で流す。
    fn data(&mut self, epoch: u64, key: i64, bytes: &[u8]) {
        if epoch != self.epoch || self.sink.is_none() || bytes.len() > MAX_DATA_LEN {
            return;
        }
        let parser = self.parsers.entry(key).or_default();
        let notes: Vec<_> = bytes.iter().filter_map(|b| parser.feed(*b)).collect();
        if !notes.is_empty() {
            self.emit(epoch, MidiEvent::Notes(notes));
        }
    }
}

#[cfg(target_os = "android")]
static ROUTER: Mutex<Option<Router>> = Mutex::new(None);

/// ロックの poison は無視して進める（sink 内の panic で以後の停止が効かなくなるのを避ける）。
#[cfg(target_os = "android")]
fn with_router<R>(f: impl FnOnce(&mut Router) -> R) -> R {
    let mut g = ROUTER.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Router::default))
}

#[cfg(target_os = "android")]
struct AndroidMidi;

#[cfg(target_os = "android")]
impl MidiInput for AndroidMidi {
    fn start(&self, sink: MidiSink) -> Result<(), DeviceError> {
        let epoch = with_router(|r| r.begin(sink));
        // ROUTER を手放してから JNI を呼ぶ（Java → Rust の一方向を守る）。
        let result = match call_activity(Some(epoch)) {
            Some(code) => decode_start(code),
            None => Err(DeviceError::Internal("midi start")),
        };
        if result.is_err() {
            with_router(|r| r.fail(epoch));
        }
        result
    }

    fn stop(&self) {
        with_router(|r| r.end());
        // Java の onDestroy が先に片付けている場合の失敗は無視してよい。
        let _ = call_activity(None);
    }
}

/// `MainActivity.midiStart(long)`（`Some(epoch)`、戻り値は結果コード）または
/// `midiStop()`（`None`）を JNI で呼ぶ。`None` の戻りは呼び出し失敗。
/// `unsafe` 2 箇所は `android_pedometer.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(epoch: Option<u64>) -> Option<i32> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        match epoch {
            Some(e) => Ok(env
                .call_method(
                    &activity,
                    jni::jni_str!("midiStart"),
                    jni::jni_sig!((long) -> int),
                    &[jni::objects::JValue::Long(e as jni::sys::jlong)],
                )?
                .i()?),
            None => {
                env.call_method(
                    &activity,
                    jni::jni_str!("midiStop"),
                    jni::jni_sig!(() -> ()),
                    &[],
                )?;
                Ok(0)
            }
        }
    })
    .ok()
}

/// `MidiHelper.nativeOnMidiStarted`（Java 側）から呼ばれる JNI エクスポート。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MidiHelper_nativeOnMidiStarted<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    epoch: jni::sys::jlong,
    sources: jni::sys::jint,
    failed: jni::sys::jint,
) {
    with_router(|r| r.started(epoch as u64, sources, failed));
}

/// `MidiHelper.nativeOnMidiSources`（Java 側）から呼ばれる JNI エクスポート（抜き差し）。
/// 固定シンボル名の根拠は `nativeOnMidiStarted` と同じ。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MidiHelper_nativeOnMidiSources<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    epoch: jni::sys::jlong,
    sources: jni::sys::jint,
    failed: jni::sys::jint,
) {
    with_router(|r| r.sources(epoch as u64, sources, failed));
}

/// `MidiHelper.nativeOnMidiData`（Java 側）から呼ばれる JNI エクスポート。受信バイト列は
/// ログへ出さない。長さは `convert_byte_array` の前に検査する。
/// 固定シンボル名の根拠は `nativeOnMidiStarted` と同じ。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MidiHelper_nativeOnMidiData<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    epoch: jni::sys::jlong,
    key: jni::sys::jlong,
    bytes: jni::objects::JByteArray<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        if bytes.is_null() {
            return Ok(());
        }
        let len = bytes.len(env)?;
        if len > MAX_DATA_LEN {
            return Ok(());
        }
        let owned = env.convert_byte_array(&bytes)?;
        with_router(|r| r.data(epoch as u64, key, &owned));
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

/// `MidiHelper.nativeOnMidiFlush`（Java 側）から呼ばれる JNI エクスポート。`onFlush` と
/// ポート抜去で、その入力ソースのパーサーを捨てる。固定シンボル名の根拠は `nativeOnMidiStarted` と同じ。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MidiHelper_nativeOnMidiFlush<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    epoch: jni::sys::jlong,
    key: jni::sys::jlong,
) {
    with_router(|r| r.flush(epoch as u64, key));
}

/// Android の `MidiInput` 実装を返す（`device/midi.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn MidiInput> {
    Arc::new(AndroidMidi)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn MidiInput> {
    Arc::new(fandhe_device::midi::UnsupportedMidiInput)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::midi::NoteEvent;
    use std::sync::Mutex;

    type Log = Arc<Mutex<Vec<String>>>;

    fn sink(log: &Log, ret: bool) -> MidiSink {
        let log = log.clone();
        Arc::new(move |ev| {
            let s = match ev {
                MidiEvent::Started { sources, failed } => format!("started {sources} {failed}"),
                MidiEvent::Sources { sources, failed } => format!("sources {sources} {failed}"),
                MidiEvent::Notes(n) => format!(
                    "notes {}",
                    n.iter()
                        .map(|e: &NoteEvent| format!("{}:{}", e.on as u8, e.note))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                MidiEvent::Failed(_) => "failed".into(),
            };
            log.lock().unwrap().push(s);
            ret
        })
    }

    fn setup(ret: bool) -> (Router, u64, Log) {
        let log = Log::default();
        let mut r = Router::default();
        let e = r.begin(sink(&log, ret));
        (r, e, log)
    }

    fn lines(log: &Log) -> Vec<String> {
        log.lock().unwrap().clone()
    }

    #[test]
    fn stale_epoch_is_dropped() {
        let (mut r, e, log) = setup(true);
        r.started(e - 1, 1, 0);
        r.sources(e + 1, 1, 0);
        r.data(e - 1, 0, &[0x90, 60, 100]);
        assert!(lines(&log).is_empty());
        r.started(e, 2, 1);
        assert_eq!(lines(&log), ["started 2 1"]);
    }

    #[test]
    fn nothing_after_end_or_before_begin() {
        let mut r = Router::default();
        r.data(0, 0, &[0x90, 60, 100]);
        let (mut r2, e, log) = setup(true);
        r2.end();
        r2.data(e, 0, &[0x90, 60, 100]);
        r2.started(e, 0, 0);
        assert!(lines(&log).is_empty());
    }

    #[test]
    fn parsers_are_per_source() {
        let (mut r, e, log) = setup(true);
        r.data(e, 1, &[0x90, 60, 100]);
        r.data(e, 2, &[0x80, 61, 0]);
        // running status は key ごと: key 1 は note on のまま。
        r.data(e, 1, &[62, 100]);
        assert_eq!(lines(&log), ["notes 1:60", "notes 0:61", "notes 1:62"]);
    }

    #[test]
    fn flush_resets_only_that_source() {
        let (mut r, e, log) = setup(true);
        r.data(e, 1, &[0x90, 60, 100]);
        r.data(e, 2, &[0x90, 61, 100]);
        // key 1 は未完成メッセージの途中で flush。running status も消えるため後続は無視される。
        r.data(e, 1, &[0x90, 62]);
        r.flush(e - 1, 1);
        r.flush(e, 1);
        r.data(e, 1, &[100, 63, 100]);
        r.data(e, 2, &[62, 100]);
        assert_eq!(lines(&log), ["notes 1:60", "notes 1:61", "notes 1:62"]);
    }

    #[test]
    fn non_note_messages_are_ignored() {
        let (mut r, e, log) = setup(true);
        r.data(e, 1, &[0xB0, 7, 100, 0xF0, 1, 2, 0xF7, 0x90, 200]);
        assert!(lines(&log).is_empty());
    }

    #[test]
    fn sink_false_detaches() {
        let (mut r, e, log) = setup(false);
        r.data(e, 1, &[0x90, 60, 100]);
        r.data(e, 1, &[0x90, 61, 100]);
        assert_eq!(lines(&log).len(), 1);
    }

    #[test]
    fn fail_only_detaches_same_epoch() {
        let (mut r, e, log) = setup(true);
        r.fail(e - 1);
        r.started(e, 0, 0);
        assert_eq!(lines(&log).len(), 1);
        r.fail(e);
        r.started(e, 0, 0);
        assert_eq!(lines(&log).len(), 1);
    }

    #[test]
    fn decode_start_maps_codes() {
        assert!(decode_start(0).is_ok());
        assert!(matches!(
            decode_start(1),
            Err(DeviceError::Unsupported("midi"))
        ));
        assert!(matches!(decode_start(2), Err(DeviceError::Internal(_))));
        assert!(matches!(decode_start(99), Err(DeviceError::Internal(_))));
        assert!(matches!(decode_start(-1), Err(DeviceError::Internal(_))));
    }

    #[test]
    fn oversize_and_negative_counts_are_dropped() {
        let (mut r, e, log) = setup(true);
        r.data(e, 1, &vec![0u8; MAX_DATA_LEN + 1]);
        r.started(e, -1, 0);
        r.sources(e, 0, -1);
        assert!(lines(&log).is_empty());
    }
}
