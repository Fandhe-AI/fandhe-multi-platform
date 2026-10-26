//! Android 向けバックグラウンド送信実装（Issue #837、親 #170 の b-android）。
//! `device/background_upload.rs::implementation` の Android 経路から呼ばれる。ドライバの直列ワーカー
//! スレッドが `start` / `cancel` / `status` / `set_signal` を呼び、`MainActivity.backgroundUpload*` を
//! 同期 JNI で叩く。Java 側は `BackgroundUploadHelper`（WorkManager の一意名 + `BackgroundUploadWorker` が
//! `HttpURLConnection` で PUT）。アプリ内に状態を持たず、`status` は呼ぶたびに OS を読み直す。
//!
//! # 結果コード（SSOT は Java `BackgroundUploadHelper`）
//!
//! - `backgroundUploadStart(String) -> int`: 0 = OK / 2 = ファイルを用意できない / 3 = 開始失敗
//! - `backgroundUploadCancel() -> int`: 0 = OK / 1 = 失敗
//! - `backgroundUploadStatus() -> long[5]`: `{kind, sent, total, http, err}`。例外では `null`。
//!   kind 0 = 無し / 1 = 待機 / 2 = 送信中 / 3 = 終了 / 4 = 取り消し。http 無しは -1。
//!   err（kind 3 のみ）0 = 応答あり / 1 = 3xx / 2 = 接続の失敗 / 3 = ファイル無し / 4 = その他
//! - `backgroundUploadObserve() -> int`: 0 = 受け付けた / 1 = 失敗
//!
//! JNI 署名は Java と本ファイルの `jni_sig!` の 2 か所で一致させる。native コールバック
//! （`nativeOnBackgroundUploadChanged`）は `set_signal` が `observe` を呼んだ後でしか Java から
//! 呼ばれないため、no-op スタブは置かない（`android_sleep_lock_events.rs` と同じ根拠）。
//!
//! # ライフサイクル（契約表 1）
//!
//! 転送の持ち主は WorkManager で、Activity の再生成をまたいで続けるのが目的のため `onDestroy` では
//! 取り消さない。Java に保留中の要求は無く、`SharedPreferences` の「今の要求の UUID」に一致する
//! `WorkInfo` だけを読むので、置き換え前の世代の結果が新しい要求を消費しない。LiveData の observer は
//! LifecycleOwner に結び付いて OS が外し、Activity 作り直しでは新しい `onCreate` が登録し直す。
//!
//! # 並行性（契約表 2・11）
//!
//! Java の static ロックで `start` / `cancel` / `status` を直列化する。Rust の呼び出しはドライバの
//! 直列ワーカーから来る。合図の `SIGNAL` はロックを外して呼ぶ。
//!
//! 守る範囲: 1 プロセス内の開始・取り消し・完了の順序、古い世代の `WorkInfo` を UUID で捨てること、
//! 嘘の成功の禁止（HTTP の状態無し・非 2xx・サイズ違い・リダイレクトを成功にしない）、取り消しを
//! 失敗にしないこと。リダイレクトは追わず（`setInstanceFollowRedirects(false)`）3xx を失敗にする。
//! 守らない範囲: WorkManager が消した古い結果（`Idle`）、接続できない間の待機中表示、複数プロセス、
//! 背景中は LiveData が非 active で前面復帰時に読み直すこと。URL・パス・OS の生の文言は出さない（A09）。

#[cfg(any(target_os = "android", test))]
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::Mutex;

#[cfg(all(not(target_os = "android"), test))]
use fandhe_device::background_upload::UnsupportedBackgroundUpload;
#[cfg(any(target_os = "android", test))]
use fandhe_device::background_upload::{
    completion_status, BackgroundUpload, UploadFailure, UploadStatus,
};
#[cfg(any(target_os = "android", test))]
use fandhe_device::DeviceError;

#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    None,
    Waiting,
    Sending,
    Finished,
    Cancelled,
}

/// `backgroundUploadStatus` の戻りを読んだ値。
#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Raw {
    kind: Kind,
    sent: u64,
    total: Option<u64>,
    http: Option<u16>,
    err: i64,
}

