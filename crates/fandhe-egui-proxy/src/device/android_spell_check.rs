//! Android 向けスペルチェック（Issue #580、親 #119）。`device/spell_check.rs::platform` の Android 経路から
//! 呼ばれ、`TextServicesManager.newSpellCheckerSession` + `SpellCheckerSession.getSentenceSuggestions`
//! （権限・`<queries>` 不要）で短い文を 1 回検査する。Java 側は `SpellCheckHelper`（専用 HandlerThread・
//! session の生成と解放・取り消し）。
//!
//! 流れ: `SpellChecker::check`（ドライバのワーカースレッド）→ [`RequestSlot::begin`] で id 採番 →
//! JNI `MainActivity.requestSpellCheck(id, text, language, limit)` → Java が結果コード・誤りの位置・候補を
//! `nativeOnSpellCheckResult` で返す（実際に検査へ使う言語を含む） → [`decode`] で契約の結果へ写像 → ワーカーが周期的に slot を見て
//! 受け取る。期限は引数 `remaining`（ドライバが 10 秒を渡す）だけで、切れたら `cancelSpellCheck(id)` で
//! session を閉じて `Internal(TIMEOUT)`。
//!
//! 写像（SSOT は `fandhe_device::spell_check`）: API 30 以下で session が null・API 31 以上で
//! スペルチェッカー無効は `Unsupported`（API 30 以下は「無効」と「辞書なし」を区別できない）、API 31 以上で
//! 有効なのに session が null は `LANGUAGE_UNAVAILABLE`、中断・OS の失敗・配列の不整合は
//! `Internal("spell_check")`、範囲の異常は `build_report` が `Internal` にする。
//!
//! ライフサイクル: Activity 再生成（`onDestroy`）で保留中の要求を Java が中断扱いで確定し、session を
//! 閉じる。`SLOT` の id は単調増加で、新しい `SpellCheckHelper` は未追跡から始まるため旧結果が新しい
//! 要求を消費しない。Rust 側の保留追跡は `spell_check::reset_pending_tracking`（`android.rs` から呼び済み）。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・session の解放・入力と誤り語のログ非出力。
//! 守らない範囲: 期限後も OS 側の検査が走り切ること、返す言語は要求した言語であり OS が選んだ subtype では
//! ないこと、スペルチェッカーの実装（OEM・キーボードアプリ）による結果の違い。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestSpellCheck` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（translation と同じ）。
//!
//! JNI 署名（Java・Rust で一致させる）: `requestSpellCheck(JLjava/lang/String;Ljava/lang/String;I)V`、
//! `cancelSpellCheck(J)V`、`native nativeOnSpellCheckResult(long, int, int[], String[], String)`。

use std::sync::Arc;
use std::time::{Duration, Instant};

use fandhe_device::spell_check::{
    build_report, RawIssue, SpellChecker, SpellReport, LANGUAGE_UNAVAILABLE, TIMEOUT,
};
use fandhe_device::DeviceError;

use super::request_slot::{RequestSlot, POLL_INTERVAL};

/// 結果コード（SSOT は Java 側 `SpellCheckHelper.RESULT_*`）。
const RESULT_OK: i32 = 0;
const RESULT_UNSUPPORTED: i32 = 1;
const RESULT_LANGUAGE_UNAVAILABLE: i32 = 2;
/// Activity 破棄による中断。Rust 側では未知のコードと同じ失敗へ写すため、値の記録とテスト用。
#[allow(dead_code)]
const RESULT_INTERRUPTED: i32 = 3;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const RESULT_FAILED: i32 = 4;

type Outcome = Result<SpellReport, DeviceError>;
/// slot に置く生の結果（結果コード, ranges, 候補, 実際に使った言語）。ranges は誤り 1 件につき
/// `utf16_start, utf16_len, 候補数` の 3 値を平らに並べたもの、候補は全件を平らに並べたもの。
/// `DeviceError` は `Clone` でないため、写像は受け取る側で行う。
type Raw = (i32, Vec<i32>, Vec<String>, Option<String>);

