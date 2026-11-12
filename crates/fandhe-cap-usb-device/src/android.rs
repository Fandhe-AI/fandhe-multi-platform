//! Android 向け USB 機器一覧実装（Issue #507、親 #112）。`driver.rs` の Android 経路
//! （`drive_sync`）から egui スレッドでボタン押下ごとに同期で 1 回呼ばれる。契約は
//! `crate::contract`（範囲は接続中機器の VID・PID・製品名の一覧のみ。機器のオープン・
//! 転送・抜き差しへの追従・接続許可の要求は範囲外）。
//!
//! Java 側 `UsbDeviceHelper.list(Context)`（static、JNI シグネチャ
//! `(Landroid/content/Context;)Ljava/lang/String;`）を `MainActivity` を介さず呼ぶ。詰め方の SSOT は `UsbDeviceHelper.java` 冒頭。文字列から
//! 契約の値への写像は純関数 [`from_android`] に置き、ホストでもテストする。
//!
//! - 空文字は USB ホスト非搭載（`Unsupported`）、`"0"` は 0 件の成功。両者を区別する
//! - `android_app()` が無い・JNI 失敗・null・形が不正（件数不一致・数値でない・u16 の値域外）は
//!   `Internal`。失敗を成功値へ畳まない。値域は変換の前に検証する
//! - シリアル番号・デバイスパス・製造者名は読まず、権限も宣言しない。製品名はログに出さない
//! - 同期の一発読み取りで保留中の要求・コールバック・購読を持たないので、Activity 再生成時に
//!   破棄すべきものは無い（古い結果が新しい要求を消費する経路も無い。世代照合は共通コア側の
//!   `finish_read`）
//!
//! 守る範囲: 1 プロセス内の取得、0 件・非対応・失敗の区別、失敗の非隠蔽。守らない範囲: 読み取り後の
//! 抜き差しの反映（再取得まで）、製品名の取得に許可が要る端末での名前の欠落、同期呼び出しが戻らない
//! 場合の期限。
//!
//! 一次情報:
//! <https://developer.android.com/develop/connectivity/usb/host>、
//! <https://developer.android.com/reference/android/hardware/usb/UsbManager#getDeviceList()>

use crate::contract::{UsbDeviceEntry, normalize};
use crate::error::UsbDeviceError;

/// Java が機器レコードの頭に置く区切り文字（U+001E）。
const REC: char = '\u{1E}';
/// Java がレコード内の項目をつなぐ区切り文字（U+001F）。
const SEP: char = '\u{1F}';

/// 1 レコード（`vid SEP pid SEP 名前`）を写す。数値でない・u16 の値域外・項目数違いは `None`。
fn parse_record(rec: &str) -> Option<UsbDeviceEntry> {
    let mut it = rec.split(SEP);
    let vid: u32 = it.next()?.parse().ok()?;
    let pid: u32 = it.next()?.parse().ok()?;
    let name = it.next()?;
    if it.next().is_some() {
        return None;
    }
    Some(UsbDeviceEntry {
        vendor_id: u16::try_from(vid).ok()?,
        product_id: u16::try_from(pid).ok()?,
        name: (!name.is_empty()).then(|| name.to_string()),
    })
}

/// `UsbDeviceHelper` の文字列を契約の値へ写す。形が不正なら `None`（呼び出し側で `Internal`）。
fn from_android(raw: &str) -> Option<Result<Vec<UsbDeviceEntry>, UsbDeviceError>> {
    if raw.is_empty() {
        return Some(Err(UsbDeviceError::Unsupported));
    }
    let mut it = raw.split(REC);
    let count: usize = it.next()?.parse().ok()?;
    let entries = it.map(parse_record).collect::<Option<Vec<_>>>()?;
    (entries.len() == count).then(|| Ok(normalize(entries)))
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::contract::{UsbDeviceEntry, UsbDevices};
    use crate::error::UsbDeviceError;
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};

    /// Android の `UsbDevices` 実装（`UsbDeviceHelper.list(Context)` を JNI の static 呼び出しで呼ぶ）。
    pub(super) struct AndroidUsbDevices;

    impl UsbDevices for AndroidUsbDevices {
        /// `unsafe` は共通層 `with_activity_class` に閉じている。
        fn list(&self) -> Result<Vec<UsbDeviceEntry>, UsbDeviceError> {
            const FAILED: UsbDeviceError = UsbDeviceError::Internal;
            let Some(app) = android_app() else {
                return Err(FAILED);
            };
            let result = with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.UsbDeviceHelper"),
                |env, activity, class| -> Result<Option<String>, JniCallFailed> {
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("list"),
                        jni::jni_sig!((android.content.Context) -> java.lang.String),
                        &[jni::objects::JValue::Object(activity)],
                    )?;
                    let jstring = env.cast_local::<jni::objects::JString>(ret.l()?)?;
                    if jstring.is_null() {
                        Ok(None)
                    } else {
                        Ok(Some(jstring.try_to_string(env)?))
                    }
                },
            );
            let raw = result.ok().flatten().ok_or(FAILED)?;
            from_android(&raw).ok_or(FAILED)?
        }
    }
}

/// Android の `UsbDevices` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn crate::contract::UsbDevices> {
    Box::new(jni_impl::AndroidUsbDevices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::MAX_DEVICES;

    fn ok(raw: &str) -> Vec<UsbDeviceEntry> {
        from_android(raw).unwrap().unwrap()
    }

    #[test]
    fn maps_two_devices() {
        let v = ok("2\u{1E}1133\u{1F}49271\u{1F}Mouse\u{1E}1\u{1F}2\u{1F}");
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].vendor_id, v[0].product_id), (1133, 49271));
        assert_eq!(v[0].name.as_deref(), Some("Mouse"));
        assert_eq!(v[1].name, None);
    }

    #[test]
    fn zero_is_empty_success() {
        assert!(ok("0").is_empty());
    }

    #[test]
    fn empty_is_unsupported() {
        assert!(matches!(
            from_android(""),
            Some(Err(UsbDeviceError::Unsupported))
        ));
    }

    #[test]
    fn malformed_fails() {
        for raw in [
            "1",
            "x",
            "0\u{1E}1\u{1F}2\u{1F}a",
            "2\u{1E}1\u{1F}2\u{1F}a",
            "1\u{1E}1\u{1F}2",
            "1\u{1E}1\u{1F}2\u{1F}a\u{1F}b",
            "1\u{1E}65536\u{1F}2\u{1F}a",
            "1\u{1E}-1\u{1F}2\u{1F}a",
            "1\u{1E}\u{1F}2\u{1F}a",
        ] {
            assert!(from_android(raw).is_none(), "{raw:?}");
        }
    }

    #[test]
    fn dupes_kept_capped_and_cleaned() {
        assert_eq!(ok("2\u{1E}1\u{1F}2\u{1F}A\u{1E}1\u{1F}2\u{1F}A").len(), 2);
        let many = format!("70{}", "\u{1E}1\u{1F}2\u{1F}x".repeat(70));
        assert_eq!(ok(&many).len(), MAX_DEVICES);
        let v = ok("1\u{1E}1\u{1F}2\u{1F}\u{7}Pixel ");
        assert_eq!(v[0].name.as_deref(), Some("Pixel"));
    }
}
