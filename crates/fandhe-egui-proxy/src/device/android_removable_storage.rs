//! Android 向け外部ストレージの着脱実装（Issue #859、親 #175。共通コアと macOS / Windows は #858）。
//! `device/removable_storage.rs::platform_removable_storage` の android 経路から UI スレッドで呼ばれ、
//! ボリュームの状態変化を Java の `RemovableStorageHelper`（`StorageVolumeCallback`、API 30）経由で受け、
//! 今の一覧を `StorageManager.getStorageVolumes()`（API 24）で読む。契約は
//! `fandhe_device::removable_storage`。
//!
//! # 結果コード（SSOT は Java 側 `RemovableStorageHelper`）
//!
//! - `MainActivity.removableStorageSupport()` の戻り値: 1 = 通知が取れる（API 30 以上）/ それ以外 = 取れない
//! - `MainActivity.removableStorageStart(long)` の戻り値: 0 = 受け付けた / 2 = 非対応（API 30 未満）/
//!   それ以外 = 失敗
//! - `MainActivity.removableStorageList()` の戻り値: null = 失敗。成功は「件数」の後に項目ごと
//!   U+001E + flags（bit0 = primary / bit1 = removable）+ U+001F + state + U+001F + description
//! - `nativeOnRemovableStorageEvent(id, state)` の state: `StorageVolume.getState()`（null は空文字）。
//!   空文字・未知の値は「その他の変化」で、出来事を捏造せず、失敗にもしない
//!
//! # OS を正にする範囲
//!
//! 一覧は覚えず、出来事が届くたびにドライバが `read_volumes` で OS から読み直す。通知の中身
//! （`StorageVolume`）からは一覧を組み立てない。種類・状態は OS が返した値をそのまま写す。
//! 読み取りの失敗・null・形が不正な文字列を 0 件へ畳まない。UUID・`getDirectory`・パスは読まず、
//! 権限もマニフェスト宣言も足さない。
//!
//! # 方式: id 照合 + 解除の集約（契約表 1・2・9・11）
//!
//! 購読は `register` で得た id を Java へ渡して数え、通知は id を添えて `fire` へ流す。停止
//! （[`AndroidGuard`] の drop）は「登録表から外す → Java のコールバック解除」の順で、解除後に
//! 遅れて届いた通知と、Activity 再生成前の古い id の通知は登録表が捨てる（id は単調増加）。
//! 開始は同期で、失敗したら登録表に何も残さない。`onPause` では止めない。`onDestroy` は Java 側が
//! コールバックを解除し、停止を「失敗」として送らない（契約表 10）。ロック取得順は Java → Rust の
//! 一方向（Rust は登録表のロックを手放してから Java を呼ぶ）。API 30 未満は `support()` がすべて
//! false、`start` が `Unsupported` で、一覧だけ読める。
//!
//! 守る範囲: 1 プロセス内の受信順、失敗・非対応を成功へ畳まないこと、全終了経路でのコールバック解除、
//! 解除後の通知の破棄。守らない範囲: 画面を開く前・止めた後の出来事、通知と一覧の読み直しの間の時間差、
//! 同じ着脱での複数回通知、一覧の読み取りの期限、複数プロセス、OS 側の遅れ。ボリューム名・状態・id・
//! 例外の内容はログ・エラーに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `removableStorageStart` を経た後でしか呼ばれず、
//! feature `device` が無効のビルドではその経路に届かない（sleep-lock-events と同じ根拠）。
//!
//! 一次情報:
//! <https://developer.android.com/reference/android/os/storage/StorageManager>、
//! <https://developer.android.com/reference/android/os/storage/StorageManager.StorageVolumeCallback>、
//! <https://developer.android.com/reference/android/os/storage/StorageVolume>

use std::sync::Arc;

#[cfg(not(target_os = "android"))]
use fandhe_device::removable_storage::UnsupportedRemovableStorage;
#[cfg(target_os = "android")]
use fandhe_device::removable_storage::{
    event_from_android_state, OnSignal, Signal, SubscriptionGuard,
};
use fandhe_device::removable_storage::{
    kind_from_android, state_from_android, EventSupport, RemovableStorage, VolumeKind, VolumeState,
};
#[cfg(target_os = "android")]
use fandhe_device::removable_storage::{volumes, Volume};
use fandhe_device::DeviceError;