/// ranges と候補の平らな並びを [`RawIssue`] へ戻す。長さが 3 の倍数でない・負の値・候補数の合計が
/// 候補の総数と一致しない場合は `None`（嘘の成功にしない）。
fn decode_issues(ranges: &[i32], suggestions: Vec<String>) -> Option<Vec<RawIssue>> {
    if ranges.len() % 3 != 0 || ranges.iter().any(|v| *v < 0) {
        return None;
    }
    let mut total = 0usize;
    for chunk in ranges.chunks_exact(3) {
        total = total.checked_add(usize::try_from(chunk[2]).ok()?)?;
    }
    if total != suggestions.len() {
        return None;
    }
    let mut it = suggestions.into_iter();
    let mut out = Vec::with_capacity(ranges.len() / 3);
    for chunk in ranges.chunks_exact(3) {
        let n = usize::try_from(chunk[2]).ok()?;
        out.push(RawIssue {
            utf16_start: usize::try_from(chunk[0]).ok()?,
            utf16_len: usize::try_from(chunk[1]).ok()?,
            suggestions: it.by_ref().take(n).collect(),
        });
    }
    Some(out)
}

/// Java の結果を契約の結果へ写像する。`requested` は Java が実使用言語を返さなかった場合だけ使う。
/// 報告する言語は同じ主言語の別 subtype が選ばれた場合に備え、Java が返した実使用言語を優先する。
fn decode(text: &str, requested: &str, raw: Raw) -> Outcome {
    let (code, ranges, suggestions, actual) = raw;
    let language = actual
        .as_deref()
        .filter(|l| !l.is_empty() && !l.contains('\0'))
        .unwrap_or(requested);
    match code {
        RESULT_OK => {
            let issues =
                decode_issues(&ranges, suggestions).ok_or(DeviceError::Internal("spell_check"))?;
            build_report(text, language, issues, false)
        }
        RESULT_UNSUPPORTED => Err(DeviceError::Unsupported("spell_check")),
        RESULT_LANGUAGE_UNAVAILABLE => Err(DeviceError::Internal(LANGUAGE_UNAVAILABLE)),
        // 中断（RESULT_INTERRUPTED）・OS の失敗・未知のコード。
        _ => Err(DeviceError::Internal("spell_check")),
    }
}

/// 要求 1 件を出して期限まで結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）、
/// `cancel` は期限切れ・差し替え時に Java 側の session を閉じさせる。
fn check_with(
    slot: &RequestSlot<Raw>,
    remaining: Duration,
    start: impl FnOnce(u64) -> bool,
    cancel: impl FnOnce(u64),
) -> Result<Raw, DeviceError> {
    // 期限は呼び出し開始時点から数える（JNI 呼び出しの所要時間も含める）。
    let deadline = Instant::now() + remaining;
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(DeviceError::Internal("spell_check"));
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
            Some((cur, Some(raw))) if cur == id => {
                slot.clear(id);
                return Ok(raw);
            }
            Some((cur, None)) if cur == id => {}
            // 差し替え済み（他人の slot は消さない）。Java 側に残る session は閉じさせる。
            _ => {
                cancel(id);
                return Err(DeviceError::Internal("spell_check"));
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Raw> = RequestSlot::new();

#[cfg(target_os = "android")]
struct AndroidSpellChecker;

#[cfg(target_os = "android")]
impl SpellChecker for AndroidSpellChecker {
    fn check(&self, text: &str, language: &str, remaining: Duration) -> Outcome {
        let raw = check_with(
            &SLOT,
            remaining,
            |id| call_request(id, text, language),
            call_cancel,
        )?;
        decode(text, language, raw)
    }
}

/// `MainActivity.requestSpellCheck(long, String, String, int)` を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_translation.rs::call_request` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_request(id: u64, text: &str, language: &str) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_text = env.new_string(text)?;
        let j_language = env.new_string(language)?;
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestSpellCheck"),
            jni::jni_sig!((long, java.lang.String, java.lang.String, int) -> ()),
            &[
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Object(&j_text),
                jni::objects::JValue::Object(&j_language),
                jni::objects::JValue::Int(
                    fandhe_device::spell_check::MAX_SUGGESTIONS as jni::sys::jint,
                ),
            ],
        )?;
        Ok(())
    })
    .is_ok()
}

