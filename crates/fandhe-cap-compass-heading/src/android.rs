//! Android 向け方位実装（Issue #1406、親 #1154）。本 crate の `driver` の Android 経路から
//! egui スレッドで呼ばれ、`CompassHelper` の static `start` / `read` / `stop`（第 1 引数は Activity）を
//! 同期 JNI で呼んで最新値を pull する（要求 id・`RequestSlot` は使わない）。Java 側は
//! `CompassHelper`（`TYPE_ROTATION_VECTOR`、無ければ加速度 + 磁気を `getRotationMatrix` で合成）。
//!
//! # 結果コード（SSOT は Java `CompassHelper`）
//!
//! - `compassStart`: 0 = OK / 1 = センサー無し（`Unsupported`）/ 2 = 登録失敗・破棄済み（`Failed`）
//! - `compassRead`: 負値 = 購読が失われた（Activity 再生成など。`Failed`）/ 0 = 値なし
//!   （未受信・1 秒超の古い値・一時停止中）/ bit32 = 1 で値あり（bit0-31 = 方位角ラジアンの
//!   float 生ビット、bit40-47 = `accuracy & 0xff`）。未定義ビットが立った値は `Failed`
//!
//! 権限は不要のため `PermissionDenied` は返さない。ラジアンを生のまま渡すのは、範囲検証を
//! 度への変換より前に Rust で行うため。
//!
//! # ライフサイクル
//!
//! `onPause` で解除（購読の意思は残し、read は値なし）、`onResume` で再登録。`onDestroy`
//! （再生成を含む）で購読を破棄し、新しい Activity の read は負値を返すので Rust 側は
//! `Failed` になり、driver が `stop` して失敗表示にする。リスナーは購読ごとに世代を分け、
//! 旧世代の値は捨てる。
//!
//! 守る範囲: 嘘の値を出さない・二重に登録しない・停止 / 背景遷移 / 破棄で解除する。
//! 守らない範囲: 偏角（真北）、画面回転の補正、磁気の乱れ、背景から戻った直後の値なし、
//! 作り直し後の失敗表示、1 秒の古さ閾値による遅れ、fallback 構成の精度。
//! 方位・精度はログに出さない。実機確認は #1403。

use std::sync::Arc;

use crate::contract::{Accuracy, Compass, CompassFailure, Heading, normalize_heading};

const START_OK: i32 = 0;
const START_UNSUPPORTED: i32 = 1;

/// `compassStart` の結果コードを契約の結果へ写像する。未知のコードは `Failed`。
fn decode_start(code: i32) -> Result<(), CompassFailure> {
    match code {
        START_OK => Ok(()),
        START_UNSUPPORTED => Err(CompassFailure::Unsupported),
        _ => Err(CompassFailure::Failed),
    }
}

/// 方位角（ラジアン、`[-π, π]`）を `0..360` の度へ。有限かつ範囲内かを変換より前に検査し、
/// 外れたら `None`（無効サンプル）。
fn heading_from_azimuth(rad: f32) -> Option<f64> {
    use std::f32::consts::PI;
    if !rad.is_finite() || !(-PI..=PI).contains(&rad) {
        return None;
    }
    let deg = f64::from(rad).to_degrees();
    normalize_heading(if deg < 0.0 { deg + 360.0 } else { deg })
}

/// Android の `SensorEvent.accuracy` の写像。範囲外（`NO_CONTACT` = -1 → 255 を含む）は `Unknown`。
fn accuracy_from_raw(raw: i32) -> Accuracy {
    match raw {
        0 | 1 => Accuracy::Unreliable,
        2 => Accuracy::Approximate,
        3 => Accuracy::High,
        _ => Accuracy::Unknown,
    }
}

const VALUE_FLAG: i64 = 1 << 32;
const ACCURACY_SHIFT: u32 = 40;
/// 定義済みビット（bit0-31・bit32・bit40-47）以外。
const UNDEFINED_BITS: i64 = !(0xffff_ffff | VALUE_FLAG | (0xff << ACCURACY_SHIFT));

/// `compassRead` の戻りを契約の結果へ写像する。負値・未定義ビットは `Failed`、0 は値なし。
fn decode_read(raw: i64) -> Result<Option<Heading>, CompassFailure> {
    if raw < 0 || raw & UNDEFINED_BITS != 0 {
        return Err(CompassFailure::Failed);
    }
    if raw & VALUE_FLAG == 0 {
        return Ok(None);
    }
    let rad = f32::from_bits((raw & 0xffff_ffff) as u32);
    let accuracy = accuracy_from_raw(((raw >> ACCURACY_SHIFT) & 0xff) as i32);
    Ok(heading_from_azimuth(rad).map(|degrees| Heading { degrees, accuracy }))
}

#[cfg(target_os = "android")]
struct AndroidCompass;

