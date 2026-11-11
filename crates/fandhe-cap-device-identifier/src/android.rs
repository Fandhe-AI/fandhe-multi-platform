//! Android 向けアプリ・端末の識別子実装（Issue #904、親 #181。機能 crate へ移行、Issue #1183）。
//! `driver.rs` の Android 経路（`platform_device_identifier`）から egui スレッドでボタン押下ごとに同期で 1 回
//! 呼ばれる。契約は `crate::contract`。
//!
//! `DeviceIdentifierHelper.read(Context)`（static。`Settings.Secure.ANDROID_ID`）を JNI で直接呼ぶ
//! （利用者の `MainActivity` に受け口は要らない）。JNI の署名は Java 側と本ファイルの 2 か所で一致させる:
//! `read (Landroid/content/Context;)Ljava/lang/String;`。Java に `native` 宣言は無く Rust から Java への
//! 一方向なので no-op スタブは置かない。AndroidApp の取得と JavaVM への attach は
//! `fandhe-platform-android` の `with_activity_class` に任せ、本ファイルに `unsafe` は無い。
//! 文字列から契約の値への写像は純関数 [`from_android`] に置き、ホストでもテストする。
//!
//! - null・空・例外・JNI 失敗・`android_app()` 無し（Activity 準備前）は `Internal`。
//!   `Unsupported` にすると起動直後の一時状態で「非対応」と出るため（`android_device_info` と同判断）
//! - 値は trim しない。`NotYetAvailable` は返さない（Android に nil 相当は無い）
//! - 同期の一発読み取りで保留中の要求・コールバック・購読を持たないので、Activity 再生成時に
//!   破棄すべきものは無い
//! - 権限は不要。広告 ID・IMEI・シリアル・MAC は読まない。値はログに出さない
//!
//! 守る範囲: 1 プロセス内の取得、null・空・失敗を成功へ畳まないこと。守らない範囲: 値がいつ
//! 変わるか、Android 7 でのアプリ間共有、読み取り後の変化の反映。実機の確認は #889。

use crate::contract::{DeviceId, IdentifierRead};
use crate::error::DeviceIdentifierError;

/// Java の戻り値（null は `None`）を契約の値へ写す。空文字は `DeviceId::new` が `Internal` にする。
fn from_android(raw: Option<String>) -> Result<IdentifierRead, DeviceIdentifierError> {
    let s = raw.ok_or(DeviceIdentifierError::Internal)?;
    DeviceId::new(s).map(IdentifierRead::Identifier)
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::contract::{DeviceIdentifier, IdentifierRead};
    use crate::error::DeviceIdentifierError;
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};

    /// Android の `DeviceIdentifier` 実装（`DeviceIdentifierHelper.read(Context)` を JNI の static 呼び出しで読む）。
    pub(super) struct AndroidDeviceIdentifier;

    impl DeviceIdentifier for AndroidDeviceIdentifier {
        fn read(&self) -> Result<IdentifierRead, DeviceIdentifierError> {
            const FAILED: DeviceIdentifierError = DeviceIdentifierError::Internal;
            let Some(app) = android_app() else {
                return Err(FAILED);
            };
            let result = with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.DeviceIdentifierHelper"),
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

/// Android の `DeviceIdentifier` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn crate::contract::DeviceIdentifier> {
    Box::new(jni_impl::AndroidDeviceIdentifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_and_empty_are_internal() {
        assert!(matches!(
            from_android(None),
            Err(DeviceIdentifierError::Internal)
        ));
        assert!(matches!(
            from_android(Some(String::new())),
            Err(DeviceIdentifierError::Internal)
        ));
    }

    #[test]
    fn value_is_masked() {
        let r = from_android(Some("0123456789abcdef".to_string())).unwrap();
        let IdentifierRead::Identifier(id) = r else {
            panic!("expected Identifier");
        };
        assert_eq!(id.char_count(), 16);
        assert_eq!(id.masked_tail(), "cdef");
    }

    #[test]
    fn not_trimmed() {
        let IdentifierRead::Identifier(id) = from_android(Some(" ab ".to_string())).unwrap() else {
            panic!("expected Identifier");
        };
        assert_eq!(id.char_count(), 4);
    }
}