/// `long[5]` を `Raw` へ。長さ違い・未知の kind・負の sent・-1 未満の total や http・u16 超の http は `None`。
#[cfg(any(target_os = "android", test))]
fn decode(v: &[i64]) -> Option<Raw> {
    let [kind, sent, total, http, err] = <[i64; 5]>::try_from(v).ok()?;
    let kind = match kind {
        0 => Kind::None,
        1 => Kind::Waiting,
        2 => Kind::Sending,
        3 => Kind::Finished,
        4 => Kind::Cancelled,
        _ => return None,
    };
    Some(Raw {
        kind,
        sent: u64::try_from(sent).ok()?,
        total: match total {
            -1 => None,
            t => Some(u64::try_from(t).ok()?),
        },
        http: match http {
            -1 => None,
            h => Some(u16::try_from(h).ok()?),
        },
        err,
    })
}

#[cfg(any(target_os = "android", test))]
fn to_status(raw: Raw) -> UploadStatus {
    match raw.kind {
        Kind::None => UploadStatus::Idle,
        Kind::Waiting => UploadStatus::Waiting,
        Kind::Sending => UploadStatus::Sending {
            sent: raw.sent,
            total: raw.total,
        },
        Kind::Cancelled => UploadStatus::Cancelled,
        Kind::Finished => match raw.err {
            0 => completion_status(raw.http, raw.sent, false),
            1 => completion_status(raw.http, raw.sent, true),
            2 => UploadStatus::Failed(UploadFailure::Network),
            3 => UploadStatus::Failed(UploadFailure::FileUnavailable),
            _ => UploadStatus::Failed(UploadFailure::Other),
        },
    }
}

/// `start` の結果コードを写す。未知の値は失敗として扱い、成功に倒さない。
#[cfg(any(target_os = "android", test))]
fn decode_start(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        2 => Err(DeviceError::Internal(
            fandhe_device::background_upload::FILE_ERROR,
        )),
        _ => Err(DeviceError::Internal("background upload start")),
    }
}

#[cfg(target_os = "android")]
static SIGNAL: Mutex<Option<fandhe_device::background_upload::OnSignal>> = Mutex::new(None);

#[cfg(target_os = "android")]
struct AndroidBackgroundUpload;

/// `MainActivity` の同期メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`。
/// 2 箇所の `unsafe` は `android_background_download.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity<T>(
    f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> Result<T, super::android::JniCallFailed>,
) -> Option<T> {
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        f(env, &activity)
    })
    .ok()
}

#[cfg(target_os = "android")]
impl BackgroundUpload for AndroidBackgroundUpload {
    fn start(&self, url: &str) -> Result<(), DeviceError> {
        fandhe_device::http::check_url(url)?;
        let code = call_activity(|env, a| {
            let url = env.new_string(url)?;
            Ok(env
                .call_method(
                    a,
                    jni::jni_str!("backgroundUploadStart"),
                    jni::jni_sig!((java.lang.String) -> int),
                    &[jni::objects::JValue::Object(&url)],
                )?
                .i()?)
        })
        .ok_or(DeviceError::Internal("background upload start"))?;
        decode_start(code)
    }

