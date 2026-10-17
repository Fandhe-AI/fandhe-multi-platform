//! Android 向けペン・筆圧入力実装（Issue #357、親 #95、契約は `fandhe_device::pen_stylus`）。
//! `device/pen_stylus.rs` の dispatch から呼ばれ、effect ドライバが押下中に `latest()` を pull する。
//!
//! winit は tool type と `AXIS_TILT` を捨てるため、Java 側 `PenStylusHelper`（SSOT）が
//! `MainActivity.dispatchTouchEvent` で MotionEvent を観測し（イベントは消費しない）、
//! 主ポインタ 1 本の種類・筆圧・傾きを JNI でここの [`SharedPen`] へ push する。位置は egui から取る。
//!
//! - 筆圧・傾きは `Pen`（スタイラス・消しゴム）のときだけ出す。指・マウスは `None`（固定値や接触面積を筆圧にしない）
//! - 傾きを報告しない機器の 0 を垂直と取り違えないよう、軸の有無（`hasTilt`）を別に受ける
//! - 離す・キャンセル・`onPause`・`onDestroy` で `clear` し、最後の値を出し続けない
//! - 再描画は要求しない。同じタッチは winit 経由で egui にも届き、押下中はドライバが次の描画を予約している
//!
//! 守る範囲と守らない範囲は `fandhe_device::pen_stylus` の doc を参照。
// ponytail: 主ポインタ 1 本・履歴サンプルを読まない。速い筆致で点が粗くなるなら `getHistorical*` を足す。

use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::pen_stylus::SharedPen;
#[cfg(not(target_os = "android"))]
use fandhe_device::pen_stylus::UnsupportedPenStylus;
use fandhe_device::pen_stylus::{
    altitude_from_android_tilt_rad, PenSample, PenStylus, PointerKind,
};

/// `MotionEvent.TOOL_TYPE_*` の値（Android 公式定数）。
const TOOL_TYPE_FINGER: i32 = 1;
const TOOL_TYPE_STYLUS: i32 = 2;
const TOOL_TYPE_MOUSE: i32 = 3;
const TOOL_TYPE_ERASER: i32 = 4;

/// Java から届いた生値を [`PenSample`] にする。筆圧・傾きはペンのときだけ。
/// 検証・丸めは変換の前に済ませる（`altitude_from_android_tilt_rad` / `PenSample::new`）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn sample_from_raw(tool_type: i32, pressure: f32, tilt_rad: f32, has_tilt: bool) -> PenSample {
    let kind = match tool_type {
        TOOL_TYPE_STYLUS | TOOL_TYPE_ERASER => PointerKind::Pen,
        TOOL_TYPE_FINGER => PointerKind::Touch,
        TOOL_TYPE_MOUSE => PointerKind::Mouse,
        _ => PointerKind::Unknown,
    };
    if kind != PointerKind::Pen {
        return PenSample::new(kind, None, None);
    }
    let altitude = if has_tilt {
        altitude_from_android_tilt_rad(tilt_rad)
    } else {
        None
    };
    PenSample::new(kind, Some(pressure), altitude)
}

#[cfg(target_os = "android")]
fn pen() -> &'static SharedPen {
    static PEN: std::sync::OnceLock<SharedPen> = std::sync::OnceLock::new();
    PEN.get_or_init(SharedPen::new)
}

/// この OS の実装。Android 以外（ホストのテスト）は非対応を返す。
pub(crate) fn implementation() -> Arc<dyn PenStylus> {
    #[cfg(target_os = "android")]
    {
        Arc::new(pen().clone())
    }
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(UnsupportedPenStylus)
    }
}

/// `PenStylusHelper.nativeOnPenSample(int, float, float, boolean)` の実体。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// `device/android_fullscreen_immersive.rs` の同種エクスポートと同じ根拠。
#[cfg(all(target_os = "android", feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PenStylusHelper_nativeOnPenSample(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    tool_type: i32,
    pressure: f32,
    tilt_rad: f32,
    has_tilt: u8,
) {
    pen().set(sample_from_raw(
        tool_type,
        pressure,
        tilt_rad,
        has_tilt != 0,
    ));
}

/// `PenStylusHelper.nativeOnPenClear()` の実体。接触なしに戻す。
///
/// # Safety
/// `nativeOnPenSample` と同じ（unsafe 操作なし、固定シンボル名）。
#[cfg(all(target_os = "android", feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PenStylusHelper_nativeOnPenClear(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
    pen().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylus_and_eraser_are_pen_with_pressure_and_tilt() {
        for t in [TOOL_TYPE_STYLUS, TOOL_TYPE_ERASER] {
            let s = sample_from_raw(t, 0.5, 0.0, true);
            assert_eq!(s.kind, PointerKind::Pen);
            assert_eq!(s.pressure, Some(0.5));
            assert_eq!(s.altitude_deg, Some(90.0));
        }
    }

    #[test]
    fn non_pen_has_no_pressure_or_tilt() {
        for (t, k) in [
            (TOOL_TYPE_FINGER, PointerKind::Touch),
            (TOOL_TYPE_MOUSE, PointerKind::Mouse),
            (0, PointerKind::Unknown),
            (99, PointerKind::Unknown),
        ] {
            let s = sample_from_raw(t, 0.8, 0.3, true);
            assert_eq!(s.kind, k);
            assert_eq!(s.pressure, None);
            assert_eq!(s.altitude_deg, None);
        }
    }

    #[test]
    fn no_tilt_axis_is_not_vertical() {
        let s = sample_from_raw(TOOL_TYPE_STYLUS, 0.5, 0.0, false);
        assert_eq!(s.altitude_deg, None);
    }

    #[test]
    fn invalid_tilt_and_pressure_are_sanitized() {
        let p = |pr: f32, tilt: f32| sample_from_raw(TOOL_TYPE_STYLUS, pr, tilt, true);
        assert_eq!(p(0.5, 2.0).altitude_deg, None);
        assert_eq!(p(0.5, f32::NAN).altitude_deg, None);
        assert_eq!(p(1.7, 0.0).pressure, Some(1.0));
        assert_eq!(p(f32::NAN, 0.0).pressure, None);
    }
}
