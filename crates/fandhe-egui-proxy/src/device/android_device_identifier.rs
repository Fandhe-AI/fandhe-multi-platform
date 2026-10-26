//! Android 向けアプリ・端末の識別子実装（Issue #904、親 #181）。`device/device_identifier.rs` の
//! Android 経路（`platform_device_identifier`）から egui スレッドでボタン押下ごとに同期で 1 回
//! 呼ばれる。契約は `fandhe_device::device_identifier`。
//!
//! Java 側 `MainActivity.deviceIdentifierRead()`（JNI シグネチャ `()Ljava/lang/String;`）が
//! `DeviceIdentifierHelper.read`（`Settings.Secure.ANDROID_ID`）へ委譲する。文字列から契約の値への
//! 写像は純関数 [`from_android`] に置き、ホストでもテストする。
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

use fandhe_device::device_identifier::{DeviceId, IdentifierRead};
use fandhe_device::DeviceError;

/// Java の戻り値（null は `None`）を契約の値へ写す。空文字は `DeviceId::new` が `Internal` にする。
fn from_android(raw: Option<String>) -> Result<IdentifierRead, DeviceError> {
    let s = raw.ok_or(DeviceError::Internal("device_identifier read"))?;
    DeviceId::new(s).map(IdentifierRead::Identifier)
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::device::android::JniCallFailed;
    use fandhe_device::device_identifier::{DeviceIdentifier, IdentifierRead};
    use fandhe_device::DeviceError;

    /// Android の `DeviceIdentifier` 実装（`MainActivity.deviceIdentifierRead()` を JNI で呼ぶ）。
    pub(super) struct AndroidDeviceIdentifier;

    impl DeviceIdentifier for AndroidDeviceIdentifier {
        /// 2 箇所の `unsafe` は `AndroidDeviceInfo::read` と同じ根拠・同じ形。
        #[allow(unsafe_code)]
        fn read(&self) -> Result<IdentifierRead, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("device_identifier read");
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
                        jni::jni_str!("deviceIdentifierRead"),
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

/// Android の `DeviceIdentifier` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::device_identifier::DeviceIdentifier> {
    Box::new(jni_impl::AndroidDeviceIdentifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_and_empty_are_internal() {
        assert!(matches!(from_android(None), Err(DeviceError::Internal(_))));
        assert!(matches!(
            from_android(Some(String::new())),
            Err(DeviceError::Internal(_))
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
