//! iOS 向け方位実装（Issue #1405、親 #1154）。`ios/fandhe_ios_compass.m`（CoreLocation の
//! heading 購読、`build.rs` が iOS ターゲットの feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 3 関数で呼ぶ。#1404 のスタブを置き換えたもので、dispatch
//! （`device/compass.rs`）・共通コア画面・trait 契約（`fandhe_device::compass`）は変えない。
//!
//! `.m` は OS の生値と状態コードを返すだけで、範囲検査と契約値への写像は [`start_result`] /
//! [`read_result`] / [`accuracy_from_degrees`] が行う（ホストでテストできるよう `cfg(any(ios, test))`）。
//!
//! # 再描画
//!
//! delegate が値・失敗を確定した直後に `.m` が `fandhe_rust_compass_changed` を呼び、
//! `crate::ios::request_repaint` で次フレームを起こす（入力が無くても画面が止まらない。
//! ドライバの 100 ms 予約とは別経路）。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 非対応端末（シミュレータ含む）・無効サンプル・範囲外・NaN を 0° などの成功値へ畳まない、
//!   停止後の古い通知と背景遷移後の古い値を返さない
//! - 守らない: OS 間の値の一致、真北・偏角・画面回転の補正、較正 UI、複数プロセス
//!
//! # 既知の天井
//!
//! - 位置情報の許可は要求しない。許可なしで heading が届くかは実機未確認で、届かなければドライバの
//!   3 秒期限で `NoReading` になる（嘘の値は出ない）。確認は #1403
//! - シミュレータは `headingAvailable` が NO で常に `Unsupported`

use fandhe_device::compass::{normalize_heading, Accuracy, CompassFailure, Heading};

/// `.m` の start の戻り値を契約へ写す（0 成功 / 1 非対応 / 2 権限拒否 / 他 失敗）。
fn start_result(code: i32) -> Result<(), CompassFailure> {
    match code {
        0 => Ok(()),
        1 => Err(CompassFailure::Unsupported),
        2 => Err(CompassFailure::PermissionDenied),
        _ => Err(CompassFailure::Failed),
    }
}

/// `.m` の read の戻り値を契約へ写す（0 値なし / 1 値あり / 2 権限拒否 / 他 失敗）。
/// 範囲外・NaN・負の精度は値なし（`.m` 側の守りと二重にする）。
fn read_result(code: i32, degrees: f64, accuracy: f64) -> Result<Option<Heading>, CompassFailure> {
    match code {
        0 => Ok(None),
        1 => {
            if accuracy < 0.0 {
                return Ok(None);
            }
            Ok(normalize_heading(degrees).map(|degrees| Heading {
                degrees,
                accuracy: accuracy_from_degrees(accuracy),
            }))
        }
        2 => Err(CompassFailure::PermissionDenied),
        _ => Err(CompassFailure::Failed),
    }
}

/// `CLHeading.headingAccuracy`（磁北との誤差の度）を表示用の精度へ写す。
// ponytail: 10° / 30° は仮置き。実機で見て調整する。OS 間の値の一致は守らない範囲。
fn accuracy_from_degrees(a: f64) -> Accuracy {
    if !a.is_finite() {
        Accuracy::Unknown
    } else if a <= 10.0 {
        Accuracy::High
    } else if a <= 30.0 {
        Accuracy::Approximate
    } else {
        Accuracy::Unreliable
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use fandhe_device::compass::{Compass, CompassFailure, Heading};

    use super::{read_result, start_result};

    // SAFETY: `ios/fandhe_ios_compass.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_compass_start() -> i32;
        fn fandhe_ios_compass_read(degrees: *mut f64, accuracy: *mut f64) -> i32;
        fn fandhe_ios_compass_stop();
    }

    struct IosCompass;

    impl Compass for IosCompass {
        #[allow(unsafe_code)]
        fn start(&self) -> Result<(), CompassFailure> {
            // SAFETY: 引数なし。`.m` は main 以外なら何もせず失敗コードを返す。
            start_result(unsafe { fandhe_ios_compass_start() })
        }

        #[allow(unsafe_code)]
        fn read(&self) -> Result<Option<Heading>, CompassFailure> {
            let (mut degrees, mut accuracy) = (0.0_f64, 0.0_f64);
            // SAFETY: 2 本のポインタは呼び出しの間有効な排他参照で、`.m` は保持しない。
            let code = unsafe { fandhe_ios_compass_read(&mut degrees, &mut accuracy) };
            read_result(code, degrees, accuracy)
        }

        #[allow(unsafe_code)]
        fn stop(&self) {
            // SAFETY: 引数なし。`.m` は main 以外なら main queue へ非同期に送る。
            unsafe { fandhe_ios_compass_stop() }
        }
    }

    /// iOS の `Compass` 実装を返す（`device/compass.rs` の iOS 経路から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn Compass> {
        Arc::new(IosCompass)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// `.m` の delegate が値・失敗を確定した直後に呼ぶ受け口。引数なし・ポインタなしで、`dispatch_sync` は
/// しない（契約表 11）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_compass_changed() {
    crate::ios::request_repaint();
}

/// 背景遷移の通知（`.m` の `DidEnterBackground` から）。背景中は `NoReading` の待機を止める。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_compass_backgrounded() {
    super::compass::note_backgrounded();
}

/// 前景復帰の通知（`.m` の `WillEnterForeground` から）。ドライバの待機起点を引き直させる。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_compass_resumed() {
    super::compass::note_resumed();
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_never_fold_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(CompassFailure::Unsupported));
        assert_eq!(start_result(2), Err(CompassFailure::PermissionDenied));
        assert_eq!(start_result(3), Err(CompassFailure::Failed));
        assert_eq!(start_result(-1), Err(CompassFailure::Failed));
    }

    #[test]
    fn read_codes() {
        assert_eq!(read_result(0, 90.0, 5.0), Ok(None));
        assert_eq!(
            read_result(2, 0.0, 0.0),
            Err(CompassFailure::PermissionDenied)
        );
        assert_eq!(read_result(3, 0.0, 0.0), Err(CompassFailure::Failed));
        let h = read_result(1, 90.0, 5.0).unwrap().unwrap();
        assert_eq!((h.degrees, h.accuracy), (90.0, Accuracy::High));
    }

    #[test]
    fn invalid_samples_are_none() {
        for (d, a) in [
            (f64::NAN, 5.0),
            (-1.0, 5.0),
            (361.0, 5.0),
            (f64::INFINITY, 5.0),
            (90.0, -1.0),
        ] {
            assert_eq!(read_result(1, d, a), Ok(None), "{d} {a}");
        }
    }

    #[test]
    fn full_circle_wraps_to_zero() {
        assert_eq!(read_result(1, 360.0, 5.0).unwrap().unwrap().degrees, 0.0);
    }

    #[test]
    fn accuracy_thresholds() {
        assert_eq!(accuracy_from_degrees(10.0), Accuracy::High);
        assert_eq!(accuracy_from_degrees(10.01), Accuracy::Approximate);
        assert_eq!(accuracy_from_degrees(30.0), Accuracy::Approximate);
        assert_eq!(accuracy_from_degrees(30.01), Accuracy::Unreliable);
        assert_eq!(accuracy_from_degrees(f64::NAN), Accuracy::Unknown);
    }
}
