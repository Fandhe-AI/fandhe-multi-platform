//! Android 向け端末・OS 情報実装（Issue #1306）。`device/device_info.rs` の Android 経路から
//! egui スレッドでボタン押下ごとに同期で 1 回呼ばれる。契約は `fandhe_device::device_info`。
//!
//! Java 側 `MainActivity.deviceInfoRead()`（JNI シグネチャ `()Ljava/lang/String;`）が
//! `DeviceInfoHelper.read` へ委譲し、OS 版・機種・メモリ総量・空き容量を U+001F 区切りの
//! 4 項目で返す。詰め方の SSOT は `DeviceInfoHelper.java` 冒頭。文字列から数値・契約の値への
//! 写像は純関数 [`from_android`] に置き、ホストでもテストする。
//!
//! - `android_app()` が無い（Activity 準備前）・JNI 失敗・null・形が不正は `Internal`。
//!   `Unsupported` にすると起動直後の一時状態で「非対応」と出るため（`android_power_state` と同判断）
//! - 全項目が不明なら `require_any` が `Internal`。失敗を成功値へ畳まない
//! - メモリは `totalMem`（カーネル予約分を除く）で広告上の RAM 容量より小さい。空き容量 0 は有効値
//! - 個体識別子（`ANDROID_ID`・シリアル・広告 ID・MAC）と端末名は読まず、権限も宣言しない
//! - 同期の一発読み取りで保留中の要求・コールバック・購読を持たないので、Activity 再生成時に
//!   破棄すべきものは無い（古い結果が新しい要求を消費する経路も無い）
//!
//! 守る範囲: 1 プロセス内の取得、不明／0／失敗の区別。守らない範囲: 読み取り後の変化の反映、
//! OS 間の表記の一致。
//!
//! 一次情報:
//! <https://developer.android.com/reference/android/os/Build>、
//! <https://developer.android.com/reference/android/app/ActivityManager.MemoryInfo>、
//! <https://developer.android.com/reference/android/os/StatFs>

use fandhe_device::device_info::{clean_text, DeviceInfoSnapshot};

/// Java が項目をつなぐ区切り文字（U+001F）。
const SEP: char = '\u{1F}';

/// `DeviceInfoHelper` の文字列を契約の値へ写す。項目数が 4 でなければ `None`（失敗）。
/// メモリ 0・数値でない値は不明、空き容量の `0` は `Some(0)`。
fn from_android(raw: &str) -> Option<DeviceInfoSnapshot> {
    let parts: Vec<&str> = raw.split(SEP).collect();
    let [version, model, memory, free] = parts[..] else {
        return None;
    };
    Some(DeviceInfoSnapshot {
        os_name: Some("Android".to_string()),
        os_version: clean_text(version),
        model: clean_text(model),
        memory_bytes: memory.parse::<u64>().ok().filter(|&n| n > 0),
        free_storage_bytes: free.parse::<u64>().ok(),
    })
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::device::android::JniCallFailed;
    use fandhe_device::device_info::{require_any, DeviceInfo, DeviceInfoSnapshot};
    use fandhe_device::DeviceError;

    /// Android の `DeviceInfo` 実装（`MainActivity.deviceInfoRead()` を JNI で呼ぶ）。
    pub(super) struct AndroidDeviceInfo;

    impl DeviceInfo for AndroidDeviceInfo {
        /// 2 箇所の `unsafe` は `AndroidPowerState::read` と同じ根拠・同じ形。
        #[allow(unsafe_code)]
        fn read(&self) -> Result<DeviceInfoSnapshot, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("device_info read");
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
                        jni::jni_str!("deviceInfoRead"),
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

            let raw = result.ok().flatten().ok_or(FAILED)?;
            require_any(from_android(&raw).ok_or(FAILED)?)
        }
    }
}

/// Android の `DeviceInfo` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::device_info::DeviceInfo> {
    Box::new(jni_impl::AndroidDeviceInfo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::device_info::require_any;

    fn join(p: [&str; 4]) -> String {
        p.join("\u{1F}")
    }

    #[test]
    fn maps_four_fields() {
        let s = from_android(&join(["14", "Google Pixel 8", "8000000000", "1234"])).unwrap();
        assert_eq!(s.os_name.as_deref(), Some("Android"));
        assert_eq!(s.os_version.as_deref(), Some("14"));
        assert_eq!(s.model.as_deref(), Some("Google Pixel 8"));
        assert_eq!(s.memory_bytes, Some(8_000_000_000));
        assert_eq!(s.free_storage_bytes, Some(1234));
    }

    #[test]
    fn zero_free_is_valid_but_empty_is_unknown() {
        let z = from_android(&join(["14", "m", "1", "0"])).unwrap();
        assert_eq!(z.free_storage_bytes, Some(0));
        let e = from_android(&join(["14", "m", "1", ""])).unwrap();
        assert_eq!(e.free_storage_bytes, None);
    }

    #[test]
    fn bad_memory_is_unknown() {
        for m in ["0", "-5", "abc", ""] {
            let s = from_android(&join(["14", "m", m, "1"])).unwrap();
            assert_eq!(s.memory_bytes, None);
        }
    }

    #[test]
    fn wrong_field_count_fails() {
        assert!(from_android("a\u{1F}b\u{1F}c").is_none());
        assert!(from_android(&format!("{}\u{1F}x", join(["a", "b", "1", "1"]))).is_none());
        assert!(from_android("").is_none());
    }

    #[test]
    fn text_is_cleaned() {
        let s = from_android(&join(["  14\n", "\u{7}Pixel ", "1", "1"])).unwrap();
        assert_eq!(s.os_version.as_deref(), Some("14"));
        assert_eq!(s.model.as_deref(), Some("Pixel"));
    }

    #[test]
    fn all_unknown_is_internal_error() {
        let s = from_android(&join(["", "", "", ""])).unwrap();
        assert_eq!(s.os_name.as_deref(), Some("Android"));
        assert!(require_any(s).is_err());
    }
}
