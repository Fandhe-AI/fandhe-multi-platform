//! Android 向けバックグラウンドダウンロード実装（Issue #392、親 #100 の b-android）。
//! `device/background_download.rs::implementation` の Android 経路から呼ばれ、ドライバのワーカー
//! スレッド（`start` / `cancel`）と egui スレッド（`status`）から `MainActivity.backgroundDownload*`
//! を同期 JNI で呼ぶ pull 型（`android_compass.rs` と同形。要求 id・`RequestSlot`・Java からの
//! native コールバックは使わない）。Java 側は `BackgroundDownloadHelper`（`DownloadManager`。GMS 非依存）。
//!
//! # 結果コード（SSOT は Java `BackgroundDownloadHelper`）
//!
//! - `backgroundDownloadStart(String) -> int`: 0 = OK / 1 = DownloadManager 無し（`Unsupported`）/ 2 = 開始失敗
//! - `backgroundDownloadCancel() -> int`: 0 = OK / 2 = 失敗
//! - `backgroundDownloadStatus() -> long[4]`: `{kind, received, total, reason}`。例外では `null`。
//!   kind 0 = id 無し / 1 = 行が無い / 2 = 転送中 / 3 = 成功 / 4 = 失敗。total 不明は -1。
//!   成功時の received は保存先ファイルの実長（0 なら Rust 側で `Failed`）
//!
//! JNI 署名は Java のメソッドと本ファイルの `jni_sig!` の 2 か所で一致させる（native コールバックは
//! 足さないので no-op スタブは要らない）。
//!
//! # ライフサイクル（契約表 1）
//!
//! 転送の持ち主は OS で、Activity の再生成・プロセス終了をまたいで続くのが目的のため
//! `onDestroy` では取り消さない。Java 側に保留中の要求は無く、照会は常に `SharedPreferences` の
//! 現在の id だけなので、古い転送の結果が新しい転送を消費しない。Rust 側の `Local` は直近の
//! 操作だけで、再起動で消える。再生成の途中で JNI が失敗したときは `Err` を返し成功にしない。
//!
//! # 並行性（契約表 2・11）
//!
//! `start` / `cancel` / `status` の JNI 呼び出しは `Local` の `Mutex` の内側で直列化する
//! （判定と更新を同じロックの内側で行う）。Java はコールバックしないのでデッドロックしない。
//!
//! 守る範囲: 1 プロセス内の開始・取り消し・状態の順序、嘘の成功の禁止（0 バイト・HTTP エラー・
//! 行が消えた転送を成功にしない）、開始失敗と取り消しでファイルと id を残さない。
//! 守らない範囲: 再起動後に一度も転送中を観測しないまま終わった転送の完了・失敗表示（`Idle` になる。転送中を観測すれば追跡中として記録し終端状態も表示する）、リダイレクト先の再検証（OS が追う）、
//! `check_url` が通す私設 IPv4 への平文（network security config は 127.0.0.1 と localhost だけ許すので
//! OS 側の失敗になり、「HTTP 400」と出ることがある）、接続できない間の転送中表示（PAUSED）、
//! 複数プロセス、ポーリング間隔ぶんの遅れ。URL・パス・OS の生の文言は出さない（A09）。

#[cfg(any(target_os = "android", test))]
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::Mutex;

#[cfg(all(not(target_os = "android"), test))]
use fandhe_device::background_download::UnsupportedBackgroundDownload;
#[cfg(any(target_os = "android", test))]
use fandhe_device::background_download::{completion_status, BackgroundDownload, DownloadStatus};

#[cfg(any(target_os = "android", test))]
const FAILED_NETWORK: &str = "接続できませんでした";
#[cfg(any(target_os = "android", test))]
const FAILED_SAVE: &str = "ファイルを保存できませんでした";
#[cfg(any(target_os = "android", test))]
const FAILED_OTHER: &str = "ダウンロードに失敗しました";

// `DownloadManager.ERROR_*`（1000 番台）。400〜599 は HTTP の状態コードそのもの。
#[cfg(any(target_os = "android", test))]
const ERROR_FILE_ERROR: i64 = 1001;
#[cfg(any(target_os = "android", test))]
const ERROR_HTTP_DATA_ERROR: i64 = 1004;
#[cfg(any(target_os = "android", test))]
const ERROR_INSUFFICIENT_SPACE: i64 = 1006;
#[cfg(any(target_os = "android", test))]
const ERROR_DEVICE_NOT_FOUND: i64 = 1007;
#[cfg(any(target_os = "android", test))]
const ERROR_FILE_ALREADY_EXISTS: i64 = 1009;

/// Java が返す `status` の種別。
#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    None,
    Gone,
    Running,
    Success,
    Failed,
}

/// `backgroundDownloadStatus` の戻りを読んだ値。
#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Raw {
    kind: Kind,
    received: u64,
    total: Option<u64>,
    reason: i64,
}

/// このプロセスで最後に行った操作（再起動で消える）。
#[cfg(any(target_os = "android", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Local {
    Untouched,
    Started,
    Cancelled,
}