/// `removableStorageStart` の成功コード（SSOT は Java 側 `RemovableStorageHelper.START_OK`）。
const START_OK: i32 = 0;
/// 非対応（API 30 未満。SSOT は `START_UNSUPPORTED`）。
const START_UNSUPPORTED: i32 = 2;
/// Java が項目の先頭に置く区切り文字（U+001E）。
const REC: char = '\u{1E}';
/// Java がフィールドをつなぐ区切り文字（U+001F）。
const SEP: char = '\u{1F}';

/// 開始の結果コードを写像する。未知の値は失敗で、成功へは倒さない。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_start(code: i32) -> Result<(), DeviceError> {
    match code {
        START_OK => Ok(()),
        START_UNSUPPORTED => Err(DeviceError::Unsupported("removable_storage")),
        _ => Err(DeviceError::Internal("removable_storage start")),
    }
}

/// `removableStorageSupport` の戻り値を写像する。1 以外は取れないことにする（嘘の対応を出さない）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_support(code: i32) -> EventSupport {
    if code == 1 {
        // `event_from_android_state` は 4 種すべてを出しうる。
        EventSupport { events: [true; 4] }
    } else {
        EventSupport::NONE
    }
}

type RawVolume = (Option<String>, VolumeKind, VolumeState);

/// Java の一覧文字列を `volumes()` へ渡す形へ写す。件数・項目数・フィールド数・flags が
/// 合わなければ `None`（失敗）。名前は空でも `Some` で渡す（`clean_text` が `None` にする）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn from_android(raw: &str) -> Option<Vec<RawVolume>> {
    let mut it = raw.split(REC);
    let count: usize = it.next()?.parse().ok()?;
    let mut out = Vec::new();
    for item in it {
        let mut f = item.split(SEP);
        let flags: u8 = f.next()?.parse().ok()?;
        let state = f.next()?;
        let desc = f.next()?;
        if f.next().is_some() || flags > 3 {
            return None;
        }
        out.push((
            Some(desc.to_string()),
            kind_from_android(flags & 1 != 0, flags & 2 != 0),
            state_from_android(state),
        ));
    }
    (out.len() == count).then_some(out)
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
        // 先に登録を外し、以後の遅れた通知を捨てさせてから Java のコールバックを外す。
        super::removable_storage::unregister(self.id);
        // Java の onDestroy が先に片付けている場合の失敗は無視してよい（冪等）。
        let _ = call_activity(Call::Stop(self.id));
    }
}

#[cfg(target_os = "android")]
struct AndroidRemovableStorage;

#[cfg(target_os = "android")]
impl RemovableStorage for AndroidRemovableStorage {
    fn support(&self) -> EventSupport {
        match call_activity(Call::Support) {
            Some(JniRet::Int(code)) => decode_support(code),
            _ => EventSupport::NONE,
        }
    }

    fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
        let id = super::removable_storage::register(on_signal);
        let result = match call_activity(Call::Start(id)) {
            Some(JniRet::Int(code)) => decode_start(code),
            _ => Err(DeviceError::Internal("removable_storage start")),
        };
        if let Err(e) = result {
            super::removable_storage::unregister(id);
            return Err(e);
        }
        Ok(Box::new(AndroidGuard { id }))
    }

    fn read_volumes(&self) -> Result<Vec<Volume>, DeviceError> {
        const FAILED: DeviceError = DeviceError::Internal("removable_storage read");
        let Some(JniRet::Str(Some(raw))) = call_activity(Call::List) else {
            return Err(FAILED);
        };
        Ok(volumes(from_android(&raw).ok_or(FAILED)?))
    }
}

/// Java へ渡す呼び出しの種類。
#[cfg(target_os = "android")]
enum Call {
    Support,
    Start(u64),
    Stop(u64),
    List,
}

/// JNI 呼び出しの戻り値。
#[cfg(target_os = "android")]
enum JniRet {
    Int(i32),
    /// null は `None`（失敗）。
    Str(Option<String>),
}

