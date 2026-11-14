//! Android 向け行動認識実装（Issue #1127、親 #124。骨格は #1126）。`driver` の
//! Android 経路から egui スレッドで呼ばれ、`ActivityRecognitionHelper.start(Activity)` /
//! `read(Activity)` / `stop(Activity)`（static）を同期 JNI で呼んで最新値を pull する
//! （継続配信の pull 型。要求 id・`RequestSlot` は使わない）。Java 側は `ActivityRecognitionHelper`
//! （Play services `ActivityRecognitionClient`、結果は `ActivityRecognitionReceiver` 経由）。
//!
//! # 結果コード（SSOT は Java `ActivityRecognitionHelper`）
//!
//! - `start`: 0 = OK / 1 = 非対応（API 28 以下・GMS 不可）/ それ以外 = `Failed`
//! - `read`: `long[]` で `[status, seq, type0, conf0, type1, conf1, ...]`。
//!   status 0 = 待機 / 1 = 値あり / 2 = 許可拒否 / 3 = 失敗 / 4 = 許可ダイアログ待ち / 負値 = 購読喪失
//!   （Activity 再生成など。`Failed`）
//!
//! 種別・確度の整数コードの写像は `crate::contract::classify_detected` が正で、
//! Java は生のコードを渡すだけ。範囲外・未知・空は捨てて待機にし、静止へ畳まない。
//!
//! # ライフサイクル
//!
//! Java が `onStop` / `onStart` で購読を解除・張り直し、`nativeOnBackgrounded` / `nativeOnResumed` で
//! driver の期限を止める・引き直す。`onDestroy` で購読を破棄し、新しい Activity の read は負値を返すので
//! `Failed` になり driver が `stop` して失敗表示にする。
//!
//! 守る範囲: 嘘の値を出さない・二重に購読しない・停止 / 背景 / 破棄で購読を解除する。
//! 守らない範囲: GMS の配信間隔と遅延、背景中の受信、過去の履歴、作り直し後の失敗表示。
//! 行動種別・確度はログに出さない。実機確認は #603。

use std::sync::Arc;

use crate::contract::{
    ActivityFailure, ActivityPoll, ActivityReading, ActivityRecognizer, classify_detected,
};

const START_OK: i32 = 0;
const START_UNSUPPORTED: i32 = 1;

const ST_WAITING: i64 = 0;
const ST_READING: i64 = 1;
const ST_DENIED: i64 = 2;
const ST_AWAITING: i64 = 4;

/// Java が返す配列の長さの上限（`status` + `seq` + 組 16 個）。超えたら不正として `Failed`。
const MAX_READ_LEN: usize = 2 + 2 * 16;

/// `activityRecognitionStart` の結果コードを契約の結果へ写像する。未知のコードは `Failed`。
fn decode_start(code: i32) -> Result<(), ActivityFailure> {
    match code {
        START_OK => Ok(()),
        START_UNSUPPORTED => Err(ActivityFailure::Unsupported),
        _ => Err(ActivityFailure::Failed),
    }
}

/// `activityRecognitionRead` の配列を契約の結果へ写像する。長さ不正・購読喪失・未知の status は
/// `Failed`。値ありでも無効な組（範囲外・未知の種別・空）は `Waiting`（静止にしない）。
fn decode_read(raw: &[i64]) -> Result<ActivityPoll, ActivityFailure> {
    if raw.len() < 2 || raw.len() > MAX_READ_LEN || !raw.len().is_multiple_of(2) {
        return Err(ActivityFailure::Failed);
    }
    match raw[0] {
        ST_WAITING => Ok(ActivityPoll::Waiting),
        ST_AWAITING => Ok(ActivityPoll::AwaitingPermission),
        ST_DENIED => Err(ActivityFailure::PermissionDenied),
        ST_READING => Ok(decode_reading(raw[1], &raw[2..])),
        _ => Err(ActivityFailure::Failed),
    }
}

/// 値ありの配列を [`ActivityPoll`] へ。i32 に収まらない要素があればサンプルごと捨てる。
fn decode_reading(seq: i64, pairs: &[i64]) -> ActivityPoll {
    let Ok(seq) = u64::try_from(seq) else {
        return ActivityPoll::Waiting;
    };
    let mut probable = Vec::with_capacity(pairs.len() / 2);
    for p in pairs.as_chunks::<2>().0 {
        match (i32::try_from(p[0]), i32::try_from(p[1])) {
            (Ok(t), Ok(c)) => probable.push((t, c)),
            _ => return ActivityPoll::Waiting,
        }
    }
    match classify_detected(&probable) {
        Some((kind, confidence)) => ActivityPoll::Reading(ActivityReading {
            kind,
            confidence,
            seq,
        }),
        None => ActivityPoll::Waiting,
    }
}

#[cfg(target_os = "android")]
struct AndroidActivityRecognizer;

/// `ActivityRecognitionHelper` の static メソッド（第 1 引数は Activity）を同期 JNI で呼ぶ。
/// Activity 未確立・JNI 失敗は `None`。JavaVM と Activity の取得は
/// `fandhe_platform_android::with_activity_class` に任せ、ここは `unsafe` を持たない。
#[cfg(target_os = "android")]
fn call_activity<T>(
    f: impl FnOnce(
        &mut jni::Env,
        &jni::objects::JObject,
        &jni::objects::JClass,
    ) -> Result<T, fandhe_platform_android::JniCallFailed>,
) -> Option<T> {
    use fandhe_platform_android::{android_app, with_activity_class};
    let app = android_app()?;
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ActivityRecognitionHelper"),
        f,
    )
    .ok()
}

