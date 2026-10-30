//! Android 向けアプリ別の言語設定の実装（Issue #1075、親 #137）。`device/per_app_language.rs` の
//! Android 経路（`platform_per_app_language`）から egui スレッドでボタン押下ごとに同期で 1 回
//! 呼ばれる。契約は `fandhe_device::per_app_language`。
//!
//! Java 側 `MainActivity.perAppLanguageRead()`（JNI シグネチャ `()Ljava/lang/String;`）が
//! `PerAppLanguageHelper.read`（`LocaleManager`）へ委譲する。戻り値の符号化は次のとおりで、
//! 写像は純関数 [`from_android`] に置きホストでもテストする（Java 側の値と一致させること）。
//!
//! | 戻り値 | 意味 |
//! |---|---|
//! | `"unsupported"` | API 33 未満 → `Unsupported` |
//! | `"app:<tag>"` | `getApplicationLocales()` が空でない → `PerApp` |
//! | `"system:<tag>"` | 空 → `getSystemLocales()` の先頭 → `System`（成功） |
//! | null・未知の接頭辞・空タグ | `Internal` |
//!
//! - `getSystemLocales()` の空・`LocaleManager` null・例外・JNI 失敗・`android_app()` 無し
//!   （Activity 準備前）は `Internal`。タグ検査の SSOT は Rust 側の `clean_tag`
//! - 同期の一発読み取りで保留中の要求・要求 id・コールバック・購読を持たないので、Activity 再生成時に
//!   破棄すべきものは無い（旧結果が新しい要求を消費することも構造上ない）
//! - 権限は不要。タグも生の文字列もログに出さない
//!
//! 守る範囲: 嘘の値を出さない・1 プロセス内の取得・押下ごとに OS へ聞く。守らない範囲: 読み取り後の
//! 変更の反映（再読込まで）、アプリ自身の文言の切り替え、複数プロセス。実機の確認は #1054。

use fandhe_device::per_app_language::{clean_tag, AppLanguage, LanguageSource};
use fandhe_device::DeviceError;

/// Java の戻り値（null は `None`）を契約の値へ写す。
fn from_android(raw: Option<String>) -> Result<AppLanguage, DeviceError> {
    const FAILED: DeviceError = DeviceError::Internal("per_app_language read");
    let s = raw.ok_or(FAILED)?;
    if s == "unsupported" {
        return Err(DeviceError::Unsupported("per_app_language"));
    }
    let (source, tag) = if let Some(t) = s.strip_prefix("app:") {
        (LanguageSource::PerApp, t)
    } else if let Some(t) = s.strip_prefix("system:") {
        (LanguageSource::System, t)
    } else {
        return Err(FAILED);
    };
    let tag = clean_tag(tag).ok_or(FAILED)?;
    Ok(AppLanguage { tag, source })
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::device::android::JniCallFailed;
    use fandhe_device::per_app_language::{AppLanguage, PerAppLanguage};
    use fandhe_device::DeviceError;

    /// Android の `PerAppLanguage` 実装（`MainActivity.perAppLanguageRead()` を JNI で呼ぶ）。
    pub(super) struct AndroidPerAppLanguage;

    impl PerAppLanguage for AndroidPerAppLanguage {
        /// 2 箇所の `unsafe` は `AndroidDeviceIdentifier::read` と同じ根拠・同じ形。
        #[allow(unsafe_code)]
        fn read(&self) -> Result<AppLanguage, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("per_app_language read");
            let Some(app) = crate::android::android_app() else {
                return Err(FAILED);
            };

            // SAFETY: `AndroidTextScale::scale_factor` と同一の根拠。`vm_as_ptr()` は
            // android-activity が管理する有効な `JavaVM*`、`app` は呼び出し中生存する。
            let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

            let result = vm.attach_current_thread(
                |env: &mut jni::Env| -> Result<Option<String>, JniCallFailed> {
                    let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                    // SAFETY: `AndroidTextScale::scale_factor` と同一の根拠。未所有の JNI
                    // グローバル参照を `Cast` で包むだけで delete は試みない。
                    let activity =
                        unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                    let ret = env.call_method(
                        &activity,
                        jni::jni_str!("perAppLanguageRead"),
                        jni::jni_sig!(() -> java.lang.String),
                        &[],
                    );
                    let ret = match ret {
                        Ok(v) => v,
                        Err(e) => {
                            // 例外は自動クリアされない（jni 0.22）。文言は読まない。
                            if env.exception_check() {
                                env.exception_clear();
                            }
                            return Err(e.into());
                        }
                    };
                    let jstring = env.cast_local::<jni::objects::JString>(ret.l()?)?;
                    if jstring.is_null() {
                        Ok(None)
                    } else {
                        Ok(Some(jstring.try_to_string(env)?))
                    }
                },
            );

            match result {
                Ok(raw) => from_android(raw),
                Err(_) => Err(FAILED),
            }
        }
    }
}

/// Android の `PerAppLanguage` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::per_app_language::PerAppLanguage> {
    Box::new(jni_impl::AndroidPerAppLanguage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_internal(r: Result<AppLanguage, DeviceError>) -> bool {
        matches!(r, Err(DeviceError::Internal(_)))
    }

    #[test]
    fn none_and_unsupported() {
        assert!(is_internal(from_android(None)));
        assert!(matches!(
            from_android(Some("unsupported".into())),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn app_and_system() {
        let a = from_android(Some("app:ja".into())).unwrap();
        assert_eq!((a.tag.as_str(), a.source), ("ja", LanguageSource::PerApp));
        let s = from_android(Some("system:ja-JP".into())).unwrap();
        assert_eq!(
            (s.tag.as_str(), s.source),
            ("ja-JP", LanguageSource::System)
        );
    }

    #[test]
    fn invalid_values_are_internal() {
        let long = format!("app:{}", "a".repeat(36));
        for raw in [
            "system:",
            "app:",
            "app:ja jp",
            &long,
            "other:ja",
            "",
            "APP:ja",
            " app:ja",
        ] {
            assert!(is_internal(from_android(Some(raw.into()))), "{raw:?}");
        }
    }
}