/// `MainActivity.removableStorage*` を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_sleep_lock_events.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(call: Call) -> Option<JniRet> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<JniRet, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        let r = match call {
            Call::Support => env
                .call_method(
                    &activity,
                    jni::jni_str!("removableStorageSupport"),
                    jni::jni_sig!(() -> int),
                    &[],
                )
                .and_then(|v| v.i())
                .map(JniRet::Int),
            Call::Start(id) => env
                .call_method(
                    &activity,
                    jni::jni_str!("removableStorageStart"),
                    jni::jni_sig!((long) -> int),
                    &[jni::objects::JValue::Long(id as jni::sys::jlong)],
                )
                .and_then(|v| v.i())
                .map(JniRet::Int),
            Call::Stop(id) => env
                .call_method(
                    &activity,
                    jni::jni_str!("removableStorageStop"),
                    jni::jni_sig!((long) -> ()),
                    &[jni::objects::JValue::Long(id as jni::sys::jlong)],
                )
                .map(|_| JniRet::Int(0)),
            Call::List => env
                .call_method(
                    &activity,
                    jni::jni_str!("removableStorageList"),
                    jni::jni_sig!(() -> java.lang.String),
                    &[],
                )
                .and_then(|v| v.l())
                .and_then(|o| {
                    let s = env.cast_local::<jni::objects::JString>(o)?;
                    if s.is_null() {
                        Ok(JniRet::Str(None))
                    } else {
                        Ok(JniRet::Str(Some(s.try_to_string(env)?)))
                    }
                }),
        };
        // 例外は自動クリアされない（jni 0.22）。文言は読まない。
        if r.is_err() && env.exception_check() {
            env.exception_clear();
        }
        Ok(r?)
    })
    .ok()
}

/// `RemovableStorageHelper.nativeOnRemovableStorageEvent`（Java 側）から呼ばれる JNI エクスポート。
/// 登録表に無い id（停止後・再生成前の古い id）の通知は捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_RemovableStorageHelper_nativeOnRemovableStorageEvent<
    'caller,
>(
    mut env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    id: jni::sys::jlong,
    state: jni::objects::JString<'caller>,
) {
    let _ = env
        .with_env(|env| -> Result<(), jni::errors::Error> {
            // null・変換失敗は空文字（その他の変化）として扱う。
            let s = if state.is_null() {
                String::new()
            } else {
                state.try_to_string(env).unwrap_or_default()
            };
            super::removable_storage::fire(id as u64, Signal::Event(event_from_android_state(&s)));
            Ok(())
        })
        .into_outcome();
}

/// Android の `RemovableStorage` 実装を返す（`device/removable_storage.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn RemovableStorage> {
    Arc::new(AndroidRemovableStorage)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn RemovableStorage> {
    Arc::new(UnsupportedRemovableStorage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::removable_storage::volumes;

    #[test]
    fn host_implementation_is_unsupported() {
        let imp = implementation();
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("removable_storage"))
        ));
        assert!(matches!(
            imp.read_volumes(),
            Err(DeviceError::Unsupported("removable_storage"))
        ));
    }

    #[test]
    fn decode_start_maps_codes() {
        assert!(decode_start(0).is_ok());
        assert!(matches!(
            decode_start(2),
            Err(DeviceError::Unsupported("removable_storage"))
        ));
        assert!(matches!(
            decode_start(1),
            Err(DeviceError::Internal("removable_storage start"))
        ));
        assert!(matches!(
            decode_start(99),
            Err(DeviceError::Internal("removable_storage start"))
        ));
    }

    #[test]
    fn decode_support_only_one_is_supported() {
        assert_eq!(decode_support(1), EventSupport { events: [true; 4] });
        assert_eq!(decode_support(0), EventSupport::NONE);
        assert_eq!(decode_support(-1), EventSupport::NONE);
    }

    #[test]
    fn from_android_reads_items() {
        let raw = "2\u{1E}3\u{1F}mounted\u{1F}SD card\u{1E}1\u{1F}mounted_ro\u{1F}Internal";
        let v = from_android(raw).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].1, VolumeKind::Removable);
        assert_eq!(v[0].2, VolumeState::Mounted);
        assert_eq!(v[1].1, VolumeKind::Internal);
        assert_eq!(v[1].2, VolumeState::MountedReadOnly);
    }

    #[test]
    fn from_android_zero_is_empty_list() {
        assert_eq!(from_android("0").map(|v| v.len()), Some(0));
    }

    #[test]
    fn from_android_rejects_malformed() {
        for raw in [
            "",
            "x",
            "1",
            "0\u{1E}1\u{1F}mounted\u{1F}a",
            "1\u{1E}4\u{1F}mounted\u{1F}a",
            "1\u{1E}1\u{1F}mounted",
            "1\u{1E}1\u{1F}mounted\u{1F}a\u{1F}b",
            "1\u{1E}x\u{1F}mounted\u{1F}a",
        ] {
            assert!(from_android(raw).is_none(), "{raw:?}");
        }
    }

    #[test]
    fn empty_name_becomes_none_through_volumes() {
        let v = volumes(from_android("1\u{1E}2\u{1F}removed\u{1F}").unwrap());
        assert_eq!(v[0].name, None);
        assert_eq!(v[0].state, VolumeState::Removed);
    }
}
