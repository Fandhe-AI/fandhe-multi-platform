//! Android 向け「接続したスキャナー・カメラ機器からの取り込み」実装（Issue #983、親 #192）。
//! `device/scanner_devices.rs::platform` の Android 経路から [`implementation`] が呼ばれ、
//! Java 側 `ScannerDevicesHelper`（`UsbManager` + `MtpDevice`）を JNI で橋渡しする。
//! 契約の SSOT は `fandhe_device::scanner_devices`。
//!
//! 流れ:
//! - ワーカーが [`import_with`] で `MainActivity.requestScannerDevicesImport(long, long, int)` を呼び、
//!   Java が `nativeOnScannerDevicesResult(id, code, found, jpeg)` で返した値を [`decode`] で契約の結果へ写す
//! - 期限（呼び出し元から渡る残り時間、起点は単調時計）が切れたら `cancelScannerDevicesImport(long)` を
//!   呼び、Java が MTP セッション・接続・レシーバーを解放した後の最終通知を待ってから `TimedOut` を返す
//!   （解除を post しただけで戻らない。期限の後に届いた成功は成功にしない）
//!
//! 結果コードの SSOT は Java 側 `ScannerDevicesHelper.CODE_*`。ロック中の機器は判別できないため
//! `DeviceLocked` は返さず `Failed`。USB 許可ダイアログを閉じた操作は `PermissionDenied`。
//! 機器が外れたことは broadcast ではなく Java が `getDeviceList` を読み直して判定し、`Interrupted` に写す
//! （trait の写像表の `ACTION_USB_DEVICE_DETACHED` と同じ状況を、OS の状態を正にして観測する）。
//!
//! 守る範囲: 1 プロセス内の要求順序・失敗を成功に畳まないこと・資源の解放・Activity 再生成時の保留中の要求の
//! 破棄（Java の `onDestroy` が `Interrupted` で確定し、Rust の [`reset_pending_tracking`] が待機中の
//! ワーカーを失敗で戻す）。守らない範囲: 探索窓の後に現れた機器、複数台のうち先頭以外、他アプリが同じ機器を
//! 開いていることによる失敗（`Failed`）、期限の後にダイアログで与えられた許可（OS に残る）。
//! 機器名・画像・id・コードはログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestScannerDevicesImport` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（`android_radio_control` と同じ）。

use std::time::{Duration, Instant};

use fandhe_device::scanner_devices::{DeviceKind, ImportedImage, ScannerImportFailure};

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 結果コード（SSOT は Java 側 `ScannerDevicesHelper.CODE_*`）。
const CODE_OK: i32 = 0;
const CODE_UNSUPPORTED: i32 = 1;
const CODE_NO_DEVICE: i32 = 2;
const CODE_NO_IMAGE: i32 = 3;
const CODE_PERMISSION_DENIED: i32 = 4;
const CODE_INTERRUPTED: i32 = 5;
const CODE_TOO_LARGE: i32 = 6;
const CODE_UNDECODABLE: i32 = 7;
const CODE_FAILED: i32 = 8;
const CODE_CANCELLED: i32 = 9;

/// Java から届く 1 件ぶんの結果（コード・見つかった台数・JPEG）。
type Reply = (i32, u32, Option<Vec<u8>>);

/// 結果コードを契約の結果へ写す。OK でも JPEG が無ければ `Failed`、未知のコードと
/// `CODE_CANCELLED`（Rust が取り消した要求の最終通知）は `Failed`（嘘の成功にしない）。
fn decode(
    code: i32,
    found: u32,
    jpeg: Option<Vec<u8>>,
) -> Result<ImportedImage, ScannerImportFailure> {
    match code {
        CODE_OK => match jpeg {
            Some(bytes) => ImportedImage::from_os(bytes, DeviceKind::Camera, found),
            None => Err(ScannerImportFailure::Failed),
        },
        CODE_UNSUPPORTED => Err(ScannerImportFailure::Unsupported),
        CODE_NO_DEVICE => Err(ScannerImportFailure::NoDevice),
        CODE_NO_IMAGE => Err(ScannerImportFailure::NoImage),
        CODE_PERMISSION_DENIED => Err(ScannerImportFailure::PermissionDenied),
        CODE_INTERRUPTED => Err(ScannerImportFailure::Interrupted),
        CODE_TOO_LARGE => Err(ScannerImportFailure::TooLarge),
        CODE_UNDECODABLE => Err(ScannerImportFailure::Undecodable),
        CODE_FAILED | CODE_CANCELLED => Err(ScannerImportFailure::Failed),
        _ => Err(ScannerImportFailure::Failed),
    }
}

