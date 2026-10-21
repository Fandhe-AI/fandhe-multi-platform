//! Android 向け端末内翻訳（Issue #537、親 #115）。`device/translation.rs::platform` の
//! Android 経路から呼ばれ、`android.view.translation`（API 31 以上）で固定文を 1 回訳す。
//! Java 側は `TranslationHelper`（状態確認・Translator の生成と破棄・取り消し）。
//!
//! 流れ: `TextTranslator::translate`（ドライバのワーカースレッド）→ [`RequestSlot::begin`] で
//! id 採番 → JNI `MainActivity.requestTranslation(id, text, source, target)` → Java が結果コードと
//! 訳文を `nativeOnTranslationResult` で返す → [`decode`] で契約の失敗種別へ写像 → ワーカーが
//! 周期的に slot を見て受け取る。期限は引数 `remaining`（ドライバが 30 秒を渡す）だけで、
//! 切れたら `cancelTranslation(id)` で OS 側の翻訳を止めて `Internal(TIMEOUT)`。
//!
//! 写像（SSOT は `fandhe_device::translation`）: API 31 未満・翻訳サービスなしは `Unsupported`、
//! 言語の組が非対応は `LANGUAGE_UNSUPPORTED`、言語パック未導入・取得中は `NOT_INSTALLED`
//! （ダウンロードは始めない）、中断・OS の失敗は `Internal("translation")`、空の訳文は
//! ドライバの `check_result` が `EMPTY` にする。
//!
//! ライフサイクル: Activity 再生成（`onDestroy`）で保留中の要求を Java が中断扱いで確定し、
//! Translator を破棄する。`SLOT` の id は単調増加で、新しい `TranslationHelper` は未追跡から
//! 始まるため旧結果が新しい要求を消費しない。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・Translator の解放。守らない範囲: 期限後も
//! OS 側の翻訳が走り切ること、端末ごとの翻訳サービスの有無による結果の違い。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestTranslation` を経た後でしか
//! 呼ばれず、feature `device` 無効ビルドではその経路に到達しない（pedometer と同じ）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use fandhe_device::translation::{
    TextTranslator, EMPTY, LANGUAGE_UNSUPPORTED, NOT_INSTALLED, TIMEOUT,
};
use fandhe_device::DeviceError;

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 結果コード（SSOT は Java 側 `TranslationHelper.RESULT_*`）。
const RESULT_OK: i32 = 0;
const RESULT_UNSUPPORTED: i32 = 1;
const RESULT_LANGUAGE_UNSUPPORTED: i32 = 2;
const RESULT_NOT_INSTALLED: i32 = 3;
/// Activity 破棄による中断。Rust 側では未知のコードと同じ失敗へ写すため、値の記録とテスト用。
#[allow(dead_code)]
const RESULT_INTERRUPTED: i32 = 4;
const RESULT_FAILED: i32 = 5;

type Outcome = Result<String, DeviceError>;
/// slot に置く生の結果（結果コード, 訳文）。`DeviceError` は `Clone` でないため、写像は受け取る側で行う。
type Raw = (i32, Option<String>);

/// Java の結果コードと訳文を契約の結果へ写像する。空かどうかはドライバが判定する（trim しない）。
fn decode(code: i32, text: Option<String>) -> Outcome {
    match code {
        RESULT_OK => text.ok_or(DeviceError::Internal(EMPTY)),
        RESULT_UNSUPPORTED => Err(DeviceError::Unsupported("translation")),
        RESULT_LANGUAGE_UNSUPPORTED => Err(DeviceError::Internal(LANGUAGE_UNSUPPORTED)),
        RESULT_NOT_INSTALLED => Err(DeviceError::Internal(NOT_INSTALLED)),
        // 中断（RESULT_INTERRUPTED）・OS の失敗・未知のコード。
        _ => Err(DeviceError::Internal("translation")),
    }
}

/// 要求 1 件を出して期限まで結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）、
/// `cancel` は期限切れ・差し替え時に OS 側の翻訳を止めさせる。
fn translate_with(
    slot: &RequestSlot<Raw>,
    remaining: Duration,
    start: impl FnOnce(u64) -> bool,
    cancel: impl FnOnce(u64),
) -> Outcome {
    // 期限は呼び出し開始時点から数える（JNI 呼び出しの所要時間も含める）。
    let deadline = Instant::now() + remaining;
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(DeviceError::Internal("translation"));
    }
    loop {
        // 結果の受理より先に期限を確認する（期限後に届いた結果は捨てる）。
        let now = Instant::now();
        if now >= deadline {
            slot.clear(id);
            cancel(id);
            return Err(DeviceError::Internal(TIMEOUT));
        }
        match slot.snapshot() {
            Some((cur, Some(outcome))) if cur == id => {
                slot.clear(id);
                return decode(outcome.0, outcome.1);
            }
            Some((cur, None)) if cur == id => {}
            // 差し替え済み（他人の slot は消さない）。Java 側に残る翻訳は止めさせる。
            _ => {
                cancel(id);
                return Err(DeviceError::Internal("translation"));
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Raw> = RequestSlot::new();

#[cfg(target_os = "android")]
struct AndroidTextTranslator;

#[cfg(target_os = "android")]
impl TextTranslator for AndroidTextTranslator {
    fn translate(&self, text: &str, source: &str, target: &str, remaining: Duration) -> Outcome {
        translate_with(
            &SLOT,
            remaining,
            |id| call_request(id, text, source, target),
            call_cancel,
        )
    }
}

/// `MainActivity.requestTranslation(long, String, String, String)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `device/android_pedometer.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_request(id: u64, text: &str, source: &str, target: &str) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_text = env.new_string(text)?;
        let j_source = env.new_string(source)?;
        let j_target = env.new_string(target)?;
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestTranslation"),
            jni::jni_sig!((long, java.lang.String, java.lang.String, java.lang.String) -> ()),
            &[
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Object(&j_text),
                jni::objects::JValue::Object(&j_source),
                jni::objects::JValue::Object(&j_target),
            ],
        )?;
        Ok(())
    })
    .is_ok()
}

