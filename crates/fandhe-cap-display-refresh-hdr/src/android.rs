//! Android 向け「画面のリフレッシュレート・HDR・広色域」（親 #177、Issue #872）。
//! `driver.rs` の Android 経路から、egui スレッドが押下ごとに
//! 同期で 1 回呼ぶ。契約の SSOT は `crate::contract`。
//!
//! # JNI 窓口
//!
//! static の `DisplayRefreshHdrHelper.read(Activity)`（署名 `(Landroid/app/Activity;)[F`、Java・`jni_sig!`・本 doc の 3 か所で一致）。戻りは `float[4] {currentHz, maxHz, hdr, wideGamut}`
//!
//! - Hz は生値。0・負・非有限（番兵 -1 を含む）は `hz_from_raw` で `None`（丸めの前に検証）
//! - hdr・wideGamut は 1 = `Some(true)`、0 = `Some(false)`、それ以外（-1 等）は `None`
//!   （API 24・25 の広色域は取れず `None`）
//! - null・長さ不正・JNI 失敗・4 項目すべて `None` は `Internal`（嘘の成功を作らない）
//!
//! # ライフサイクル
//!
//! 同期の一発読み取りで、保留中の要求・id・コールバックを持たない。Activity 再生成で破棄すべき
//! ものは無い。古い世代の完了は画面側の `finish_read` が捨てる。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 失敗・全項目未取得を成功へ畳まない、例外を JNI に残さない。守らない: 読み取り後の
//! 画面モード変更（次の押下で読み直す）。
//!
//! 一次情報: <https://developer.android.com/reference/android/view/Display>

use crate::contract::{DisplayCapabilities, hz_from_raw, require_any};
use crate::error::DisplayRefreshHdrError;

/// 1.0 / 0.0 だけを真偽に写す。それ以外（-1・NaN・範囲外）は「取れない」。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn flag(v: f32) -> Option<bool> {
    if v == 1.0 {
        Some(true)
    } else if v == 0.0 {
        Some(false)
    } else {
        None
    }
}

/// Java の `float[4]`（失敗・null・長さ不正は `None`）を契約の値へ写す。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn from_android(raw: Option<[f32; 4]>) -> Result<DisplayCapabilities, DisplayRefreshHdrError> {
    let r = raw.ok_or(DisplayRefreshHdrError::Internal("display_refresh_hdr read"))?;
    require_any(DisplayCapabilities {
        current_hz: hz_from_raw(f64::from(r[0])),
        max_hz: hz_from_raw(f64::from(r[1])),
        hdr: flag(r[2]),
        wide_gamut: flag(r[3]),
    })
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::from_android;
    use crate::contract::{DisplayCapabilities, DisplayRefreshHdr};
    use crate::error::DisplayRefreshHdrError;
    use fandhe_platform_android::JniCallFailed;

    /// Android の `DisplayRefreshHdr` 実装（`DisplayRefreshHdrHelper.read` を JNI で呼ぶ）。
    pub(super) struct AndroidDisplayRefreshHdr;

    impl DisplayRefreshHdr for AndroidDisplayRefreshHdr {
        fn read(&self) -> Result<DisplayCapabilities, DisplayRefreshHdrError> {
            use fandhe_platform_android::{android_app, with_activity_class};
            use jni::objects::JValue;
            let Some(app) = android_app() else {
                return Err(DisplayRefreshHdrError::Internal("display_refresh_hdr read"));
            };
            // クラスは Activity の class loader で解決する（`with_activity_class`）。
            let result = with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.DisplayRefreshHdrHelper"),
                |env, activity, class| -> Result<Option<[f32; 4]>, JniCallFailed> {
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("read"),
                        jni::jni_sig!((android.app.Activity) -> float[]),
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
                    let arr = env.cast_local::<jni::objects::JFloatArray>(ret.l()?)?;
                    if arr.is_null() {
                        return Ok(None);
                    }
                    // 長さを検証してから読む。
                    if arr.len(env)? != 4 {
                        return Ok(None);
                    }
                    let mut v = [0f32; 4];
                    arr.get_region(env, 0, &mut v)?;
                    Ok(Some(v))
                },
            );

            from_android(result.ok().flatten())
        }
    }
}

/// Android の `DisplayRefreshHdr` 実装を返す。全 API 帯で Hz と HDR は読めるため `Unsupported` は返さない。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn crate::contract::DisplayRefreshHdr> {
    Box::new(jni_impl::AndroidDisplayRefreshHdr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_values_map() {
        let c = from_android(Some([59.94, 120.0, 1.0, 1.0])).unwrap();
        assert_eq!(c.current_hz, Some(60));
        assert_eq!(c.max_hz, Some(120));
        assert_eq!(c.hdr, Some(true));
        assert_eq!(c.wide_gamut, Some(true));
    }

    #[test]
    fn zero_flags_are_no_not_unknown() {
        let c = from_android(Some([60.0, 60.0, 0.0, 0.0])).unwrap();
        assert_eq!(c.hdr, Some(false));
        assert_eq!(c.wide_gamut, Some(false));
    }

    #[test]
    fn bad_hz_becomes_none() {
        for bad in [-1.0, 0.0, f32::NAN, f32::INFINITY] {
            let c = from_android(Some([bad, bad, 1.0, 1.0])).unwrap();
            assert_eq!((c.current_hz, c.max_hz), (None, None));
        }
    }

    #[test]
    fn api_24_25_wide_gamut_unknown() {
        let c = from_android(Some([60.0, 60.0, 0.0, -1.0])).unwrap();
        assert_eq!(c.wide_gamut, None);
        assert_eq!(c.hdr, Some(false));
    }

    #[test]
    fn all_sentinel_or_none_is_internal() {
        assert!(matches!(
            from_android(Some([-1.0; 4])),
            Err(DisplayRefreshHdrError::Internal("display_refresh_hdr read"))
        ));
        assert!(matches!(
            from_android(None),
            Err(DisplayRefreshHdrError::Internal("display_refresh_hdr read"))
        ));
    }

    #[test]
    fn out_of_range_flags_are_none() {
        let c = from_android(Some([60.0, 60.0, 2.0, f32::NAN])).unwrap();
        assert_eq!((c.hdr, c.wide_gamut), (None, None));
    }
}
