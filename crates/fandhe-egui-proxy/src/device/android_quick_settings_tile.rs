//! Android 向けクイック設定タイル（`TileService`）の実装（親 #139、Issue #720）。
//! `device/quick_settings_tile.rs` の Android 経路（dispatch）から呼ばれ、effect ドライバの
//! ワーカースレッドで [`QuickSettingsTile::snapshot`] として回数を読む。
//!
//! 流れ: ユーザーがクイック設定の編集から「Fandhe カウント」を追加して押す → OS が
//! `QuickSettingsTileService.onClick` を呼ぶ → `QuickSettingsTileStore.increment` が
//! `synchronized` の内側で読み・+1・`commit()` する（SharedPreferences `fandhe_quick_settings_tile`
//! の `count`、ASCII 10 進数の String）。保存に失敗した押下は数えず、static の失敗件数を +1 する。
//! 本ファイルは JNI で `MainActivity.quickSettingsTileCountRaw()`（`()Ljava/lang/String;`、未保存は
//! null）と `MainActivity.quickSettingsTileFailedPresses()`（`()J`）を 1 回の attach の中で読み、
//! [`map_snapshot`] が契約の値へ写す。null は 0 回、壊れた内容・溢れ・負の失敗件数は `Internal`
//! （0 に戻さない）。
//!
//! JNI の署名は Java 側（`MainActivity`）と本ファイルの 2 か所で一致させる。no-op スタブは置かない
//! （Java に `native` 宣言は無く、呼び出しは Rust から Java への一方向だけ。`android_home_widget` と同じ）。
//!
//! `android_app()` が `None`（Activity が未確立・破棄直後）のときは `Unsupported` ではなく
//! `Internal` を返す。ドライバは `Unsupported` を受けると読み出しを恒久的にやめるため、一時的な
//! 不在で「未対応」に固まらないようにする。次の読み出しで回数表示へ戻る。
//!
//! Activity 再生成: 本機能は同期の pull 型で要求 id も保留中の要求も持たない
//! （`RequestSlot` / `RequestTracker` は使わない）。再生成中の読み出しが JNI 失敗しても `Internal`
//! になるだけで、次回の読み出しで新しい Activity 経由で回復する。古い完了が新しい表示を上書きしない
//! 世代管理は `Poller` / `finish_read` が担う。
//!
//! 守る範囲: 1 プロセス内で 1 押下 = +1、保存失敗を数えたことにしない、未押下（0）と読み出し失敗の
//! 区別、嘘の成功の禁止。守らない範囲: ポーリング間隔ぶんの表示の遅れ・プロセス終了で失敗件数が
//! 消えること・複数プロセス・タイル側への回数表示・アプリからのタイル追加（`requestAddTileService`）。
//! 回数と OS のエラー文はログに出さない。新しい権限・依存は足さない。

use std::sync::Arc;

use fandhe_device::quick_settings_tile::{parse_count, QuickSettingsTile, TileSnapshot};
use fandhe_device::DeviceError;

/// Android の `QuickSettingsTile` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn QuickSettingsTile> {
    Arc::new(AndroidQuickSettingsTile)
}

struct AndroidQuickSettingsTile;

impl QuickSettingsTile for AndroidQuickSettingsTile {
    fn snapshot(&self) -> Result<TileSnapshot, DeviceError> {
        let (raw, failed) = imp::read()?;
        map_snapshot(raw.as_deref(), failed)
    }
}

/// Java から読んだ保存内容と失敗件数を契約の値へ写す。未保存（`None`）は 0 回、壊れた内容と
/// 負の失敗件数は `Internal`（成功へ倒さない）。
fn map_snapshot(raw: Option<&str>, failed: i64) -> Result<TileSnapshot, DeviceError> {
    let failed = u64::try_from(failed).map_err(|_| DeviceError::Internal("quick_settings_tile"))?;
    let count = match raw {
        None => 0,
        Some(s) => parse_count(s.as_bytes())?,
    };
    Ok(TileSnapshot {
        count,
        failed_presses: Some(failed),
    })
}

#[cfg(target_os = "android")]
mod imp {
    use fandhe_device::DeviceError;

    const INTERNAL: DeviceError = DeviceError::Internal("quick_settings_tile");

    /// 保存内容（未保存は `None`）と失敗件数を 1 回の attach で読む。JNI 失敗・Java 例外は
    /// 例外を clear して `Internal`（文言は読まない）。
    #[allow(unsafe_code)]
    pub(super) fn read() -> Result<(Option<String>, i64), DeviceError> {
        let Some(app) = crate::android::android_app() else {
            return Err(INTERNAL);
        };
        // SAFETY: `android_home_widget::with_activity` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` で、`app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(
            |env: &mut jni::Env| -> Result<(Option<String>, i64), super::super::android::JniCallFailed> {
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない
                // （`app` の生存中有効）。
                let activity =
                    unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                let result = (|| {
                    let ret = env.call_method(
                        &activity,
                        jni::jni_str!("quickSettingsTileCountRaw"),
                        jni::jni_sig!(() -> java.lang.String),
                        &[],
                    )?;
                    let jstring = env.cast_local::<jni::objects::JString>(ret.l()?)?;
                    let raw = if jstring.is_null() {
                        None
                    } else {
                        Some(jstring.try_to_string(env)?)
                    };
                    let failed = env
                        .call_method(
                            &activity,
                            jni::jni_str!("quickSettingsTileFailedPresses"),
                            jni::jni_sig!(() -> long),
                            &[],
                        )?
                        .j()?;
                    Ok((raw, failed))
                })();
                if result.is_err() && env.exception_check() {
                    // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                    env.exception_clear();
                }
                result
            },
        )
        .map_err(|_| INTERNAL)
    }
}

/// `device.rs` が Android 限定（とテスト）で載せるため、ホストの実行では使われない。
/// 呼ばれても成功へ倒さない。
#[cfg(not(target_os = "android"))]
mod imp {
    use fandhe_device::DeviceError;

    pub(super) fn read() -> Result<(Option<String>, i64), DeviceError> {
        Err(DeviceError::Unsupported("quick_settings_tile"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal<T: std::fmt::Debug>(r: Result<T, DeviceError>) {
        assert!(
            matches!(r, Err(DeviceError::Internal("quick_settings_tile"))),
            "{r:?}"
        );
    }

    #[test]
    fn unsaved_is_zero_with_no_failures() {
        let s = map_snapshot(None, 0).unwrap();
        assert_eq!(s.count, 0);
        assert_eq!(s.failed_presses, Some(0));
    }

    #[test]
    fn saved_count_is_parsed() {
        assert_eq!(map_snapshot(Some("5"), 0).unwrap().count, 5);
        assert_eq!(
            map_snapshot(Some("18446744073709551615"), 0).unwrap().count,
            u64::MAX
        );
    }

    #[test]
    fn broken_content_is_internal_not_zero() {
        for bad in ["", "+1", "-1", " 1", "1\n", "18446744073709551616", "abc"] {
            internal(map_snapshot(Some(bad), 0));
        }
    }

    #[test]
    fn failed_presses_are_reported_and_negative_is_internal() {
        assert_eq!(map_snapshot(None, 3).unwrap().failed_presses, Some(3));
        internal(map_snapshot(None, -1));
    }

    #[test]
    fn host_implementation_never_succeeds() {
        assert!(matches!(
            implementation().snapshot(),
            Err(DeviceError::Unsupported("quick_settings_tile"))
        ));
    }
}
