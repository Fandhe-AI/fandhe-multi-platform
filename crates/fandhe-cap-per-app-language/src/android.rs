//! Android 向けアプリ別の言語設定の実装（Issue #1075、親 #137）。`driver` の
//! Android 経路（`platform_per_app_language`）から egui スレッドでボタン押下ごとに同期で 1 回
//! 呼ばれる。契約は `crate::contract`。
//!
//! Java 側 `PerAppLanguageHelper.read(Context)`（static、`LocaleManager`、JNI シグネチャ
//! `(Landroid/content/Context;)Ljava/lang/String;`）を `with_activity_class` 経由で呼ぶ。戻り値の符号化は次のとおりで、
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

use crate::contract::{AppLanguage, LanguageSource, clean_tag};
use crate::error::PerAppLanguageError;

/// Java の戻り値（null は `None`）を契約の値へ写す。
fn from_android(raw: Option<String>) -> Result<AppLanguage, PerAppLanguageError> {
    const FAILED: PerAppLanguageError = PerAppLanguageError::Internal;
    let s = raw.ok_or(FAILED)?;
    if s == "unsupported" {
        return Err(PerAppLanguageError::Unsupported);
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
    use crate::contract::{AppLanguage, PerAppLanguage};
    use crate::error::PerAppLanguageError;
    use fandhe_platform_android::JniCallFailed;

    /// Android の `PerAppLanguage` 実装（`PerAppLanguageHelper.read(Context)`（static）を JNI で呼ぶ）。
    pub(super) struct AndroidPerAppLanguage;

    impl PerAppLanguage for AndroidPerAppLanguage {
        fn read(&self) -> Result<AppLanguage, PerAppLanguageError> {
            const FAILED: PerAppLanguageError = PerAppLanguageError::Internal;
            let Some(app) = fandhe_platform_android::android_app() else {
                return Err(FAILED);
            };
            // unsafe は `with_activity_class` に集約されている。クラスは Activity の class loader で解決する。
            let result = fandhe_platform_android::with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.PerAppLanguageHelper"),
                |env, activity, class| -> Result<Option<String>, JniCallFailed> {
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.content.Context) -> java.lang.String),
                        &[jni::objects::JValue::Object(activity)],
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
pub(crate) fn implementation() -> Box<dyn crate::contract::PerAppLanguage> {
    Box::new(jni_impl::AndroidPerAppLanguage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_internal(r: Result<AppLanguage, PerAppLanguageError>) -> bool {
        matches!(r, Err(PerAppLanguageError::Internal))
    }

    #[test]
    fn none_and_unsupported() {
        assert!(is_internal(from_android(None)));
        assert!(matches!(
            from_android(Some("unsupported".into())),
            Err(PerAppLanguageError::Unsupported)
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
