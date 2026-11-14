//! Android 実資格情報保存実装（`EncryptedSharedPreferences`、Issue #924）。`driver::platform_secure_credential` の
//! Android 経路から使われ、ドライバが別スレッドで呼ぶ（「別スレッドで OS を呼ぶ理由」は `driver` doc 参照）。
//!
//! Java 側は `SecureCredentialHelper.request(Context, int, String, String)`（static）。同期 API を直接呼ぶだけで
//! 非同期コールバックを持たないため、JNI 呼び出しの戻り値をそのまま同期で返す（mailbox・JNI 受け口は無い）。
//! Activity のライフサイクルに参加しないため provider も不要。
//!
//! Java 側は保存値に固定プレフィックス `"v1:"` を付けて格納する（`EncryptedSharedPreferences` が復号後の
//! `"__NULL__"` を null 番兵として扱うため。codex-review 指摘、PR #931）。ここへ届く `Secret` は Java 側で
//! 既にプレフィックスを除去済みで、この契約を意識する必要はない。
//!
//! `unsafe` は共通層 `fandhe_platform_android::with_activity_class` に閉じており、本ファイルには無い。

use crate::contract::{Secret, SecureCredential, validate_key, validate_secret};
use crate::error::SecureCredentialError;

/// Java 側 `SecureCredentialHelper.request` の `op` 引数（Java 側の同名定数と対で決める契約）。
const SECURE_CREDENTIAL_OP_STORE: i32 = 0;
const SECURE_CREDENTIAL_OP_LOAD: i32 = 1;
const SECURE_CREDENTIAL_OP_DELETE: i32 = 2;

/// Android 実資格情報保存実装。
pub struct AndroidSecureCredential;

impl AndroidSecureCredential {
    /// `SecureCredentialHelper.request(Context, int, String, String)` を呼ぶ。
    /// `value` は STORE 以外では空文字列を渡す（Java 側は `op` で無視する）。
    #[cfg(target_os = "android")]
    fn request(
        &self,
        op: i32,
        key: &str,
        value: &str,
    ) -> Result<Option<String>, SecureCredentialError> {
        use fandhe_platform_android::{JniCallFailed, with_activity_class};
        let Some(app) = fandhe_platform_android::android_app() else {
            return Err(SecureCredentialError::Unsupported);
        };
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.SecureCredentialHelper"),
            |env, activity, class| -> Result<Option<String>, JniCallFailed> {
                let j_key = env.new_string(key)?;
                let j_value = env.new_string(value)?;
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("request"),
                    jni::jni_sig!((android.content.Context, int, java.lang.String, java.lang.String) -> java.lang.String),
                    &[
                        jni::objects::JValue::Object(activity),
                        jni::objects::JValue::Int(op),
                        jni::objects::JValue::Object(&j_key),
                        jni::objects::JValue::Object(&j_value),
                    ],
                );
                let ret = match ret {
                    Ok(v) => v,
                    Err(e) => {
                        // Java 側が `IllegalStateException` を投げた場合（内部エラー）、例外は自動クリアされない
                        // （jni 0.22 の仕様）ため、後続の JNI 呼び出しに影響しないよう明示的にクリアする。
                        // 文言は読まない（security.md A09、値・例外メッセージをログに残さない契約）。
                        if env.exception_check() {
                            env.exception_clear();
                        }
                        return Err(e.into());
                    }
                };
                let obj = ret.l()?;
                let jstring = env.cast_local::<jni::objects::JString>(obj)?;
                if jstring.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(jstring.try_to_string(env)?))
                }
            },
        )
        .map_err(|_| SecureCredentialError::Internal("secure_credential: jni call failed"))
    }

    /// ホストのテストビルド用。Android 以外では呼ばれず、成功へは倒さない。
    #[cfg(not(target_os = "android"))]
    fn request(
        &self,
        _op: i32,
        _key: &str,
        _value: &str,
    ) -> Result<Option<String>, SecureCredentialError> {
        Err(SecureCredentialError::Unsupported)
    }
}

impl SecureCredential for AndroidSecureCredential {
    fn store(&self, key: &str, secret: &str) -> Result<(), SecureCredentialError> {
        let key = validate_key(key)?;
        let secret = validate_secret(secret)?;
        self.request(SECURE_CREDENTIAL_OP_STORE, key, secret)
            .map(|_| ())
    }

    fn load(&self, key: &str) -> Result<Option<Secret>, SecureCredentialError> {
        let key = validate_key(key)?;
        self.request(SECURE_CREDENTIAL_OP_LOAD, key, "")
            .map(|opt| opt.map(Secret::new))
    }

    fn delete(&self, key: &str) -> Result<(), SecureCredentialError> {
        let key = validate_key(key)?;
        self.request(SECURE_CREDENTIAL_OP_DELETE, key, "")
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// op 定数が Java 側 `SecureCredentialHelper` の同名定数と対で決めた値からずれていないことを固定する
    /// （値そのものは JNI 越しに渡るだけで型検査が効かないため）。
    #[test]
    fn op_constants_match_java_side_contract() {
        assert_eq!(SECURE_CREDENTIAL_OP_STORE, 0);
        assert_eq!(SECURE_CREDENTIAL_OP_LOAD, 1);
        assert_eq!(SECURE_CREDENTIAL_OP_DELETE, 2);
    }

    /// 入力検証は OS を呼ぶ前に済む（ホストでは OS 呼び出しが `Unsupported` になるため、検証エラーとの区別で確かめる）。
    #[test]
    fn invalid_key_is_rejected_before_the_os_call() {
        assert!(matches!(
            AndroidSecureCredential.load("a/b"),
            Err(SecureCredentialError::InvalidKey(_))
        ));
    }
}