/// `long[4]` を `Raw` へ。長さ違い・未知の kind・負の received・-1 未満の total は `None`。
#[cfg(any(target_os = "android", test))]
fn decode(v: &[i64]) -> Option<Raw> {
    let [kind, received, total, reason] = <[i64; 4]>::try_from(v).ok()?;
    let kind = match kind {
        0 => Kind::None,
        1 => Kind::Gone,
        2 => Kind::Running,
        3 => Kind::Success,
        4 => Kind::Failed,
        _ => return None,
    };
    Some(Raw {
        kind,
        received: u64::try_from(received).ok()?,
        total: match total {
            -1 => None,
            t => Some(u64::try_from(t).ok()?),
        },
        reason,
    })
}

/// 失敗の理由コードを固定文言へ写す（A09: 生の文言を出さない）。
#[cfg(any(target_os = "android", test))]
fn failure_status(reason: i64) -> DownloadStatus {
    match reason {
        400..=599 => completion_status(Some(reason as u16), 0),
        ERROR_INSUFFICIENT_SPACE
        | ERROR_FILE_ERROR
        | ERROR_DEVICE_NOT_FOUND
        | ERROR_FILE_ALREADY_EXISTS => DownloadStatus::Failed(FAILED_SAVE.to_string()),
        ERROR_HTTP_DATA_ERROR => DownloadStatus::Failed(FAILED_NETWORK.to_string()),
        _ => DownloadStatus::Failed(FAILED_OTHER.to_string()),
    }
}

/// 転送中を観測したら追跡中（`Started`）として記録する。再起動後に再接続した転送の
/// 完了・失敗を、以後の `decide` で `Idle` に落とさず表示するため。
#[cfg(any(target_os = "android", test))]
fn adopt(raw: Raw, local: Local) -> Local {
    if raw.kind == Kind::Running && local == Local::Untouched {
        Local::Started
    } else {
        local
    }
}

/// OS の状態と直近の操作から表示する状態を決める純関数。
#[cfg(any(target_os = "android", test))]
fn decide(raw: Raw, local: Local) -> DownloadStatus {
    let running = DownloadStatus::Running {
        received: raw.received,
        total: raw.total,
    };
    match (raw.kind, local) {
        (Kind::Running, _) => running,
        (Kind::None, Local::Cancelled) | (Kind::Gone, _) if local != Local::Untouched => {
            DownloadStatus::Cancelled
        }
        (Kind::Success, Local::Started) => completion_status(None, raw.received),
        (Kind::Failed, Local::Started) => failure_status(raw.reason),
        (Kind::Success | Kind::Failed, Local::Cancelled) => DownloadStatus::Cancelled,
        // 未操作（再起動後を含む）の完了・失敗・行なしは、再起動で失われる表示として `Idle`。
        _ => DownloadStatus::Idle,
    }
}

#[cfg(target_os = "android")]
struct AndroidBackgroundDownload {
    local: Mutex<Local>,
}

/// `MainActivity` の同期メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`。
/// 2 箇所の `unsafe` は `android_compass.rs::call_activity` と同一の根拠。
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
impl BackgroundDownload for AndroidBackgroundDownload {
    fn start(&self, url: &str) -> Result<(), fandhe_device::DeviceError> {
        use fandhe_device::DeviceError;
        fandhe_device::http::check_url(url)?;
        let mut local = self.local.lock().unwrap_or_else(|e| e.into_inner());
        let code = call_activity(|env, a| {
            let url = env.new_string(url)?;
            Ok(env
                .call_method(
                    a,
                    jni::jni_str!("backgroundDownloadStart"),
                    jni::jni_sig!((java.lang.String) -> int),
                    &[jni::objects::JValue::Object(&url)],
                )?
                .i()?)
        })
        .ok_or(DeviceError::Internal("開始できませんでした"))?;
        match code {
            0 => {
                *local = Local::Started;
                Ok(())
            }
            1 => Err(DeviceError::Unsupported("background download")),
            _ => Err(DeviceError::Internal("開始できませんでした")),
        }
    }

    fn cancel(&self) -> Result<(), fandhe_device::DeviceError> {
        use fandhe_device::DeviceError;
        let mut local = self.local.lock().unwrap_or_else(|e| e.into_inner());
        let code = call_activity(|env, a| {
            Ok(env
                .call_method(
                    a,
                    jni::jni_str!("backgroundDownloadCancel"),
                    jni::jni_sig!(() -> int),
                    &[],
                )?
                .i()?)
        })
        .ok_or(DeviceError::Internal("取り消しできませんでした"))?;
        if code != 0 {
            return Err(DeviceError::Internal("取り消しできませんでした"));
        }
        *local = Local::Cancelled;
        Ok(())
    }

