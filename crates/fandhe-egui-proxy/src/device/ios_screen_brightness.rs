//! iOS 向け画面輝度実装（Issue #634、親 #126）。`UIScreen.brightness`
//! （`ios/fandhe_ios_screen_brightness.m`、`build.rs` が iOS 分岐の feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 3 関数で書く / 読む。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/screen_brightness.rs` の iOS 経路が `implementation()`
//! を呼ぶ（dispatch・共通コア画面・trait 契約は #633 で確定済みで触らない）。
//!
//! # 状態とスレッド
//!
//! 元の値・要求値・observer は `.m` 側が main スレッドだけで扱う（egui の update = main から
//! 同期で呼ばれる）。Rust 側は状態を持たない。main 以外からの呼び出しは `.m` が UIKit に触れず
//! 戻りコード 2 を返し、ここで `Internal` へ写す。
//!
//! # `overriding` の規約
//!
//! 背景へ回って利用者の値へ戻している間も、`read()` は `overriding=true` と要求値を返す。
//! `overriding=false` にするのは、利用者による変更（[`NEAR_EQUAL_TOLERANCE`] を超える差）を
//! 検知して上書きをやめたときだけ。共通コア画面は `false` を「利用者が変えた」と解釈して要求を
//! 捨てるため、背景遷移で偽にすると前面復帰後に掛け直されない。
//!
//! # 再描画（契約表 4）
//!
//! observer（背景遷移・前面復帰・利用者変更の検知）が状態を更新した直後に、`.m` が
//! [`fandhe_rust_screen_brightness_changed`] を呼び、`crate::ios::request_repaint` で次フレームを
//! 起こす（入力が無いと egui が描画せず表示が古いまま止まるのを防ぐ）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! trait doc（`fandhe_device::screen_brightness`）を引き継ぐ。守る: 解除・背景遷移で利用者の値へ
//! 戻ること、範囲外の値を OS へ渡さないこと、失敗を成功と見せないこと。守らない: プロセス強制終了時
//! の復元（画面ロックまで残る。OS が片付けるため同期させない）、複数ウィンドウ・外部ディスプレイ、
//! 自動輝度による OS 側の変化（利用者の変更と区別できず、上書きをやめる場合がある）。
//! [`NEAR_EQUAL_TOLERANCE`] は仮の値で、実機確認（#628）で確定する。

#[cfg(target_os = "ios")]
use std::sync::Arc;

use fandhe_device::screen_brightness::{validate_level, BrightnessReading, LEVEL_MAX, LEVEL_MIN};
#[cfg(target_os = "ios")]
use fandhe_device::screen_brightness::{ScreenBrightness, NEAR_EQUAL_TOLERANCE};
use fandhe_device::DeviceError;

// `ios/fandhe_ios_screen_brightness.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_screen_brightness_set(level: f64, tolerance: f64) -> i32;
    fn fandhe_ios_screen_brightness_clear() -> i32;
    fn fandhe_ios_screen_brightness_read(out_level: *mut f64, out_overriding: *mut i32) -> i32;
}

/// 戻りコード（0 = 成功、4 = 引数不正、2・3・未知値 = 失敗）を契約のエラー種別へ写す。
fn decode_status(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        4 => Err(DeviceError::InvalidInput("screen-brightness level")),
        _ => Err(DeviceError::Internal("screen-brightness")),
    }
}

/// OS から読んだ輝度を f32 へ丸める。範囲検査は丸めの前に f64 のまま行い（契約表 5）、
/// OS 側の異常値なので失敗は `InvalidInput` ではなく `Internal` に写す。
fn level_from_os(raw: f64) -> Result<f32, DeviceError> {
    if !raw.is_finite() || raw < f64::from(LEVEL_MIN) || raw > f64::from(LEVEL_MAX) {
        return Err(DeviceError::Internal("screen-brightness"));
    }
    validate_level(raw as f32).map_err(|_| DeviceError::Internal("screen-brightness"))
}

