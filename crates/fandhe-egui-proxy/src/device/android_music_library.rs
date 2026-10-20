//! Android 向け音楽ライブラリ実装（Issue #485、親 #108）。`device/music_library.rs::implementation`
//! の Android 経路から呼ばれ、`MediaStore.Audio.Media` の曲のタイトルとアーティストを
//! 期限付きで 1 回読む。Java 側は `MusicLibraryHelper`（許可確認・クエリ・取り消し）。
//!
//! 流れ: `MusicLibrary::list_tracks`（ワーカースレッド）→ [`RequestSlot::begin`] で id 採番 →
//! JNI `MainActivity.requestMusicLibraryList(id)` → Java が結果コードと行を
//! `nativeOnMusicLibraryResult` で返す → [`decode`] で契約の失敗種別へ写像し、整形は
//! `tracks_from` に一本化 → ワーカーが slot を見て受け取る。期限は契約の `LIST_TIMEOUT`
//! 1 つだけで、切れたら `cancelMusicLibraryList(id)` でクエリを取り消し `Timeout`
//! （0 曲にしない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・0 曲と失敗の区別・拒否/破棄/壊れた入力を成功に
//! 畳まないこと。守らない範囲: 並び順、読み取り後の変化、期限後に OS 側の処理が走り切ること。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestMusicLibraryList` を経た後でしか
//! 呼ばれない（歩数計と同じ）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use fandhe_device::music_library::{
    decode_code, tracks_from, MusicLibrary, MusicLibraryFailure, TrackList, MAX_TRACKS,
};

use super::request_slot::{RequestSlot, POLL_INTERVAL};

type Outcome = Result<TrackList, MusicLibraryFailure>;

/// Java が送る最大行数。`MAX_TRACKS` の次の 1 行で `truncated` を判定させる。
const MAX_ROWS: usize = MAX_TRACKS + 1;

/// Java の結果コードと行（`[title0, artist0, title1, ...]`）を契約の結果へ写像する。
/// 成功時に行が無い・奇数個・上限超過のときは、組がずれるため捨てずに `Failed`。
fn decode(code: i32, rows: Option<Vec<Option<String>>>) -> Outcome {
    if let Some(f) = decode_code(code) {
        return Err(f);
    }
    let rows = rows.ok_or(MusicLibraryFailure::Failed)?;
    if rows.len() % 2 != 0 || rows.len() > 2 * MAX_ROWS {
        return Err(MusicLibraryFailure::Failed);
    }
    let mut it = rows.into_iter();
    let mut pairs = Vec::new();
    while let (Some(t), Some(a)) = (it.next(), it.next()) {
        pairs.push((t, a));
    }
    Ok(tracks_from(pairs))
}

/// 要求 1 件を出して期限まで結果を待つ。`start` は OS へ要求を渡す（false は呼び出し失敗）、
/// `cancel` は期限切れ時に OS 側のクエリを取り消させる。
fn read_with(
    slot: &RequestSlot<Outcome>,
    timeout: Duration,
    start: impl FnOnce(u64) -> bool,
    cancel: impl FnOnce(u64),
) -> Outcome {
    // 期限は呼び出し開始時点から数える（JNI 呼び出しの所要時間も含める）。
    let deadline = Instant::now() + timeout;
    let id = slot.begin();
    if !start(id) {
        slot.clear(id);
        return Err(MusicLibraryFailure::Failed);
    }
    loop {
        // 結果の受理より先に期限を確認する（期限後に届いた値は捨てる）。
        let now = Instant::now();
        if now >= deadline {
            slot.clear(id);
            cancel(id);
            return Err(MusicLibraryFailure::Timeout);
        }
        match slot.snapshot() {
            Some((cur, Some(outcome))) if cur == id => {
                slot.clear(id);
                return outcome;
            }
            Some((cur, None)) if cur == id => {}
            // 差し替え済み（他人の slot は消さない）。Java 側に残るクエリを取り消させる。
            _ => {
                cancel(id);
                return Err(MusicLibraryFailure::Timeout);
            }
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(target_os = "android")]
static SLOT: RequestSlot<Outcome> = RequestSlot::new();

#[cfg(target_os = "android")]
struct AndroidMusicLibrary;

#[cfg(target_os = "android")]
impl MusicLibrary for AndroidMusicLibrary {
    fn list_tracks(&self, timeout: Duration) -> Outcome {
        read_with(
            &SLOT,
            timeout,
            |id| call_activity(false, id),
            |id| {
                let _ = call_activity(true, id);
            },
        )
    }
}

/// `MainActivity.requestMusicLibraryList(long)`（`cancel=false`）または
/// `cancelMusicLibraryList(long)`（`cancel=true`）を JNI で呼ぶ。`false` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_pedometer.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(cancel: bool, id: u64) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        let args = [jni::objects::JValue::Long(id as jni::sys::jlong)];
        if cancel {
            env.call_method(
                &activity,
                jni::jni_str!("cancelMusicLibraryList"),
                jni::jni_sig!((long) -> ()),
                &args,
            )?;
        } else {
            env.call_method(
                &activity,
                jni::jni_str!("requestMusicLibraryList"),
                jni::jni_sig!((long) -> ()),
                &args,
            )?;
        }
        Ok(())
    })
    .is_ok()
}