    fn cancel(&self) -> Result<(), DeviceError> {
        let code = call_activity(|env, a| {
            Ok(env
                .call_method(
                    a,
                    jni::jni_str!("backgroundUploadCancel"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?)
        })
        .ok_or(DeviceError::Internal("background upload cancel"))?;
        if code == 0 {
            Ok(())
        } else {
            Err(DeviceError::Internal("background upload cancel"))
        }
    }

    fn status(&self) -> Result<UploadStatus, DeviceError> {
        use jni::objects::JLongArray;
        let values = call_activity(|env, a| {
            let ret = env.call_method(
                a,
                jni::jni_str!("backgroundUploadStatus"),
                jni::jni_sig!(() -> long[]),
                &[],
            )?;
            let arr = env.cast_local::<JLongArray>(ret.l()?)?;
            if arr.is_null() {
                return Ok(None);
            }
            // 長さを検証してから読む。
            if arr.len(env)? != 5 {
                return Ok(None);
            }
            let mut v = [0i64; 5];
            arr.get_region(env, 0, &mut v)?;
            Ok(Some(v))
        })
        .flatten();
        // 読めないときは Idle に畳まず Err（ドライバが ReadFailed にする）。
        values
            .as_ref()
            .and_then(|v| decode(v))
            .map(to_status)
            .ok_or(DeviceError::Internal("background upload status"))
    }

    fn set_signal(
        &self,
        on_signal: fandhe_device::background_upload::OnSignal,
    ) -> Result<(), DeviceError> {
        *SIGNAL.lock().unwrap_or_else(|e| e.into_inner()) = Some(on_signal);
        let code = call_activity(|env, a| {
            Ok(env
                .call_method(
                    a,
                    jni::jni_str!("backgroundUploadObserve"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?)
        })
        .ok_or(DeviceError::Internal("background upload observe"))?;
        if code == 0 {
            Ok(())
        } else {
            Err(DeviceError::Internal("background upload observe"))
        }
    }
}

/// Java の LiveData の変化から呼ばれ、登録済みの合図を鳴らす（状態は運ばない）。
/// ロックは clone してから外し、その外で呼ぶ。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_BackgroundUploadHelper_nativeOnBackgroundUploadChanged<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
) {
    let cb = SIGNAL.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(cb) = cb {
        cb();
    }
}

/// Android の実装を返す。非 Android のテストビルドは `Unsupported` 固定（dispatch は Android でしか呼ばない）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn BackgroundUpload> {
    Arc::new(AndroidBackgroundUpload)
}

/// ホストのテストビルドにも載るための形。
#[cfg(all(not(target_os = "android"), test))]
pub(crate) fn implementation() -> Arc<dyn BackgroundUpload> {
    Arc::new(UnsupportedBackgroundUpload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::background_upload::FILE_SIZE;

    fn fin(sent: i64, http: i64, err: i64) -> UploadStatus {
        to_status(decode(&[3, sent, FILE_SIZE as i64, http, err]).unwrap())
    }

    #[test]
    fn decode_rejects_malformed() {
        assert_eq!(decode(&[2, 1, 5, -1]), None);
        assert_eq!(decode(&[2, 1, 5, -1, 0, 0]), None);
        assert_eq!(decode(&[9, 0, 0, -1, 0]), None);
        assert_eq!(decode(&[2, -1, 5, -1, 0]), None);
        assert_eq!(decode(&[2, 1, -2, -1, 0]), None);
        assert_eq!(decode(&[3, 1, 5, -2, 0]), None);
        assert_eq!(decode(&[3, 1, 5, 70000, 0]), None);
    }

    #[test]
    fn finished_maps_through_completion_status() {
        let n = FILE_SIZE as i64;
        assert_eq!(
            fin(n, 200, 0),
            UploadStatus::Completed {
                bytes: FILE_SIZE,
                http_status: 200
            }
        );
        assert_eq!(
            fin(n, 404, 0),
            UploadStatus::Failed(UploadFailure::Http(404))
        );
        assert_eq!(
            fin(10, 200, 0),
            UploadStatus::Failed(UploadFailure::SizeMismatch {
                sent: 10,
                expected: FILE_SIZE
            })
        );
        assert_eq!(fin(n, -1, 0), UploadStatus::Failed(UploadFailure::Other));
    }

    #[test]
    fn redirect_is_never_success() {
        assert_eq!(
            fin(FILE_SIZE as i64, 200, 1),
            UploadStatus::Failed(UploadFailure::RedirectNotAllowed)
        );
        assert_eq!(
            fin(FILE_SIZE as i64, 302, 1),
            UploadStatus::Failed(UploadFailure::RedirectNotAllowed)
        );
    }

    #[test]
    fn network_and_file_errors() {
        assert_eq!(fin(0, -1, 2), UploadStatus::Failed(UploadFailure::Network));
        assert_eq!(
            fin(0, -1, 3),
            UploadStatus::Failed(UploadFailure::FileUnavailable)
        );
        assert_eq!(fin(0, -1, 4), UploadStatus::Failed(UploadFailure::Other));
        assert_eq!(fin(0, -1, 99), UploadStatus::Failed(UploadFailure::Other));
    }

    #[test]
    fn waiting_sending_cancelled() {
        let st = |v: [i64; 5]| to_status(decode(&v).unwrap());
        assert_eq!(st([0, 0, 10, -1, 0]), UploadStatus::Idle);
        assert_eq!(st([1, 0, 10, -1, 0]), UploadStatus::Waiting);
        assert_eq!(
            st([2, 4, 10, -1, 0]),
            UploadStatus::Sending {
                sent: 4,
                total: Some(10)
            }
        );
        assert_eq!(st([4, 0, 10, -1, 0]), UploadStatus::Cancelled);
    }

    #[test]
    fn start_code_mapping() {
        assert!(decode_start(0).is_ok());
        assert!(matches!(
            decode_start(2),
            Err(DeviceError::Internal(m)) if m == fandhe_device::background_upload::FILE_ERROR
        ));
        assert!(decode_start(3).is_err());
        assert!(decode_start(7).is_err());
    }
}
