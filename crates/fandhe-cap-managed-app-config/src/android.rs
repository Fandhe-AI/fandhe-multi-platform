//! Android 向け MDM 管理アプリ設定の読み取り実装（Issue #946、親 #185）。`driver.rs` の
//! Android 経路から、押下ごとにワーカースレッドで 1 回呼ばれる。契約は
//! `crate::contract`（固定 3 キーを読むだけ。購読・定期 pull はしない）。
//!
//! Java 側 static の `ManagedAppConfigHelper.read(Context)`（JNI シグネチャ
//! `(Landroid/content/Context;)Ljava/lang/String;`、`RestrictionsManager.getApplicationRestrictions()`）が
//! ワイヤ文字列を返す。詰め方の SSOT は `crate::contract` のモジュール doc、
//! 文字列から契約の値への写像は `parse_wire`。ここの純関数 [`decode`] はホストでもテストする。
//!
//! - null・JNI 失敗・例外・`android_app()` 無しは `Internal(READ_FAILED)`。「設定なし」へ畳まない
//! - 制限なし（Bundle が null・空）は Java が `NoConfig` になるワイヤを返す
//! - 同期の一発読み取りで保留中の要求・コールバック・購読を持たない。Activity 再生成時に破棄すべき
//!   ものは無く、古い結果が新しい要求の id を消費する経路も無い（世代照合は driver と共通コア側）
//! - 管理値の中身・例外の文言はログにも出さない
//!
//! 守る範囲: 1 プロセス内の取得、失敗・型違い・未設定の区別。守らない範囲: 読み取り後の変化の反映
//! （再押下まで）、API 34+ の Supervision 管理者の値、OS 呼び出しが戻らない場合の期限。
//!
//! 一次情報:
//! <https://developer.android.com/reference/android/content/RestrictionsManager#getApplicationRestrictions()>、
//! <https://developer.android.com/work/managed-configurations>

use std::sync::Arc;

use crate::contract::ManagedAppConfig;
#[cfg(any(target_os = "android", test))]
use crate::contract::{READ_FAILED, Report, parse_wire};
#[cfg(any(target_os = "android", test))]
use crate::error::ManagedAppConfigError;

/// JNI の戻り値（`None` は null・呼び出し失敗）を契約上の結果へ写す。失敗を成功へ倒さない。
#[cfg(any(target_os = "android", test))]
fn decode(raw: Option<String>) -> Result<Report, ManagedAppConfigError> {
    parse_wire(&raw.ok_or(ManagedAppConfigError::Internal(READ_FAILED))?)
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::decode;
    use crate::contract::{ManagedAppConfig, Report};
    use crate::error::ManagedAppConfigError;
    use fandhe_platform_android::JniCallFailed;

    /// Android の `ManagedAppConfig` 実装（`ManagedAppConfigHelper.read` を JNI で呼ぶ）。
    pub(super) struct AndroidManagedAppConfig;

    impl ManagedAppConfig for AndroidManagedAppConfig {
        fn read(&self) -> Result<Report, ManagedAppConfigError> {
            use fandhe_platform_android::{android_app, with_activity_class};
            use jni::objects::JValue;
            let Some(app) = android_app() else {
                return decode(None);
            };
            // クラスは Activity の class loader で解決する（`with_activity_class`）。
            let result = with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.ManagedAppConfigHelper"),
                |env, activity, class| -> Result<Option<String>, JniCallFailed> {
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.content.Context) -> java.lang.String),
                        &[JValue::Object(activity)],
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

            decode(result.ok().flatten())
        }
    }
}

/// Android の `ManagedAppConfig` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn ManagedAppConfig> {
    Arc::new(jni_impl::AndroidManagedAppConfig)
}

/// ホストのテストビルド向け。dispatch は Android でしか呼ばない。非対応を返し成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn ManagedAppConfig> {
    Arc::new(crate::contract::UnsupportedManagedAppConfig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_internal_not_no_config() {
        assert!(matches!(
            decode(None),
            Err(ManagedAppConfigError::Internal(READ_FAILED))
        ));
    }

    #[test]
    fn empty_restrictions_is_no_config() {
        assert!(matches!(
            decode(Some("-\u{1f}-\u{1f}-\u{1f}0".into())),
            Ok(Report::NoConfig)
        ));
    }

    #[test]
    fn counts_other_keys() {
        let r = decode(Some("1\u{1f}-42\u{1f}5\u{1f}2".into())).unwrap();
        assert!(matches!(
            r,
            Report::Config {
                other_keys: Some(2),
                ..
            }
        ));
    }

    #[test]
    fn malformed_is_internal() {
        for raw in ["", "1", "1\u{1f}2\u{1f}3", "x\u{1f}-\u{1f}-\u{1f}0"] {
            assert!(
                matches!(
                    decode(Some(raw.into())),
                    Err(ManagedAppConfigError::Internal(READ_FAILED))
                ),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn host_impl_is_unsupported() {
        assert!(matches!(
            implementation().read(),
            Err(ManagedAppConfigError::Unsupported(_))
        ));
    }
}
