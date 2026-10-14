//! Android 向け音声デバイス一覧実装（Issue #1332）。`device/audio_devices.rs` の Android 経路
//! （`drive_sync`）から egui スレッドでボタン押下ごとに同期で 1 回呼ばれる。契約は
//! `fandhe_device::audio_devices`（範囲は接続中デバイス名の一覧のみ。選択・切り替え・音量は範囲外）。
//!
//! Java 側 `MainActivity.audioDevicesList()`（JNI シグネチャ `()Ljava/lang/String;`）が
//! `AudioDevicesHelper.list` へ委譲し、入力・出力の名前を U+001D 区切りの 2 区画で返す。
//! 詰め方の SSOT は `AudioDevicesHelper.java` 冒頭。文字列から契約の値への写像は純関数
//! [`from_android`] に置き、ホストでもテストする。
//!
//! - 区画が空は取得不可（`Unavailable`）、`"0"` は 0 件（`Listed`）。両者を区別する
//! - `android_app()` が無い・JNI 失敗・null・形が不正は `Internal`（起動直後の一時状態で
//!   「非対応」と出さないため。`android_device_info` と同判断）。両方向とも取れなければ
//!   `require_listed` が `Internal`。失敗を成功値へ畳まない
//! - `getAddress()`（MAC を含む）・`getId()` は読まず、権限も宣言しない。名前はログに出さない
//! - 同期の一発読み取りで保留中の要求・コールバック・購読を持たないので、Activity 再生成時に
//!   破棄すべきものは無い（古い結果が新しい要求を消費する経路も無い。世代照合は共通コア側）
//!
//! 守る範囲: 1 プロセス内の取得、0 件と取得不可の区別、失敗の非隠蔽。守らない範囲: 読み取り後の
//! 接続・取り外しの反映（再取得まで）、OS 間の名前の表記の一致（内蔵機器は機種名で並ぶ）。
//!
//! 一次情報:
//! <https://developer.android.com/reference/android/media/AudioManager#getDevices(int)>、
//! <https://developer.android.com/reference/android/media/AudioDeviceInfo>

use fandhe_device::audio_devices::{listed, AudioDeviceList, AudioDirection};

/// Java が項目をつなぐ区切り文字（U+001F）。
const SEP: char = '\u{1F}';
/// Java が入力区画と出力区画をつなぐ区切り文字（U+001D）。
const GROUP: char = '\u{1D}';

/// 1 区画を方向へ写す。空は `Unavailable`、件数と名前の個数が合わなければ `None`。
fn parse_section(section: &str) -> Option<AudioDirection> {
    if section.is_empty() {
        return Some(AudioDirection::Unavailable);
    }
    let mut it = section.split(SEP);
    let count: usize = it.next()?.parse().ok()?;
    let names: Vec<Option<String>> = it.map(|s| Some(s.to_string())).collect();
    (names.len() == count).then(|| listed(names))
}

/// `AudioDevicesHelper` の文字列を契約の値へ写す。区画数が 2 でない・区画が不正なら `None`（失敗）。
fn from_android(raw: &str) -> Option<AudioDeviceList> {
    let parts: Vec<&str> = raw.split(GROUP).collect();
    let [inputs, outputs] = parts[..] else {
        return None;
    };
    Some(AudioDeviceList {
        inputs: parse_section(inputs)?,
        outputs: parse_section(outputs)?,
    })
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::device::android::JniCallFailed;
    use fandhe_device::audio_devices::{require_listed, AudioDeviceList, AudioDevices};
    use fandhe_device::DeviceError;

    /// Android の `AudioDevices` 実装（`MainActivity.audioDevicesList()` を JNI で呼ぶ）。
    pub(super) struct AndroidAudioDevices;

    impl AudioDevices for AndroidAudioDevices {
        /// 2 箇所の `unsafe` は `AndroidDeviceInfo::read` と同じ根拠・同じ形。
        #[allow(unsafe_code)]
        fn list(&self) -> Result<AudioDeviceList, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("audio_devices list");
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
                        jni::jni_str!("audioDevicesList"),
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
            require_listed(from_android(&raw).ok_or(FAILED)?)
        }
    }
}

/// Android の `AudioDevices` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::audio_devices::AudioDevices> {
    std::sync::Arc::new(jni_impl::AndroidAudioDevices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::audio_devices::{require_listed, MAX_DEVICES};

    fn names(d: &AudioDirection) -> Vec<Option<&str>> {
        match d {
            AudioDirection::Listed(v) => v.iter().map(|x| x.name.as_deref()).collect(),
            other => panic!("Listed ではない: {other:?}"),
        }
    }

    #[test]
    fn maps_both_directions() {
        let l = from_android("2\u{1F}Mic\u{1F}Headset\u{1D}1\u{1F}Speaker").unwrap();
        assert_eq!(names(&l.inputs), [Some("Mic"), Some("Headset")]);
        assert_eq!(names(&l.outputs), [Some("Speaker")]);
    }

    #[test]
    fn zero_devices_is_listed_empty() {
        let l = from_android("0\u{1D}0").unwrap();
        assert_eq!(l.inputs, AudioDirection::Listed(vec![]));
        assert!(require_listed(l).is_ok());
    }

    #[test]
    fn empty_section_is_unavailable() {
        let l = from_android("\u{1D}1\u{1F}Speaker").unwrap();
        assert_eq!(l.inputs, AudioDirection::Unavailable);
        assert_eq!(names(&l.outputs), [Some("Speaker")]);
        assert!(require_listed(l).is_ok());
    }

    #[test]
    fn both_unavailable_is_internal_error() {
        let l = from_android("\u{1D}").unwrap();
        assert!(require_listed(l).is_err());
    }

    #[test]
    fn malformed_fails() {
        for raw in [
            "",
            "0",
            "0\u{1D}0\u{1D}0",
            "2\u{1F}a\u{1D}0",
            "x\u{1D}0",
            "1\u{1D}0",
        ] {
            assert!(from_android(raw).is_none(), "{raw:?}");
        }
    }

    #[test]
    fn empty_name_counts_and_dupes_kept() {
        let l = from_android("3\u{1F}\u{1F}A\u{1F}A\u{1D}0").unwrap();
        assert_eq!(names(&l.inputs), [None, Some("A"), Some("A")]);
    }

    #[test]
    fn capped_and_cleaned() {
        let many = format!("70{}", "\u{1F}x".repeat(70));
        let l = from_android(&format!("{many}\u{1D}1\u{1F}\u{7}Pixel ")).unwrap();
        assert_eq!(names(&l.inputs).len(), MAX_DEVICES);
        assert_eq!(names(&l.outputs), [Some("Pixel")]);
    }
}