/// 要求 1 件を出して結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）。
/// `remaining` を過ぎたら `cancel` を 1 回呼び、Java が解放を済ませた最終通知を待って `TimedOut` を返す。
/// reset や差し替えで slot が他の要求のものになったら、その slot は消さずに戻る。
fn import_with(
    slot: &RequestSlot<Reply>,
    remaining: Duration,
    start: impl FnOnce(u64) -> bool,
    cancel: impl FnOnce(u64) -> bool,
) -> Result<ImportedImage, ScannerImportFailure> {
    let begin = Instant::now();
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(ScannerImportFailure::Failed);
    }
    let mut cancel = Some(cancel);
    let mut timed_out = false;
    loop {
        match slot.snapshot() {
            Some((cur, Some((code, found, jpeg)))) if cur == id => {
                slot.clear(id);
                // 期限の後に届いた結果は、成功でも成功として受け取らない。
                return if timed_out {
                    Err(ScannerImportFailure::TimedOut)
                } else {
                    decode(code, found, jpeg)
                };
            }
            Some((cur, None)) if cur == id => {
                if !timed_out && begin.elapsed() >= remaining {
                    timed_out = true;
                    if let Some(cancel) = cancel.take() {
                        // ponytail: JNI が失敗したら解放の完了を待てない（天井）。待たずに戻す。
                        if !cancel(id) {
                            slot.clear(id);
                            return Err(ScannerImportFailure::TimedOut);
                        }
                    }
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            _ => {
                return Err(if timed_out {
                    ScannerImportFailure::TimedOut
                } else {
                    ScannerImportFailure::Failed
                })
            }
        }
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Reply> = RequestSlot::new();

/// Activity 再生成直後に `android::reset_pending_tracking` から呼ぶ。保留中の要求を捨て、
/// 待機中のワーカーを失敗で戻し、旧要求の遅延完了を id 不一致で無視させる。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    SLOT.reset();
}

/// Android の `ScannerDevices` 実装。
#[cfg(target_os = "android")]
struct AndroidScannerDevices;

#[cfg(target_os = "android")]
impl fandhe_device::scanner_devices::ScannerDevices for AndroidScannerDevices {
    fn import_one(&self, remaining: Duration) -> Result<ImportedImage, ScannerImportFailure> {
        use fandhe_device::scanner_devices::{DISCOVERY_WINDOW, MAX_PAGE_BYTES};
        let window_ms = DISCOVERY_WINDOW.as_millis() as i64;
        let max_bytes = MAX_PAGE_BYTES.min(i32::MAX as usize) as i32;
        import_with(
            &SLOT,
            remaining,
            |id| call_start(id, window_ms, max_bytes),
            call_cancel,
        )
    }
}

/// Android の実装を返す。`device/scanner_devices.rs` の dispatch は android の cfg 内でだけ呼ぶ。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::scanner_devices::ScannerDevices>
{
    std::sync::Arc::new(AndroidScannerDevices)
}

/// `MainActivity.requestScannerDevicesImport(long, long, int)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_radio_control.rs::call_set` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_start(id: u64, window_ms: i64, max_bytes: i32) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(
        |env: &mut jni::Env| -> Result<(), super::android::JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestScannerDevicesImport"),
                jni::jni_sig!((long, long, int) -> ()),
                &[
                    jni::objects::JValue::Long(id as jni::sys::jlong),
                    jni::objects::JValue::Long(window_ms),
                    jni::objects::JValue::Int(max_bytes),
                ],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `MainActivity.cancelScannerDevicesImport(long)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `call_start` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_cancel(id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `call_start` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(
        |env: &mut jni::Env| -> Result<(), super::android::JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `call_start` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("cancelScannerDevicesImport"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(id as jni::sys::jlong)],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `ScannerDevicesHelper.nativeOnScannerDevicesResult`（Java 側）から呼ばれる JNI エクスポート。
/// 旧要求の遅延到着は `deliver` が id 不一致で捨てる。JPEG は長さを `MAX_PAGE_BYTES` で検査してから
/// コピーし、超過は `CODE_TOO_LARGE` に差し替える。値はログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クラスは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_ScannerDevicesHelper_nativeOnScannerDevicesResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    found: jni::sys::jint,
    jpeg: jni::objects::JByteArray<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let mut code = code;
        let bytes = if jpeg.is_null() {
            None
        } else if jpeg.len(env)? > fandhe_device::scanner_devices::MAX_PAGE_BYTES {
            code = CODE_TOO_LARGE;
            None
        } else {
            Some(env.convert_byte_array(&jpeg)?)
        };
        SLOT.deliver(request_id as u64, (code, found.max(0) as u32, bytes));
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ScannerImportFailure as F;

    const PNG_HEAD: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    const SHORT: Duration = Duration::from_millis(50);
    const LONG: Duration = Duration::from_secs(30);

    fn ok_reply() -> Reply {
        (CODE_OK, 1, Some(PNG_HEAD.to_vec()))
    }

    #[test]
    fn decode_maps_every_code_and_never_folds_to_success() {
        assert!(decode(CODE_OK, 1, Some(PNG_HEAD.to_vec())).is_ok());
        let cases = [
            (CODE_UNSUPPORTED, F::Unsupported),
            (CODE_NO_DEVICE, F::NoDevice),
            (CODE_NO_IMAGE, F::NoImage),
            (CODE_PERMISSION_DENIED, F::PermissionDenied),
            (CODE_INTERRUPTED, F::Interrupted),
            (CODE_TOO_LARGE, F::TooLarge),
            (CODE_UNDECODABLE, F::Undecodable),
            (CODE_FAILED, F::Failed),
            (CODE_CANCELLED, F::Failed),
            (10, F::Failed),
            (99, F::Failed),
            (-1, F::Failed),
        ];
        for (code, want) in cases {
            assert_eq!(decode(code, 1, None).err(), Some(want), "code {code}");
        }
    }

    #[test]
    fn decode_ok_requires_image_and_device() {
        assert_eq!(decode(CODE_OK, 1, None).err(), Some(F::Failed));
        assert_eq!(
            decode(CODE_OK, 0, Some(PNG_HEAD.to_vec())).err(),
            Some(F::NoDevice)
        );
        assert_eq!(
            decode(CODE_OK, 1, Some(Vec::new())).err(),
            Some(F::Undecodable)
        );
    }

    #[test]
    fn import_fails_and_clears_when_start_fails() {
        let slot = RequestSlot::<Reply>::new();
        let r = import_with(&slot, LONG, |_| false, |_| true);
        assert_eq!(r.err(), Some(F::Failed));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn import_returns_delivered_result_and_clears() {
        let slot = RequestSlot::<Reply>::new();
        let r = import_with(
            &slot,
            LONG,
            |id| {
                slot.deliver(id, ok_reply());
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert!(r.is_ok());
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn import_maps_failure_codes() {
        let slot = RequestSlot::<Reply>::new();
        let r = import_with(
            &slot,
            LONG,
            |id| {
                slot.deliver(id, (CODE_PERMISSION_DENIED, 0, None));
                true
            },
            |_| true,
        );
        assert_eq!(r.err(), Some(F::PermissionDenied));
    }

    #[test]
    fn import_ignores_other_ids_and_fails_after_reset() {
        let slot = RequestSlot::<Reply>::new();
        let r = import_with(
            &slot,
            LONG,
            |id| {
                assert!(!slot.deliver(id + 1, ok_reply()));
                slot.reset();
                true
            },
            |_| true,
        );
        assert_eq!(r.err(), Some(F::Failed));
    }

    #[test]
    fn deadline_cancels_once_and_waits_for_final_notice() {
        let slot = RequestSlot::<Reply>::new();
        let mut calls = 0;
        let r = import_with(
            &slot,
            SHORT,
            |_| true,
            |id| {
                calls += 1;
                slot.deliver(id, (CODE_CANCELLED, 0, None));
                true
            },
        );
        assert_eq!(r.err(), Some(F::TimedOut));
        assert_eq!(calls, 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn success_after_deadline_is_not_accepted() {
        let slot = RequestSlot::<Reply>::new();
        let r = import_with(
            &slot,
            SHORT,
            |_| true,
            |id| {
                slot.deliver(id, ok_reply());
                true
            },
        );
        assert_eq!(r.err(), Some(F::TimedOut));
    }

    #[test]
    fn cancel_call_failure_returns_timed_out_immediately() {
        let slot = RequestSlot::<Reply>::new();
        let r = import_with(&slot, SHORT, |_| true, |_| false);
        assert_eq!(r.err(), Some(F::TimedOut));
        assert!(slot.snapshot().is_none());
    }
}
