//! Android 向けホーム画面ウィジェット実装（Issue #1067、親 #1064、macOS 実装 #1065 の
//! 横展開）。`device/home_widget.rs` の Android 経路から [`AndroidHomeWidget`]
//! （`fandhe_device::home_widget::HomeWidget` 実装）を使う。
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
//! `true` を返し `Ok` になる（macOS 実装 `apple_home_widget.rs` と同じ。反映タイミングは
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
//! （書き込みは画面側が高々 1 件に絞る、`device/home_widget.rs::drive`）。

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use fandhe_device::DeviceError;

/// `set_text` の JNI 結果（`Some(commit の成否)`、JNI 失敗は `None`）を契約上の結果へ写す
/// （ホストの `cargo test` で固定する純関数）。`commit()` 失敗と JNI 失敗はどちらも
/// `Internal`（嘘の成功にしない）。再描画要求の失敗は Java 側で握るのでここへは届かない。
fn set_outcome(committed: Option<bool>) -> Result<(), DeviceError> {
    match committed {
        Some(true) => Ok(()),
        Some(false) | None => Err(DeviceError::Internal("home_widget")),
    }
}

/// Android 実ホーム画面ウィジェット実装。
#[cfg(target_os = "android")]
pub struct AndroidHomeWidget;

#[cfg(target_os = "android")]
impl AndroidHomeWidget {
    /// Activity インスタンスへ `f` を実行する共通処理（唯一の `unsafe` 集約点）。
    /// `AndroidSecureCredential::request` と同じ根拠・同じ形。`android_app()` が `None` なら
    /// `Unsupported`、JNI 失敗は `Internal`（Java 例外は clear し文言は読まない）。
    #[allow(unsafe_code)]
    fn with_activity<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &jni::objects::JObject,
        ) -> Result<T, super::android::JniCallFailed>,
    ) -> Result<T, DeviceError> {
        let Some(app) = crate::android::android_app() else {
            return Err(DeviceError::Unsupported("home_widget"));
        };
        // SAFETY: `AndroidSecureCredential::request` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` を返し、`android_app()` は最新の
        // `AndroidApp` を複製して返すため、`app` は呼び出し時点で有効な `Activity` を指す。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 同上。`activity_as_ptr()` は `AndroidApp` 生存中有効な未所有の
            // グローバル参照で、`Cast` は所有権を持たないため delete を試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let result = f(env, &activity);
            if result.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            result
        })
        .map_err(|_| DeviceError::Internal("home_widget"))
    }

    fn call_set(text: &str) -> Result<bool, DeviceError> {
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

    fn call_get() -> Result<Option<String>, DeviceError> {
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
impl fandhe_device::home_widget::HomeWidget for AndroidHomeWidget {
    fn set_text(&self, text: &str) -> Result<(), DeviceError> {
        fandhe_device::home_widget::validate_text(text)?;
        match Self::call_set(text) {
            Ok(committed) => set_outcome(Some(committed)),
            // `Unsupported`（Activity なし）は握り込まずそのまま返す。
            Err(DeviceError::Internal(_)) => set_outcome(None),
            Err(e) => Err(e),
        }
    }

    fn current_text(&self) -> Result<Option<String>, DeviceError> {
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
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(set_outcome(None), Err(DeviceError::Internal(_))));
    }
}
