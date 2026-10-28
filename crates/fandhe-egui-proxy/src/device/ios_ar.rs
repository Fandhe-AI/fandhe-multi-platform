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
//! - 守る（映像）: 形式違い・nil・寸法不整合を画像に見せかけない（映像なし = `None`）
//! - 守らない: 姿勢・アンカー・hit-test、OS 間の値の一致、複数プロセス、映像の向き・なめらかさ・
//!   数値と同じフレーム由来であること・色の厳密さ
//!
//! # カメラ映像（Issue #1009）
//!
//! `read_frame` は driver の 100 ms pull に相乗りして `.m` の `fandhe_ios_ar_read_frame` を呼ぶ。
//! `.m` は main 限定で `currentFrame.capturedImage`（二面 YCbCr 4:2:0 full range）を lock し、生の面を
//! `fandhe_rust_ar_frame` へ同期に渡す。Rust 側が [`frame_from_planes`] で RGBA 変換と縮小を行う。
//! `ARFrame` は呼び出しの外で保持しない。OS コールバックが映像を共有状態へ入れる経路はなく、
//! 再描画は上記の変化通知と driver の `request_repaint_after` で足りる。
//!
//! # 既知の天井
//!
//! 実機での追跡・権限ダイアログ・背景復帰・中断の挙動は未確認（#306）。

#[cfg(any(target_os = "ios", test))]
use fandhe_device::ar::{checked_reading, ArFailure, ArSample, LimitedReason, Tracking};
#[cfg(any(target_os = "ios", test))]
use fandhe_device::camera_preview_stream::ScaledFrame;

/// `.m` が渡した二面（Y と交互配置の CbCr）を RGBA へ変換し長辺 320 以下へ縮める。
/// 不正な入力（寸法・stride・長さ）はすべて `None`（映像なし）。
// ponytail: main で毎回元画像（例 1920x1440）の全画素を変換する。負荷が出たら間引き読み
// （色差面だけ読む・行を飛ばす）へ移す。#1000 の実機確認で判断する。
#[cfg(any(target_os = "ios", test))]
fn frame_from_planes(
    width: u32,
    height: u32,
    y: &[u8],
    y_stride: usize,
    uv: &[u8],
    uv_stride: usize,
) -> Option<ScaledFrame> {
    let v = uv.get(1..)?;
    fandhe_device::camera_preview_stream::downscale_yuv420(
        width, height, y, y_stride, uv, v, uv_stride, 2,
    )
}

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

    use fandhe_device::camera_preview_stream::ScaledFrame;

    use super::{read_result, start_result};

    // SAFETY: `ios/fandhe_ios_ar.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_ar_start() -> i32;
        fn fandhe_ios_ar_read(tracking: *mut i32, pos: *mut f64, planes: *mut i32) -> i32;
        fn fandhe_ios_ar_stop();
        fn fandhe_ios_ar_read_frame(ctx: *mut std::ffi::c_void);
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
        fn read_frame(&self) -> Option<ScaledFrame> {
            let mut out: Option<ScaledFrame> = None;
            // SAFETY: `out` は呼び出しの間有効な排他参照。`.m` は同期に 0〜1 回 `fandhe_rust_ar_frame` へ
            // 渡すだけで保持しない。
            unsafe { fandhe_ios_ar_read_frame((&mut out as *mut Option<ScaledFrame>).cast()) };
            out
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

/// 映像の面を受ける（`.m` の `fandhe_ios_ar_read_frame` から同期に呼ばれる）。
/// `ctx` は `read_frame` の `Option<ScaledFrame>`。lock 中の面を変換して書き込むだけで保持しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code, clippy::too_many_arguments)]
#[no_mangle]
extern "C" fn fandhe_rust_ar_frame(
    ctx: *mut std::ffi::c_void,
    width: u32,
    height: u32,
    y: *const u8,
    y_len: usize,
    y_stride: usize,
    uv: *const u8,
    uv_len: usize,
    uv_stride: usize,
) {
    if ctx.is_null() || y.is_null() || uv.is_null() {
        return;
    }
    // SAFETY: `.m` は pixel buffer を lock している間だけ呼ぶ。`y` / `uv` は各面の base address で、
    // 長さは `BytesPerRow * Height`。`ctx` は `read_frame` が渡した排他参照。
    let (y, uv, out) = unsafe {
        (
            std::slice::from_raw_parts(y, y_len),
            std::slice::from_raw_parts(uv, uv_len),
            &mut *ctx.cast::<Option<ScaledFrame>>(),
        )
    };
    *out = frame_from_planes(width, height, y, y_stride, uv, uv_stride);
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
    fn frame_white_2x2() {
        let f = frame_from_planes(2, 2, &[255; 4], 2, &[128, 128], 2).unwrap();
        assert_eq!((f.width, f.height), (2, 2));
        assert_eq!(&f.rgba[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn frame_is_downscaled() {
        let (y, uv) = (vec![100; 640 * 480], vec![128; 640 * 240]);
        let f = frame_from_planes(640, 480, &y, 640, &uv, 640).unwrap();
        assert!(f.width.max(f.height) <= fandhe_device::camera_preview_stream::PREVIEW_MAX_SIDE);
    }

    #[test]
    fn frame_rejects_bad_input() {
        assert!(frame_from_planes(2, 2, &[0; 4], 2, &[], 2).is_none());
        assert!(frame_from_planes(2, 2, &[0; 3], 2, &[128, 128], 2).is_none());
        assert!(frame_from_planes(2, 2, &[0; 4], 1, &[128, 128], 2).is_none());
        assert!(frame_from_planes(0, 2, &[0; 4], 2, &[128, 128], 2).is_none());
        assert!(frame_from_planes(9000, 2, &[0; 4], 9000, &[128, 128], 2).is_none());
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
