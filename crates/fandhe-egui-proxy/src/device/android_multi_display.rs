//! Android 向け複数ディスプレイ実装（Issue #1343、親 #1134）。`device/multi_display.rs` の
//! Android 経路から egui スレッドが 1 秒ごとに同期で呼ぶ。契約は `fandhe_device::multi_display`。
//!
//! Java 側 `MainActivity.multiDisplayRead()`（JNI シグネチャ `()Ljava/lang/String;`）が
//! `MultiDisplayHelper.read` へ委譲し、`DisplayManager` の画面一覧を返す。詰め方の SSOT は
//! `MultiDisplayHelper.java` 冒頭（画面は U+001E、項目は U+001F 区切りの 5 項目）。文字列から
//! 契約の値への写像は純関数 [`from_android`] に置き、ホストでもテストする。
//!
//! - `android_app()` が無い・JNI 失敗・null・形が不正は `Internal`（起動直後の一時状態で
//!   「非対応」と出さないため。`android_device_info` と同判断）
//! - 空一覧・サイズ 0・主画面の正規化・空名は呼び出し側の `validate` が扱う
//! - 一部の画面だけ読めた一覧で個数を偽らず、形が不正なら全体を失敗にする
//! - 個体識別子（`getUniqueId`・`DisplayAddress`）は読まず、権限も宣言しない
//! - 同期の一発読み取りで保留中の要求・id・コールバックを持たないので、Activity 再生成時に
//!   破棄すべきものは無い。`DisplayListener`・`Presentation` は使わない（契約が 1 秒 pull のため）
//!
//! 守る範囲: 1 プロセス内の取得、失敗・空・不正を成功へ畳まないこと。守らない範囲: 反映遅れ
//! （最大 1 秒）、仮想ディスプレイ・`getRealMetrics` の論理サイズと物理パネルの差、複数プロセス、
//! 画面の位置・並び。
//!
//! 一次情報: <https://developer.android.com/reference/android/hardware/display/DisplayManager>

use fandhe_device::multi_display::ScreenInfo;

/// 画面をつなぐ区切り文字（U+001E）。
const RS: char = '\u{1E}';
/// 項目をつなぐ区切り文字（U+001F）。
const SEP: char = '\u{1F}';

/// 名前のエスケープ（`\\` -> `\`、`\r` -> U+001E、`\s` -> U+001F）を戻す。不正な並びは `None`。
fn unescape_name(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match it.next()? {
            '\\' => '\\',
            'r' => RS,
            's' => SEP,
            _ => return None,
        });
    }
    Some(out)
}

/// `MultiDisplayHelper` の文字列を画面一覧へ写す。空文字は空一覧（`validate` が失敗にする）。
/// 項目数・幅高さ・主画面フラグが不正なら `None`。倍率が読めなければ `scale: None`。
fn from_android(raw: &str) -> Option<Vec<ScreenInfo>> {
    if raw.is_empty() {
        return Some(Vec::new());
    }
    raw.split(RS)
        .map(|rec| {
            let parts: Vec<&str> = rec.split(SEP).collect();
            let [w, h, scale, primary, name] = parts[..] else {
                return None;
            };
            Some(ScreenInfo {
                width_px: w.parse().ok()?,
                height_px: h.parse().ok()?,
                scale: scale.parse().ok(),
                is_primary: match primary {
                    "1" => true,
                    "0" => false,
                    _ => return None,
                },
                name: Some(unescape_name(name)?),
            })
        })
        .collect()
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::device::android::JniCallFailed;
    use fandhe_device::multi_display::{MultiDisplay, ScreenInfo};
    use fandhe_device::DeviceError;

    /// Android の `MultiDisplay` 実装（`MainActivity.multiDisplayRead()` を JNI で呼ぶ）。
    pub(super) struct AndroidMultiDisplay;

    impl MultiDisplay for AndroidMultiDisplay {
        /// 2 箇所の `unsafe` は `AndroidPowerState::read` と同じ根拠・同じ形。
        #[allow(unsafe_code)]
        fn read(&self) -> Result<Vec<ScreenInfo>, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("multi_display read");
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
                        jni::jni_str!("multiDisplayRead"),
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
            from_android(&raw).ok_or(FAILED)
        }
    }
}

/// Android の `MultiDisplay` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::multi_display::MultiDisplay> {
    Box::new(jni_impl::AndroidMultiDisplay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::multi_display::validate;
    use fandhe_device::DeviceError;

    fn rec(p: [&str; 5]) -> String {
        p.join("\u{1F}")
    }

    #[test]
    fn maps_two_screens() {
        let raw = [
            rec(["1080", "2400", "2.75", "1", "Built-in"]),
            rec(["1920", "1080", "1.0", "0", "HDMI"]),
        ]
        .join("\u{1E}");
        let v = from_android(&raw).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].width_px, v[0].height_px), (1080, 2400));
        assert_eq!(v[0].scale, Some(2.75));
        assert!(v[0].is_primary && !v[1].is_primary);
        assert_eq!(v[1].name.as_deref(), Some("HDMI"));
    }

    #[test]
    fn malformed_fails() {
        assert!(from_android("a\u{1F}b").is_none());
        for bad in [
            ["-1", "1", "1", "1", "n"],
            ["x", "1", "1", "1", "n"],
            ["", "1", "1", "1", "n"],
            ["1", "1", "1", "2", "n"],
        ] {
            assert!(from_android(&rec(bad)).is_none());
        }
        let ok = rec(["1", "1", "1", "1", "n"]);
        assert!(from_android(&format!("{ok}\u{1E}bad")).is_none());
    }

    #[test]
    fn bad_scale_becomes_none_after_validate() {
        for s in ["abc", "NaN", "0", ""] {
            let v = validate(
                from_android(&rec(["1", "1", s, "1", "n"])).ok_or(DeviceError::Internal("x")),
            )
            .unwrap();
            assert_eq!(v[0].scale, None);
        }
    }

    #[test]
    fn empty_and_zero_size_are_internal() {
        assert!(validate(Ok(from_android("").unwrap())).is_err());
        let z = from_android(&rec(["0", "10", "1", "1", "n"])).unwrap();
        assert!(validate(Ok(z)).is_err());
    }

    #[test]
    fn name_with_separators_round_trips() {
        // Java 側のエスケープ結果: `A<US>B\\` / 区切り文字のみ `<RS><US>`
        let v = from_android(&rec(["1", "1", "1", "1", "A\\sB\\\\"])).unwrap();
        assert_eq!(v[0].name.as_deref(), Some("A\u{1F}B\\"));
        let v = from_android(&rec(["1", "1", "1", "1", "\\r\\s"])).unwrap();
        assert_eq!(v[0].name.as_deref(), Some("\u{1E}\u{1F}"));
        assert!(from_android(&rec(["1", "1", "1", "1", "a\\"])).is_none());
        assert!(from_android(&rec(["1", "1", "1", "1", "\\x"])).is_none());
    }

    #[test]
    fn name_is_not_trimmed_and_empty_becomes_none() {
        let v = validate(Ok(from_android(&rec(["1", "1", "1", "1", " a "])).unwrap())).unwrap();
        assert_eq!(v[0].name.as_deref(), Some(" a "));
        let v = validate(Ok(from_android(&rec(["1", "1", "1", "1", ""])).unwrap())).unwrap();
        assert_eq!(v[0].name, None);
    }
}