/// `MainActivity.cancelSpellCheck(long)` を JNI で呼ぶ。失敗しても期限切れの確定は変わらない。
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
            jni::jni_str!("cancelSpellCheck"),
            jni::jni_sig!((long) -> ()),
            &[jni::objects::JValue::Long(id as jni::sys::jlong)],
        )?;
        Ok(())
    });
}

/// `SpellCheckHelper.nativeOnSpellCheckResult`（Java 側）から呼ばれる JNI エクスポート。
/// 入力文・誤り語・候補は一切ログへ出さない。null の配列は空として扱う。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_SpellCheckHelper_nativeOnSpellCheckResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    ranges: jni::objects::JIntArray<'caller>,
    suggestions: jni::objects::JObjectArray<'caller>,
    language: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let mut flat = Vec::new();
        if !ranges.is_null() {
            let n = ranges.len(env)?;
            flat = vec![0i32; n];
            ranges.get_region(env, 0, &mut flat)?;
        }
        let mut words = Vec::new();
        if !suggestions.is_null() {
            let n = suggestions.len(env)?;
            for i in 0..n {
                // 要素ごとに local 参照を解放し、件数が多くても上限に当たらないようにする。
                let one = env.with_local_frame(4, |env| -> Result<String, jni::errors::Error> {
                    let obj = suggestions.get_element(env, i)?;
                    let s = env.cast_local::<jni::objects::JString>(obj)?;
                    s.try_to_string(env)
                })?;
                words.push(one);
            }
        }
        let actual = if language.is_null() {
            None
        } else {
            Some(language.try_to_string(env)?)
        };
        SLOT.deliver(request_id as u64, (code, flat, words, actual));
        Ok(())
    });
    if !matches!(outcome.into_outcome(), jni::Outcome::Ok(_)) {
        // JNI 例外などで結果を読めなかった。要求を放置せず失敗で確定する。
        SLOT.deliver(
            request_id as u64,
            (RESULT_FAILED, Vec::new(), Vec::new(), None),
        );
    }
}