    fn status(&self) -> Result<DownloadStatus, fandhe_device::DeviceError> {
        use fandhe_device::DeviceError;
        use jni::objects::JLongArray;
        let mut local = self.local.lock().unwrap_or_else(|e| e.into_inner());
        let values = call_activity(|env, a| {
            let ret = env.call_method(
                a,
                jni::jni_str!("backgroundDownloadStatus"),
                jni::jni_sig!(() -> long[]),
                &[],
            )?;
            let arr = env.cast_local::<JLongArray>(ret.l()?)?;
            if arr.is_null() {
                return Ok(None);
            }
            // 長さを検証してから読む。
            let n = arr.len(env)?;
            if n != 4 {
                return Ok(None);
            }
            let mut v = [0i64; 4];
            arr.get_region(env, 0, &mut v)?;
            Ok(Some(v))
        })
        .flatten();
        let raw = values
            .as_ref()
            .and_then(|v| decode(v))
            .ok_or(DeviceError::Internal("状態を読めませんでした"))?;
        *local = adopt(raw, *local);
        Ok(decide(raw, *local))
    }
}

/// Android の実装を返す。非 Android のテストビルドは `Unsupported` 固定（dispatch は Android でしか呼ばない）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    Arc::new(AndroidBackgroundDownload {
        local: Mutex::new(Local::Untouched),
    })
}

/// ホストのテストビルドにも載るための形。
#[cfg(all(not(target_os = "android"), test))]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    Arc::new(UnsupportedBackgroundDownload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(kind: Kind, received: u64, reason: i64) -> Raw {
        Raw {
            kind,
            received,
            total: Some(10),
            reason,
        }
    }

    #[test]
    fn decode_rejects_malformed() {
        assert_eq!(decode(&[2, 1, 5]), None);
        assert_eq!(decode(&[2, 1, 5, 0, 0]), None);
        assert_eq!(decode(&[9, 0, 0, 0]), None);
        assert_eq!(decode(&[2, -1, 5, 0]), None);
        assert_eq!(decode(&[2, 1, -2, 0]), None);
        let r = decode(&[2, 3, -1, 0]).unwrap();
        assert_eq!((r.kind, r.received, r.total), (Kind::Running, 3, None));
    }

    #[test]
    fn failure_mapping() {
        assert_eq!(
            failure_status(404),
            DownloadStatus::Failed("HTTP 404".into())
        );
        assert_eq!(
            failure_status(500),
            DownloadStatus::Failed("HTTP 500".into())
        );
        assert_eq!(
            failure_status(1006),
            DownloadStatus::Failed(FAILED_SAVE.into())
        );
        assert_eq!(
            failure_status(1001),
            DownloadStatus::Failed(FAILED_SAVE.into())
        );
        assert_eq!(
            failure_status(1004),
            DownloadStatus::Failed(FAILED_NETWORK.into())
        );
        assert_eq!(
            failure_status(1002),
            DownloadStatus::Failed(FAILED_OTHER.into())
        );
        assert_eq!(
            failure_status(0),
            DownloadStatus::Failed(FAILED_OTHER.into())
        );
    }

    #[test]
    fn decide_table() {
        use Kind::*;
        use Local::*;
        let run = DownloadStatus::Running {
            received: 4,
            total: Some(10),
        };
        for l in [Started, Cancelled, Untouched] {
            assert_eq!(decide(raw(Running, 4, 0), l), run);
        }
        assert_eq!(decide(raw(None, 0, 0), Started), DownloadStatus::Idle);
        assert_eq!(
            decide(raw(None, 0, 0), Cancelled),
            DownloadStatus::Cancelled
        );
        assert_eq!(decide(raw(None, 0, 0), Untouched), DownloadStatus::Idle);
        assert_eq!(decide(raw(Gone, 0, 0), Started), DownloadStatus::Cancelled);
        assert_eq!(
            decide(raw(Gone, 0, 0), Cancelled),
            DownloadStatus::Cancelled
        );
        assert_eq!(decide(raw(Gone, 0, 0), Untouched), DownloadStatus::Idle);
        assert_eq!(
            decide(raw(Success, 7, 0), Started),
            DownloadStatus::Completed { bytes: 7 }
        );
        assert_eq!(decide(raw(Success, 7, 0), Untouched), DownloadStatus::Idle);
        assert_eq!(decide(raw(Failed, 0, 404), Untouched), DownloadStatus::Idle);
        assert_eq!(
            decide(raw(Failed, 0, 404), Started),
            DownloadStatus::Failed("HTTP 404".into())
        );
    }

    #[test]
    fn adopt_tracks_running_after_restart() {
        let l = adopt(raw(Kind::Running, 1, 0), Local::Untouched);
        assert_eq!(l, Local::Started);
        assert_eq!(
            decide(raw(Kind::Success, 7, 0), l),
            DownloadStatus::Completed { bytes: 7 }
        );
        assert_eq!(
            adopt(raw(Kind::Running, 1, 0), Local::Cancelled),
            Local::Cancelled
        );
        assert_eq!(
            adopt(raw(Kind::Success, 1, 0), Local::Untouched),
            Local::Untouched
        );
    }

    #[test]
    fn zero_byte_success_is_failed() {
        assert!(matches!(
            decide(raw(Kind::Success, 0, 0), Local::Started),
            DownloadStatus::Failed(_)
        ));
    }
}