/// `read` の戻りコードと out 値を読み戻しへ写す。
fn reading_from_os(
    code: i32,
    raw_level: f64,
    raw_overriding: i32,
) -> Result<BrightnessReading, DeviceError> {
    decode_status(code)?;
    let overriding = match raw_overriding {
        0 => false,
        1 => true,
        _ => return Err(DeviceError::Internal("screen-brightness")),
    };
    Ok(BrightnessReading {
        level: Some(level_from_os(raw_level)?),
        overriding,
    })
}

/// iOS の `ScreenBrightness` 実装（`UIScreen.brightness`）。
#[cfg(target_os = "ios")]
struct IosScreenBrightness;

#[cfg(target_os = "ios")]
impl ScreenBrightness for IosScreenBrightness {
    #[allow(unsafe_code)]
    fn set_override(&self, level: f32) -> Result<(), DeviceError> {
        let level = validate_level(level)?;
        // SAFETY: 引数は値渡しの f64 のみでポインタを経由しない。main スレッドの検査は `.m` 側で
        // 行い、main 以外では UIKit に触れず 2 を返す。
        let code = unsafe {
            fandhe_ios_screen_brightness_set(f64::from(level), f64::from(NEAR_EQUAL_TOLERANCE))
        };
        decode_status(code)
    }

    #[allow(unsafe_code)]
    fn clear_override(&self) -> Result<(), DeviceError> {
        // SAFETY: 引数なし・戻りは値渡しの i32。main 検査は `.m` 側で行う。
        decode_status(unsafe { fandhe_ios_screen_brightness_clear() })
    }

    #[allow(unsafe_code)]
    fn read(&self) -> Result<BrightnessReading, DeviceError> {
        let mut level: f64 = 0.0;
        let mut overriding: i32 = 0;
        // SAFETY: 2 つのポインタはこのスタック上の有効な変数を指し、呼び出しの間だけ使われる。
        // `.m` 側は成功時のみ書き込む。main 検査は `.m` 側で行う。
        let code = unsafe { fandhe_ios_screen_brightness_read(&mut level, &mut overriding) };
        reading_from_os(code, level, overriding)
    }
}

/// `device/screen_brightness.rs` の iOS 経路から呼ばれる。プロセスで 1 個の iOS 実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn ScreenBrightness> {
    Arc::new(IosScreenBrightness)
}

/// `.m` の observer が状態を更新した直後に main スレッドから呼ぶ受け口。次フレームを起こす。
/// 引数を取らず panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_screen_brightness_changed() {
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_internal(e: &DeviceError) -> bool {
        matches!(e, DeviceError::Internal("screen-brightness"))
    }

    #[test]
    fn decode_status_maps_codes() {
        assert!(decode_status(0).is_ok());
        assert!(matches!(
            decode_status(4),
            Err(DeviceError::InvalidInput("screen-brightness level"))
        ));
        for code in [2, 3, -1, 99] {
            assert!(is_internal(&decode_status(code).unwrap_err()), "{code}");
        }
    }

    #[test]
    fn level_from_os_rejects_out_of_range_before_rounding() {
        for raw in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.01,
            1.01,
            1.000_000_000_1,
        ] {
            assert!(is_internal(&level_from_os(raw).unwrap_err()), "{raw}");
        }
        assert_eq!(level_from_os(0.0).unwrap(), 0.0);
        assert_eq!(level_from_os(0.5).unwrap(), 0.5);
        assert_eq!(level_from_os(1.0).unwrap(), 1.0);
    }

    #[test]
    fn reading_from_os_maps_status_and_overriding() {
        assert!(reading_from_os(3, 0.5, 1).is_err());
        assert!(is_internal(&reading_from_os(0, 0.5, 2).unwrap_err()));
        assert_eq!(
            reading_from_os(0, 0.5, 1).unwrap(),
            BrightnessReading {
                level: Some(0.5),
                overriding: true
            }
        );
        assert_eq!(
            reading_from_os(0, 0.25, 0).unwrap(),
            BrightnessReading {
                level: Some(0.25),
                overriding: false
            }
        );
    }
}
