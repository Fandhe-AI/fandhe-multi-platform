//! Android 向けクイック設定タイル（`TileService`）の実装（親 #139、Issue #720）。
//! 本 crate の `driver` の Android 経路（dispatch）から呼ばれ、effect ドライバの
//! ワーカースレッドで [`QuickSettingsTile::snapshot`] として回数を読む。
//!
//! 流れ: ユーザーがクイック設定の編集から「Fandhe カウント」を追加して押す → OS が
//! `QuickSettingsTileService.onClick` を呼ぶ → `QuickSettingsTileStore.increment` が
//! `synchronized` の内側で読み・+1・`commit()` する（SharedPreferences `fandhe_quick_settings_tile`
//! の `count`、ASCII 10 進数の String）。保存に失敗した押下は数えず、static の失敗件数を +1 する。
//! 本ファイルは JNI で `QuickSettingsTileStore.readRaw(Context)`（`(Landroid/content/Context;)Ljava/lang/String;`、
//! 未保存は null）と `QuickSettingsTileStore.failedPresses()`（`()J`）を static 呼び出しで 1 回の attach の中で読み、
//! [`map_snapshot`] が契約の値へ写す。null は 0 回、壊れた内容・溢れ・負の失敗件数は `Internal`
//! （0 に戻さない）。
//!
//! JNI の署名は Java 側（`QuickSettingsTileStore`）と本ファイルの 2 か所で一致させる。no-op スタブは置かない
//! （Java に `native` 宣言は無く、呼び出しは Rust から Java への一方向だけ。`fandhe_platform_android::with_activity` 経由の機能 crate（home-widget 等）と同じ）。
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

use crate::contract::{QuickSettingsTile, TileSnapshot, parse_count};
use crate::error::QuickSettingsTileError;

/// Android の `QuickSettingsTile` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn QuickSettingsTile> {
    Arc::new(AndroidQuickSettingsTile)
}

struct AndroidQuickSettingsTile;

impl QuickSettingsTile for AndroidQuickSettingsTile {
    fn snapshot(&self) -> Result<TileSnapshot, QuickSettingsTileError> {
        let (raw, failed) = imp::read()?;
        map_snapshot(raw.as_deref(), failed)
    }
}

/// Java から読んだ保存内容と失敗件数を契約の値へ写す。未保存（`None`）は 0 回、壊れた内容と
/// 負の失敗件数は `Internal`（成功へ倒さない）。
fn map_snapshot(raw: Option<&str>, failed: i64) -> Result<TileSnapshot, QuickSettingsTileError> {
    let failed = u64::try_from(failed).map_err(|_| QuickSettingsTileError::Internal)?;
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
    use crate::error::QuickSettingsTileError;
    use fandhe_platform_android::JniCallFailed;

    const INTERNAL: QuickSettingsTileError = QuickSettingsTileError::Internal;

    /// 保存内容（未保存は `None`）と失敗件数を 1 回の attach で読む。JNI 失敗・Java 例外は
    /// 例外を clear して `Internal`（文言は読まない）。`unsafe` は共通層
    /// `fandhe_platform_android::with_activity_class` に閉じている。
    pub(super) fn read() -> Result<(Option<String>, i64), QuickSettingsTileError> {
        let Some(app) = fandhe_platform_android::android_app() else {
            return Err(INTERNAL);
        };
        fandhe_platform_android::with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.QuickSettingsTileStore"),
            |env, activity, class| -> Result<(Option<String>, i64), JniCallFailed> {
                let result = (|| {
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("readRaw"),
                        jni::jni_sig!((android.content.Context) -> java.lang.String),
                        &[jni::objects::JValue::Object(activity)],
                    )?;
                    let jstring = env.cast_local::<jni::objects::JString>(ret.l()?)?;
                    let raw = if jstring.is_null() {
                        None
                    } else {
                        Some(jstring.try_to_string(env)?)
                    };
                    let failed = env
                        .call_static_method(
                            class,
                            jni::jni_str!("failedPresses"),
                            jni::jni_sig!(() -> long),
                            &[],
                        )?
                        .j()?;
                    Ok::<_, jni::errors::Error>((raw, failed))
                })();
                if result.is_err() && env.exception_check() {
                    // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                    env.exception_clear();
                }
                Ok(result?)
            },
        )
        .map_err(|_| INTERNAL)
    }
}

/// `lib.rs` が Android 限定（とテスト）で載せるため、ホストの実行では使われない。
/// 呼ばれても成功へ倒さない。
#[cfg(not(target_os = "android"))]
mod imp {
    use crate::error::QuickSettingsTileError;

    pub(super) fn read() -> Result<(Option<String>, i64), QuickSettingsTileError> {
        Err(QuickSettingsTileError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal<T: std::fmt::Debug>(r: Result<T, QuickSettingsTileError>) {
        assert!(matches!(r, Err(QuickSettingsTileError::Internal)), "{r:?}");
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
            Err(QuickSettingsTileError::Unsupported)
        ));
    }
}