/// `MainActivity.cancelTranslation(long)` を JNI で呼ぶ。失敗しても期限切れの確定は変わらない。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_cancel(id: u64) {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return;
    };
    // SAFETY: `call_request` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let _ = vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: `call_request` と同一の根拠。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("cancelTranslation"),
            jni::jni_sig!((long) -> ()),
            &[jni::objects::JValue::Long(id as jni::sys::jlong)],
        )?;
        Ok(())
    });
}

/// `TranslationHelper.nativeOnTranslationResult`（Java 側）から呼ばれる JNI エクスポート。
/// 原文・訳文は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_TranslationHelper_nativeOnTranslationResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    text: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let text = if text.is_null() {
            None
        } else {
            Some(text.try_to_string(env)?)
        };
        SLOT.deliver(request_id as u64, (code, text));
        Ok(())
    });
    if !matches!(outcome.into_outcome(), jni::Outcome::Ok(_)) {
        // JNI 例外などで訳文を読めなかった。要求を放置せず失敗で確定する。
        SLOT.deliver(request_id as u64, (RESULT_FAILED, None));
    }
}

/// Android の `TextTranslator` 実装を返す（`device/translation.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn TextTranslator> {
    Arc::new(AndroidTextTranslator)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn TextTranslator> {
    Arc::new(fandhe_device::translation::UnsupportedTextTranslator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);

    fn is_internal(r: &Outcome, s: &str) -> bool {
        matches!(r, Err(DeviceError::Internal(x)) if *x == s)
    }

    #[test]
    fn decode_maps_codes() {
        assert_eq!(decode(RESULT_OK, Some("hi".into())).unwrap(), "hi");
        assert!(is_internal(&decode(RESULT_OK, None), EMPTY));
        assert!(matches!(
            decode(RESULT_UNSUPPORTED, None),
            Err(DeviceError::Unsupported("translation"))
        ));
        assert!(is_internal(
            &decode(RESULT_LANGUAGE_UNSUPPORTED, None),
            LANGUAGE_UNSUPPORTED
        ));
        assert!(is_internal(
            &decode(RESULT_NOT_INSTALLED, None),
            NOT_INSTALLED
        ));
        assert!(is_internal(
            &decode(RESULT_INTERRUPTED, None),
            "translation"
        ));
        assert!(is_internal(&decode(RESULT_FAILED, None), "translation"));
        assert!(is_internal(&decode(99, Some("x".into())), "translation"));
    }

    #[test]
    fn delivers_result() {
        let slot = Arc::new(RequestSlot::<Raw>::new());
        let s = slot.clone();
        let r = translate_with(
            &slot,
            T,
            move |id| {
                let s = s.clone();
                std::thread::spawn(move || {
                    s.deliver(id, (RESULT_OK, Some("hello".into())));
                });
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert_eq!(r.unwrap(), "hello");
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_cancels_once_and_is_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let cancelled = Cell::new(0);
        let r = translate_with(
            &slot,
            Duration::from_millis(150),
            |_| true,
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert!(is_internal(&r, TIMEOUT));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_is_not_overshot_by_poll_interval() {
        let slot = RequestSlot::<Raw>::new();
        let t0 = Instant::now();
        let _ = translate_with(&slot, Duration::from_millis(150), |_| true, |_| {});
        assert!(t0.elapsed() < Duration::from_millis(190));
    }

    #[test]
    fn result_delivered_after_deadline_is_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let cancelled = Cell::new(0);
        let r = translate_with(
            &slot,
            Duration::from_millis(10),
            |id| {
                std::thread::sleep(Duration::from_millis(20));
                assert!(slot.deliver(id, (RESULT_OK, Some("late".into()))));
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert!(is_internal(&r, TIMEOUT));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn start_failure_is_failed_and_clears() {
        let slot = RequestSlot::<Raw>::new();
        let r = translate_with(&slot, T, |_| false, |_| panic!("no cancel"));
        assert!(is_internal(&r, "translation"));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn stale_id_is_ignored_until_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let r = translate_with(
            &slot,
            Duration::from_millis(150),
            |id| {
                assert!(!slot.deliver(id + 100, (RESULT_OK, Some("x".into()))));
                true
            },
            |_| {},
        );
        assert!(is_internal(&r, TIMEOUT));
    }

    #[test]
    fn replaced_request_returns_immediately_and_keeps_new_slot() {
        let slot = RequestSlot::<Raw>::new();
        let new_id = Cell::new(0);
        let cancelled = Cell::new(0);
        let t0 = Instant::now();
        let r = translate_with(
            &slot,
            Duration::from_secs(5),
            |_| {
                new_id.set(slot.begin());
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(cancelled.get(), 1);
        assert!(is_internal(&r, "translation"));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(
            slot.snapshot().map(|(i, o)| (i, o.is_none())),
            Some((new_id.get(), true))
        );
    }
}