#[cfg(target_os = "android")]
impl ActivityRecognizer for AndroidActivityRecognizer {
    fn start(&self) -> Result<(), ActivityFailure> {
        let code = call_activity(|env, a, class| {
            Ok(env
                .call_static_method(
                    class,
                    jni::jni_str!("start"),
                    jni::jni_sig!((android.app.Activity) -> int),
                    &[jni::objects::JValue::Object(a)],
                )?
                .i()?)
        })
        .ok_or(ActivityFailure::Failed)?;
        decode_start(code)
    }

    fn read(&self) -> Result<ActivityPoll, ActivityFailure> {
        let values = call_activity(|env, a, class| {
            use jni::objects::JLongArray;
            let ret = env.call_static_method(
                class,
                jni::jni_str!("read"),
                jni::jni_sig!((android.app.Activity) -> long[]),
                &[jni::objects::JValue::Object(a)],
            )?;
            let arr = env.cast_local::<JLongArray>(ret.l()?)?;
            if arr.is_null() {
                return Ok(None);
            }
            // 長さを検証してから読む。
            let len = arr.len(env)?;
            if !(2..=MAX_READ_LEN).contains(&len) {
                return Ok(None);
            }
            let mut v = vec![0i64; len];
            arr.get_region(env, 0, &mut v)?;
            Ok(Some(v))
        })
        .flatten()
        .ok_or(ActivityFailure::Failed)?;
        decode_read(&values)
    }

    fn stop(&self) {
        let _ = call_activity(|env, a, class| {
            env.call_static_method(
                class,
                jni::jni_str!("stop"),
                jni::jni_sig!((android.app.Activity) -> ()),
                &[jni::objects::JValue::Object(a)],
            )?;
            Ok(())
        });
    }
}

/// Android の `ActivityRecognizer` 実装を返す（`driver` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn ActivityRecognizer> {
    Arc::new(AndroidActivityRecognizer)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn ActivityRecognizer> {
    Arc::new(crate::contract::UnsupportedActivityRecognizer)
}

/// 背景遷移の通知（Java `ActivityRecognitionHelper.onStop` から、Rust が start を呼んだ後だけ呼ばれる）。
/// 背景中は driver が最初の 1 件の期限を進めない。atomics を書くだけでロックは取らない。
/// シンボル名は JNI 命名規約上の固定名。署名 `()V` は Java 宣言と一致させる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じる。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ActivityRecognitionHelper_nativeOnBackgrounded(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
    crate::driver::note_backgrounded();
}

/// 前景復帰の通知（Java `ActivityRecognitionHelper.onStart`）。期限の起点を引き直し、次フレームを起こす。
///
/// # Safety
/// [`Java_com_fandhe_platform_ActivityRecognitionHelper_nativeOnBackgrounded`] と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_ActivityRecognitionHelper_nativeOnResumed(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
    crate::driver::note_resumed();
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ActivityKind, Confidence};

    #[test]
    fn host_never_succeeds() {
        let r = implementation();
        assert_eq!(r.start(), Err(ActivityFailure::Unsupported));
        assert_eq!(r.read(), Err(ActivityFailure::Unsupported));
    }

    #[test]
    fn start_codes() {
        assert_eq!(decode_start(0), Ok(()));
        assert_eq!(decode_start(1), Err(ActivityFailure::Unsupported));
        assert_eq!(decode_start(2), Err(ActivityFailure::Failed));
        assert_eq!(decode_start(9), Err(ActivityFailure::Failed));
    }

    #[test]
    fn read_status_mapping() {
        assert_eq!(decode_read(&[0, 0]), Ok(ActivityPoll::Waiting));
        assert_eq!(decode_read(&[4, 0]), Ok(ActivityPoll::AwaitingPermission));
        assert_eq!(decode_read(&[2, 0]), Err(ActivityFailure::PermissionDenied));
        assert_eq!(decode_read(&[3, 0]), Err(ActivityFailure::Failed));
        assert_eq!(decode_read(&[-1, 0]), Err(ActivityFailure::Failed));
        assert_eq!(decode_read(&[7, 0]), Err(ActivityFailure::Failed));
    }

    #[test]
    fn read_rejects_bad_shape() {
        assert_eq!(decode_read(&[]), Err(ActivityFailure::Failed));
        assert_eq!(decode_read(&[1]), Err(ActivityFailure::Failed));
        // 組の部分が奇数長。
        assert_eq!(decode_read(&[1, 1, 3]), Err(ActivityFailure::Failed));
        assert_eq!(
            decode_read(&vec![0i64; MAX_READ_LEN + 2]),
            Err(ActivityFailure::Failed)
        );
    }

    #[test]
    fn reading_never_becomes_stationary_on_bad_sample() {
        // 組なし・確度が範囲外・未知の種別・i32 範囲外はすべて待機。
        assert_eq!(decode_read(&[1, 1]), Ok(ActivityPoll::Waiting));
        assert_eq!(decode_read(&[1, 1, 3, 101]), Ok(ActivityPoll::Waiting));
        assert_eq!(decode_read(&[1, 1, 99, 80]), Ok(ActivityPoll::Waiting));
        assert_eq!(
            decode_read(&[1, 1, i64::from(i32::MAX) + 1, 80]),
            Ok(ActivityPoll::Waiting)
        );
        assert_eq!(decode_read(&[1, -1, 3, 80]), Ok(ActivityPoll::Waiting));
    }

    #[test]
    fn reading_passes_seq_and_classifies() {
        // DetectedActivity.WALKING = 7、確度 90。
        let Ok(ActivityPoll::Reading(r)) = decode_read(&[1, 42, 7, 90]) else {
            panic!("reading expected");
        };
        assert_eq!(r.kind, ActivityKind::Walking);
        assert_eq!(r.confidence, Confidence::High);
        assert_eq!(r.seq, 42);
    }
}