/// `MusicLibraryHelper.nativeOnMusicLibraryResult`（Java 側）から呼ばれる JNI エクスポート。
/// 曲名・アーティストは一切ログへ出さない。`rows` が読めなければ全体を `Failed` にする。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` による panic の遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MusicLibraryHelper_nativeOnMusicLibraryResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    rows: jni::objects::JObjectArray<'caller>,
) {
    use jni::objects::JString;
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let parsed: Option<Vec<Option<String>>> = if rows.is_null() {
            None
        } else {
            let len = rows.len(env)?;
            // 上限超過は読まずに壊れた入力として扱う（`decode` が Failed にする）。
            if len > 2 * MAX_ROWS {
                Some(vec![None; 2 * MAX_ROWS + 1])
            } else {
                let mut v = Vec::with_capacity(len);
                for i in 0..len {
                    let obj = rows.get_element(env, i)?;
                    if obj.is_null() {
                        v.push(None);
                        continue;
                    }
                    let s = env.cast_local::<JString>(obj)?;
                    v.push(Some(s.try_to_string(env)?));
                }
                Some(v)
            }
        };
        SLOT.deliver(request_id as u64, decode(code, parsed));
        Ok(())
    });
    if !matches!(outcome.into_outcome(), jni::Outcome::Ok(_)) {
        // JNI 例外などで行を読めなかった。要求を放置せず Failed で確定する。
        SLOT.deliver(request_id as u64, Err(MusicLibraryFailure::Failed));
    }
}

/// Android の `MusicLibrary` 実装を返す（`device/music_library.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn MusicLibrary> {
    Arc::new(AndroidMusicLibrary)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn MusicLibrary> {
    Arc::new(fandhe_device::music_library::UnsupportedMusicLibrary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const T: Duration = Duration::from_millis(500);

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn decode_maps_codes() {
        assert_eq!(decode(1, None), Err(MusicLibraryFailure::Unsupported));
        assert_eq!(decode(2, None), Err(MusicLibraryFailure::PermissionDenied));
        assert_eq!(decode(4, None), Err(MusicLibraryFailure::Failed));
        assert_eq!(decode(99, None), Err(MusicLibraryFailure::Failed));
        assert_eq!(decode(0, None), Err(MusicLibraryFailure::Failed));
    }

    #[test]
    fn decode_pairs_rows() {
        let l = decode(0, Some(vec![s("a"), None, None, s("b")])).unwrap();
        assert_eq!(l.tracks.len(), 2);
        assert_eq!(l.tracks[0].title.as_deref(), Some("a"));
        assert_eq!(l.tracks[0].artist, None);
        assert_eq!(l.tracks[1].artist.as_deref(), Some("b"));
        assert!(!l.truncated);
    }

    #[test]
    fn decode_zero_tracks_is_success() {
        let l = decode(0, Some(vec![])).unwrap();
        assert!(l.tracks.is_empty() && !l.truncated);
    }

    #[test]
    fn decode_rejects_odd_and_oversize() {
        assert_eq!(
            decode(0, Some(vec![s("a")])),
            Err(MusicLibraryFailure::Failed)
        );
        assert_eq!(
            decode(0, Some(vec![None; 2 * MAX_ROWS + 2])),
            Err(MusicLibraryFailure::Failed)
        );
    }

    #[test]
    fn decode_truncates_at_21_rows() {
        let l = decode(0, Some(vec![s("x"); 2 * MAX_ROWS])).unwrap();
        assert_eq!(l.tracks.len(), MAX_TRACKS);
        assert!(l.truncated);
        let l = decode(0, Some(vec![s("x"); 2 * MAX_TRACKS])).unwrap();
        assert!(!l.truncated);
    }

    fn empty() -> TrackList {
        tracks_from(Vec::new())
    }

    #[test]
    fn delivers_result() {
        let slot = Arc::new(RequestSlot::<Outcome>::new());
        let sl = slot.clone();
        let r = read_with(
            &slot,
            T,
            move |id| {
                let sl = sl.clone();
                std::thread::spawn(move || {
                    sl.deliver(id, Ok(empty()));
                });
                true
            },
            |_| panic!("cancel must not run"),
        );
        assert!(r.is_ok());
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn timeout_cancels_once_and_is_timeout() {
        let slot = RequestSlot::<Outcome>::new();
        let cancelled = Cell::new(0);
        let t0 = Instant::now();
        let r = read_with(
            &slot,
            Duration::from_millis(150),
            |_| true,
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(r, Err(MusicLibraryFailure::Timeout));
        assert_eq!(cancelled.get(), 1);
        assert!(slot.snapshot().is_none());
        assert!(t0.elapsed() < Duration::from_millis(190));
    }

    #[test]
    fn result_after_deadline_is_timeout() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(
            &slot,
            Duration::from_millis(10),
            |id| {
                std::thread::sleep(Duration::from_millis(20));
                assert!(slot.deliver(id, Ok(empty())));
                true
            },
            |_| {},
        );
        assert_eq!(r, Err(MusicLibraryFailure::Timeout));
    }

    #[test]
    fn start_failure_is_failed() {
        let slot = RequestSlot::<Outcome>::new();
        let r = read_with(&slot, T, |_| false, |_| panic!("no cancel"));
        assert_eq!(r, Err(MusicLibraryFailure::Failed));
        assert!(slot.snapshot().is_none());
    }

    #[test]
    fn replaced_request_returns_immediately_and_keeps_new_slot() {
        let slot = RequestSlot::<Outcome>::new();
        let new_id = Cell::new(0);
        let cancelled = Cell::new(0);
        let t0 = Instant::now();
        let r = read_with(
            &slot,
            Duration::from_secs(5),
            |_| {
                new_id.set(slot.begin());
                true
            },
            |_| cancelled.set(cancelled.get() + 1),
        );
        assert_eq!(cancelled.get(), 1);
        assert_eq!(r, Err(MusicLibraryFailure::Timeout));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(slot.snapshot(), Some((new_id.get(), None)));
    }
}
