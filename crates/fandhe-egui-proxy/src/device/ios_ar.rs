//! iOS 向け AR 実装（Issue #308、親 #90 の (b-ios)）。`ios/fandhe_ios_ar.m`（ARKit の `ARSession`、
//! `build.rs` が iOS ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 3 関数で
//! 呼ぶ。(a) のスタブを置き換えたもので、dispatch（`device/ar.rs`）・共通コア画面・trait 契約
//! （`fandhe_device::ar`）は変えない。
//!
//! `.m` は OS の生値と状態コードを返すだけで、範囲検査と契約値への写像は [`start_result`] /
//! [`read_result`] / [`tracking_from_code`] が行う（ホストでテストできるよう `cfg(any(ios, test))`）。
//!
//! # 再描画
//!
//! `.m` が状態の変化を確定した直後に `fandhe_rust_ar_changed` を呼び、`crate::ios::request_repaint` で
//! 次フレームを起こす。位置は毎フレーム変わるため要求せず、ドライバの 100 ms pull で拾う。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 非対応端末（シミュレータ含む）・権限拒否・NaN・範囲外・未知の追跡状態を成功値へ畳まない、
//!   停止後と背景遷移後の古い値を返さない
//! - 守らない: カメラ映像の描画、姿勢・アンカー・hit-test、OS 間の値の一致、複数プロセス
//!
//! # 既知の天井
//!
//! 実機での追跡・権限ダイアログ・背景復帰・中断の挙動は未確認（#306）。

#[cfg(any(target_os = "ios", test))]
use fandhe_device::ar::{checked_reading, ArFailure, ArSample, LimitedReason, Tracking};

/// `.m` の start の戻り値を契約へ写す（0 成功 / 1 非対応 / 2 権限拒否 / 他 失敗）。
#[cfg(any(target_os = "ios", test))]
fn start_result(code: i32) -> Result<(), ArFailure> {
    match code {
        0 => Ok(()),
        1 => Err(ArFailure::Unsupported),
        2 => Err(ArFailure::PermissionDenied),
        _ => Err(ArFailure::Failed),
    }
}

/// `.m` の追跡状態コードを契約へ写す。未知の値は None（成功値へ畳まない）。
#[cfg(any(target_os = "ios", test))]
fn tracking_from_code(code: i32) -> Option<Tracking> {
    Some(match code {
        0 => Tracking::NotAvailable,
        1 => Tracking::Normal,
        10 => Tracking::Limited(LimitedReason::Initializing),
        11 => Tracking::Limited(LimitedReason::ExcessiveMotion),
        12 => Tracking::Limited(LimitedReason::InsufficientFeatures),
        13 => Tracking::Limited(LimitedReason::Relocalizing),
        19 => Tracking::Limited(LimitedReason::Other),
        _ => return None,
    })
}

/// `.m` の read の戻り値を契約へ写す（0 値なし / 1 値あり / 2 権限拒否 / 4 許可待ち / 5 非対応 / 他 失敗）。
/// 未知の追跡状態・負の平面数・NaN・範囲外の位置は値なし（`.m` 側の守りと二重にする）。
#[cfg(any(target_os = "ios", test))]
fn read_result(
    code: i32,
    tracking: i32,
    pos: [f64; 3],
    planes: i32,
) -> Result<ArSample, ArFailure> {
    match code {
        0 => Ok(ArSample::Waiting),
        4 => Ok(ArSample::AwaitingPermission),
        2 => Err(ArFailure::PermissionDenied),
        5 => Err(ArFailure::Unsupported),
        1 => {
            let (Some(t), Ok(planes)) = (tracking_from_code(tracking), u32::try_from(planes))
            else {
                return Ok(ArSample::Waiting);
            };
            Ok(checked_reading(t, pos, Some(planes)).map_or(ArSample::Waiting, ArSample::Reading))
        }
        _ => Err(ArFailure::Failed),
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use fandhe_device::ar::{ArFailure, ArSample, ArTracking};

    use super::{read_result, start_result};

    // SAFETY: `ios/fandhe_ios_ar.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_ar_start() -> i32;
        fn fandhe_ios_ar_read(tracking: *mut i32, pos: *mut f64, planes: *mut i32) -> i32;
        fn fandhe_ios_ar_stop();
    }

    struct IosAr;

    impl ArTracking for IosAr {
        #[allow(unsafe_code)]
        fn start(&self) -> Result<(), ArFailure> {
            // SAFETY: 引数なし。`.m` は main 以外なら何もせず失敗コードを返す。
            start_result(unsafe { fandhe_ios_ar_start() })
        }

        #[allow(unsafe_code)]
        fn read(&self) -> Result<ArSample, ArFailure> {
            let (mut tracking, mut pos, mut planes) = (0_i32, [0.0_f64; 3], 0_i32);
            // SAFETY: 3 本のポインタは呼び出しの間有効な排他参照（`pos` は 3 要素）で、`.m` は保持しない。
            let code = unsafe { fandhe_ios_ar_read(&mut tracking, pos.as_mut_ptr(), &mut planes) };
            read_result(code, tracking, pos, planes)
        }

        #[allow(unsafe_code)]
        fn stop(&self) {
            // SAFETY: 引数なし。`.m` は main 以外なら main queue へ非同期に送る。
            unsafe { fandhe_ios_ar_stop() }
        }
    }

    /// iOS の `ArTracking` 実装を返す（`device/ar.rs` の iOS 経路から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn ArTracking> {
        Arc::new(IosAr)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// 状態変化の通知（`.m` から）。引数なし・panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ar_changed() {
    crate::ios::request_repaint();
}