/// Android の `SpellChecker` 実装を返す（`device/spell_check.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn SpellChecker> {
    Arc::new(AndroidSpellChecker)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn SpellChecker> {
    Arc::new(fandhe_device::spell_check::UnsupportedSpellChecker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);

    fn is_internal<R>(r: &Result<R, DeviceError>, s: &str) -> bool {
        matches!(r, Err(DeviceError::Internal(x)) if *x == s)
    }

    fn ok_raw(ranges: Vec<i32>, sugg: &[&str]) -> Raw {
        (
            RESULT_OK,
            ranges,
            sugg.iter().map(|s| s.to_string()).collect(),
            None,
        )
    }

    #[test]
    fn decode_maps_codes() {
        let empty = |c| (c, vec![], vec![], None);
        assert!(matches!(
            decode("a", "en-US", empty(RESULT_UNSUPPORTED)),
            Err(DeviceError::Unsupported("spell_check"))
        ));
        assert!(is_internal(
            &decode("a", "en-US", empty(RESULT_LANGUAGE_UNAVAILABLE)),
            LANGUAGE_UNAVAILABLE
        ));
        assert!(is_internal(
            &decode("a", "en-US", empty(RESULT_INTERRUPTED)),
            "spell_check"
        ));
        assert!(is_internal(
            &decode("a", "en-US", empty(RESULT_FAILED)),
            "spell_check"
        ));
        assert!(is_internal(&decode("a", "en-US", empty(99)), "spell_check"));
    }

    #[test]
    fn decode_ok_builds_report() {
        // 「😀 helo wrld」: 絵文字は UTF-16 で 2 単位。helo = 3..7、wrld = 8..12。
        let text = "😀 helo wrld";
        let raw = ok_raw(vec![3, 4, 2, 8, 4, 1], &["hello", "help", "world"]);
        let r = decode(text, "en-US", raw).unwrap();
        assert_eq!(r.language, "en-US");
        assert_eq!(r.issues.len(), 2);
        assert_eq!((r.issues[0].start, r.issues[0].len), (2, 4));
        assert_eq!(r.issues[0].word, "helo");
        assert_eq!(r.issues[0].suggestions, vec!["hello", "help"]);
        assert_eq!(r.issues[1].word, "wrld");
        assert_eq!(r.issues[1].suggestions, vec!["world"]);
        // 誤り 0 件は成功。
        let r = decode("hello", "en-US", ok_raw(vec![], &[])).unwrap();
        assert!(r.issues.is_empty());
    }

    #[test]
    fn decode_uses_actual_language_for_same_primary_fallback() {
        // en-GB を要求したが端末に en-US の subtype しかない場合、実使用言語を報告する。
        let mut raw = ok_raw(vec![], &[]);
        raw.3 = Some("en-US".to_string());
        assert_eq!(decode("hello", "en-GB", raw).unwrap().language, "en-US");
        // 実使用言語が無い・空なら要求した言語。
        let mut raw = ok_raw(vec![], &[]);
        raw.3 = Some(String::new());
        assert_eq!(decode("hello", "en-GB", raw).unwrap().language, "en-GB");
        assert_eq!(
            decode("hello", "en-GB", ok_raw(vec![], &[]))
                .unwrap()
                .language,
            "en-GB"
        );
    }

    #[test]
    fn decode_issues_rejects_malformed() {
        let s = |n: usize| vec!["x".to_string(); n];
        assert!(decode_issues(&[0, 1], s(0)).is_none());
        assert!(decode_issues(&[-1, 1, 0], s(0)).is_none());
        assert!(decode_issues(&[0, -1, 0], s(0)).is_none());
        assert!(decode_issues(&[0, 1, -1], s(0)).is_none());
        assert!(decode_issues(&[0, 1, 2], s(1)).is_none());
        assert!(decode_issues(&[0, 1, 1], s(2)).is_none());
        assert_eq!(decode_issues(&[0, 1, 1], s(1)).unwrap().len(), 1);
        assert!(is_internal(
            &decode("ab", "en-US", ok_raw(vec![0, 1, 2], &["x"])),
            "spell_check"
        ));
    }

    #[test]
    fn decode_ok_with_bad_range_is_internal() {
        assert!(decode("hello", "en-US", ok_raw(vec![0, 0, 0], &[])).is_err());
        assert!(decode("hello", "en-US", ok_raw(vec![3, 9, 0], &[])).is_err());
        assert!(decode("hello", "en-US", ok_raw(vec![0, 3, 0, 2, 2, 0], &[])).is_err());
    }

    #[test]
    fn delivers_result() {
        let slot = Arc::new(RequestSlot::<Raw>::new());
        let s = slot.clone();
        let r = check_with(
            &slot,
            T,
            move |id| {
                let s = s.clone();
                std::thread::spawn(move || {
                    s.deliver(id, (RESULT_OK, vec![], vec![], None));
                });
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert_eq!(r.unwrap().0, RESULT_OK);
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_cancels_once_and_is_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let cancelled = Cell::new(0);
        let r = check_with(
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
        let _ = check_with(&slot, Duration::from_millis(150), |_| true, |_| {});
        assert!(t0.elapsed() < Duration::from_millis(190));
    }

    #[test]
    fn result_delivered_after_deadline_is_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let cancelled = Cell::new(0);
        let r = check_with(
            &slot,
            Duration::from_millis(10),
            |id| {
                std::thread::sleep(Duration::from_millis(20));
                assert!(slot.deliver(id, (RESULT_OK, vec![], vec![], None)));
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
        let r = check_with(&slot, T, |_| false, |_| panic!("no cancel"));
        assert!(is_internal(&r, "spell_check"));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn stale_id_is_ignored_until_timeout() {
        let slot = RequestSlot::<Raw>::new();
        let r = check_with(
            &slot,
            Duration::from_millis(150),
            |id| {
                assert!(!slot.deliver(id + 100, (RESULT_OK, vec![], vec![], None)));
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
        let r = check_with(
            &slot,
            Duration::from_secs(5),
            |_| {
                new_id.set(slot.begin());
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(cancelled.get(), 1);
        assert!(is_internal(&r, "spell_check"));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(
            slot.snapshot().map(|(i, o)| (i, o.is_none())),
            Some((new_id.get(), true))
        );
    }
}
