//! Android 向けホーム画面ウィジェット実装（Issue #1067、親 #1064、macOS 実装 #1065 の
//! 横展開。機能 crate への移設は #1174）。`lib.rs` の Android 経路から [`AndroidHomeWidget`]
//! （[`crate::HomeWidget`] 実装）を使う。AndroidApp の取得と JavaVM への attach は
//! `fandhe-platform-android` の `with_activity` に任せ、本ファイルに `unsafe` は無い。
//!
//! # 保存主体
//!
//! 保存・読み出し・再描画要求はすべて Java 側が行う（`MainActivity.homeWidgetSetText` /
//! `homeWidgetCurrentText`、`HomeWidgetProvider`）。保存先は SharedPreferences
//! `fandhe_home_widget` の `text` キー。本モジュールは JNI 越しの橋渡しと、入力検証
//! （`validate_text`、JNI より前）・戻り値の写像だけを担う。
//!
//! # 再描画要求の扱い
//!
//! `Ok` は「`commit()` が成功した（保存できた）」ことだけを約束する。`AppWidgetManager`
//! への再描画要求はベストエフォートで、例外・ウィジェット 0 個の空振りでも Java 側は
//! `true` を返し `Ok` になる（macOS 実装 `macos.rs` と同じ。反映タイミングは
//! OS 側で非同期・保証しない）。
//!
//! # JNI 署名（Java 側と 2 か所で一致させる。native 宣言は無いので no-op スタブは不要）
//!
//! - `private boolean homeWidgetSetText(String)`
//! - `private String homeWidgetCurrentText()`（未保存は null ↔ `None`）
//!
//! # 守る範囲・守らない範囲
//!
//! 守る: 保存失敗・JNI 失敗を成功へ畳まない、未保存と空文字の区別、保存値をログ・エラー
//! 文言へ出さない。守らない: ウィジェット側の表示反映タイミング、複数プロセス間の順序
//! （書き込みは画面側が高々 1 件に絞る、`lib.rs::drive`）。

use crate::HomeWidgetError;

/// `set_text` の JNI 結果（`Some(commit の成否)`、JNI 失敗は `None`）を契約上の結果へ写す
/// （ホストの `cargo test` で固定する純関数）。`commit()` 失敗と JNI 失敗はどちらも
/// `Internal`（嘘の成功にしない）。再描画要求の失敗は Java 側で握るのでここへは届かない。
fn set_outcome(committed: Option<bool>) -> Result<(), HomeWidgetError> {
    match committed {
        Some(true) => Ok(()),
        Some(false) | None => Err(HomeWidgetError::Internal),
    }
}

/// Android 実ホーム画面ウィジェット実装。
#[cfg(target_os = "android")]
pub struct AndroidHomeWidget;

#[cfg(target_os = "android")]
impl AndroidHomeWidget {
    /// Activity インスタンスへ `f` を実行する共通処理。`android_app()` が `None` なら
    /// `Unsupported`、JNI 失敗は `Internal`（Java 例外は clear し文言は読まない）。
    fn with_activity<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &jni::objects::JObject,
        ) -> Result<T, fandhe_platform_android::JniCallFailed>,
    ) -> Result<T, HomeWidgetError> {
        let Some(app) = fandhe_platform_android::android_app() else {
            return Err(HomeWidgetError::Unsupported);
        };
        fandhe_platform_android::with_activity(&app, |env, activity| {
            let result = f(env, activity);
            if result.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            result
        })
        .map_err(|_| HomeWidgetError::Internal)
    }

    fn call_set(text: &str) -> Result<bool, HomeWidgetError> {
        Self::with_activity(|env, activity| {
            let j_text = env.new_string(text)?;
            let ret = env.call_method(
                activity,
                jni::jni_str!("homeWidgetSetText"),
                jni::jni_sig!((java.lang.String) -> boolean),
                &[jni::objects::JValue::Object(&j_text)],
            )?;
            Ok(ret.z()?)
        })
    }

    fn call_get() -> Result<Option<String>, HomeWidgetError> {
        Self::with_activity(|env, activity| {
            let ret = env.call_method(
                activity,
                jni::jni_str!("homeWidgetCurrentText"),
                jni::jni_sig!(() -> java.lang.String),
                &[],
            )?;
            let jstring = env.cast_local::<jni::objects::JString>(ret.l()?)?;
            if jstring.is_null() {
                Ok(None)
            } else {
                Ok(Some(jstring.try_to_string(env)?))
            }
        })
    }
}

#[cfg(target_os = "android")]
impl crate::HomeWidget for AndroidHomeWidget {
    fn set_text(&self, text: &str) -> Result<(), HomeWidgetError> {
        crate::validate_text(text)?;
        match Self::call_set(text) {
            Ok(committed) => set_outcome(Some(committed)),
            // `Unsupported`（Activity なし）は握り込まずそのまま返す。
            Err(HomeWidgetError::Internal) => set_outcome(None),
            Err(e) => Err(e),
        }
    }

    fn current_text(&self) -> Result<Option<String>, HomeWidgetError> {
        Self::call_get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_successful_commit_is_ok() {
        assert!(set_outcome(Some(true)).is_ok());
    }

    #[test]
    fn failed_commit_and_jni_failure_are_internal_errors() {
        assert!(matches!(
            set_outcome(Some(false)),
            Err(HomeWidgetError::Internal)
        ));
        assert!(matches!(set_outcome(None), Err(HomeWidgetError::Internal)));
    }
}