/// `CompassHelper` の static メソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`。`unsafe` は共通層
/// `fandhe_platform_android::with_activity_class` に閉じている。
#[cfg(target_os = "android")]
fn call_helper<T>(
    f: impl FnOnce(
        &mut jni::Env,
        &jni::objects::JObject,
        &jni::objects::JClass,
    ) -> Result<T, fandhe_platform_android::JniCallFailed>,
) -> Option<T> {
    let app = fandhe_platform_android::android_app()?;
    fandhe_platform_android::with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.CompassHelper"),
        f,
    )
    .ok()
}

#[cfg(target_os = "android")]
impl Compass for AndroidCompass {
    fn start(&self) -> Result<(), CompassFailure> {
        let code = call_helper(|env, a, class| {
            Ok(env
                .call_static_method(
                    class,
                    jni::jni_str!("start"),
                    jni::jni_sig!((android.app.Activity) -> int),
                    &[jni::objects::JValue::Object(a)],
                )?
                .i()?)
        })
        .ok_or(CompassFailure::Failed)?;
        decode_start(code)
    }

    fn read(&self) -> Result<Option<Heading>, CompassFailure> {
        let raw = call_helper(|env, a, class| {
            Ok(env
                .call_static_method(
                    class,
                    jni::jni_str!("read"),
                    jni::jni_sig!((android.app.Activity) -> long),
                    &[jni::objects::JValue::Object(a)],
                )?
                .j()?)
        })
        .ok_or(CompassFailure::Failed)?;
        decode_read(raw)
    }

    fn stop(&self) {
        let _ = call_helper(|env, a, class| {
            env.call_static_method(
                class,
                jni::jni_str!("stop"),
                jni::jni_sig!((android.app.Activity) -> ()),
                &[jni::objects::JValue::Object(a)],
            )?;
            Ok(())
        });
    }
}

/// Android の `Compass` 実装を返す（本 crate の `driver` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn Compass> {
    Arc::new(AndroidCompass)
}

/// ホストのテストビルドにも載るための形。dispatch は Android でしか呼ばない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn Compass> {
    Arc::new(crate::contract::UnsupportedCompass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn pack(rad: f32, acc: i64) -> i64 {
        i64::from(rad.to_bits()) | VALUE_FLAG | (acc << ACCURACY_SHIFT)
    }

    #[test]
    fn start_codes() {
        assert_eq!(decode_start(0), Ok(()));
        assert_eq!(decode_start(1), Err(CompassFailure::Unsupported));
        assert_eq!(decode_start(2), Err(CompassFailure::Failed));
        assert_eq!(decode_start(9), Err(CompassFailure::Failed));
    }

    #[test]
    fn azimuth_to_degrees() {
        assert_eq!(heading_from_azimuth(0.0), Some(0.0));
        assert!((heading_from_azimuth(PI / 2.0).unwrap() - 90.0).abs() < 1e-4);
        assert!((heading_from_azimuth(-PI / 2.0).unwrap() - 270.0).abs() < 1e-4);
        assert!((heading_from_azimuth(PI).unwrap() - 180.0).abs() < 1e-4);
        assert!((heading_from_azimuth(-PI).unwrap() - 180.0).abs() < 1e-4);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, PI + 0.01, -4.0] {
            assert_eq!(heading_from_azimuth(bad), None);
        }
        assert!(heading_from_azimuth(-1e-9).is_none_or(|d| d < 360.0));
    }

    #[test]
    fn accuracy_map() {
        assert_eq!(accuracy_from_raw(0), Accuracy::Unreliable);
        assert_eq!(accuracy_from_raw(1), Accuracy::Unreliable);
        assert_eq!(accuracy_from_raw(2), Accuracy::Approximate);
        assert_eq!(accuracy_from_raw(3), Accuracy::High);
        assert_eq!(accuracy_from_raw(255), Accuracy::Unknown);
        assert_eq!(accuracy_from_raw(-1), Accuracy::Unknown);
    }

    #[test]
    fn read_decoding_never_fakes_success() {
        assert_eq!(decode_read(-1), Err(CompassFailure::Failed));
        assert_eq!(decode_read(i64::MIN), Err(CompassFailure::Failed));
        assert_eq!(decode_read(0), Ok(None));
        let h = decode_read(pack(PI / 2.0, 3)).unwrap().unwrap();
        assert!((h.degrees - 90.0).abs() < 1e-4);
        assert_eq!(h.accuracy, Accuracy::High);
        assert_eq!(decode_read(pack(f32::NAN, 3)), Ok(None));
        assert_eq!(decode_read(pack(4.0, 3)), Ok(None));
        assert_eq!(
            decode_read(pack(0.0, 3) | (1 << 33)),
            Err(CompassFailure::Failed)
        );
        assert_eq!(
            decode_read(pack(0.0, 3) | (1 << 48)),
            Err(CompassFailure::Failed)
        );
        // フラグ無しで方位ビットだけ立った値は値なし。
        assert_eq!(decode_read(i64::from(1.0f32.to_bits())), Ok(None));
    }
}