/// 背景遷移の通知。背景中は `NoReading` の待機を止める。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ar_backgrounded() {
    super::ar::note_backgrounded();
}

/// 前景復帰の通知。ドライバの待機起点を引き直させる。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_ar_resumed() {
    super::ar::note_resumed();
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_never_fold_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(ArFailure::Unsupported));
        assert_eq!(start_result(2), Err(ArFailure::PermissionDenied));
        assert_eq!(start_result(3), Err(ArFailure::Failed));
        assert_eq!(start_result(-1), Err(ArFailure::Failed));
    }

    #[test]
    fn read_status_codes() {
        let p = [0.0; 3];
        assert_eq!(read_result(0, 1, p, 0), Ok(ArSample::Waiting));
        assert_eq!(read_result(4, 1, p, 0), Ok(ArSample::AwaitingPermission));
        assert_eq!(read_result(2, 1, p, 0), Err(ArFailure::PermissionDenied));
        assert_eq!(read_result(5, 1, p, 0), Err(ArFailure::Unsupported));
        assert_eq!(read_result(3, 1, p, 0), Err(ArFailure::Failed));
        assert_eq!(read_result(-1, 1, p, 0), Err(ArFailure::Failed));
    }

    #[test]
    fn valid_reading_and_invalid_samples() {
        let ok = read_result(1, 1, [1.0, 2.0, -3.0], 2).unwrap();
        let ArSample::Reading(r) = ok else { panic!() };
        assert_eq!(r.tracking, Tracking::Normal);
        assert_eq!(r.position, Some([1.0, 2.0, -3.0]));
        assert_eq!(r.horizontal_planes, Some(2));
        for bad in [
            [f64::NAN, 0.0, 0.0],
            [f64::INFINITY, 0.0, 0.0],
            [1000.1, 0.0, 0.0],
        ] {
            assert_eq!(read_result(1, 1, bad, 0), Ok(ArSample::Waiting));
        }
        assert_eq!(read_result(1, 99, [0.0; 3], 0), Ok(ArSample::Waiting));
        assert_eq!(read_result(1, 1, [0.0; 3], -1), Ok(ArSample::Waiting));
    }

    #[test]
    fn not_available_drops_position() {
        let ArSample::Reading(r) = read_result(1, 0, [1.0, 1.0, 1.0], 0).unwrap() else {
            panic!()
        };
        assert_eq!(r.position, None);
    }

    #[test]
    fn tracking_codes() {
        use LimitedReason::*;
        for (c, t) in [
            (10, Initializing),
            (11, ExcessiveMotion),
            (12, InsufficientFeatures),
            (13, Relocalizing),
            (19, Other),
        ] {
            assert_eq!(tracking_from_code(c), Some(Tracking::Limited(t)));
        }
        assert_eq!(tracking_from_code(7), None);
    }
}
