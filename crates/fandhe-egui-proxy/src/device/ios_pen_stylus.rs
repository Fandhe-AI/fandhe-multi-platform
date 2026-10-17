//! iOS 向けペン・筆圧入力実装（Issue #356、親 #95）。`ios/fandhe_ios_pen_stylus.m`
//! （winit の base view に付ける観測専用の `UIGestureRecognizer`、`build.rs` が iOS ターゲットの
//! feature `device` 有効時にコンパイルする）が `UITouch` の種類・筆圧・傾きを読み、
//! `fandhe_rust_pen_stylus_set` / `_clear` でこのモジュールの [`SharedPen`] へ置く（push 型）。
//! `device/pen_stylus.rs` の iOS 経路から呼ばれ、契約は `fandhe_device::pen_stylus`。
//! winit / egui-winit は iOS でペンか指かと `altitudeAngle` を捨てるため、薄い層で補う。
//! 位置はドライバが egui から取るので、ここでは扱わない。
//!
//! `.m` は OS の生値を渡すだけで、範囲検査と契約値への写像は [`decode`] が行う
//! （ホストでテストできるよう `cfg(any(ios, test))`）。
//!
//! # 再描画
//!
//! 値の `set` / `clear` の直後に `crate::ios::request_repaint` で次フレームを起こす
//! （入力が無くても egui が止まらない。契約表 4）。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 1 プロセス内の順序、指・ポインタの固定値を筆圧・傾きにしないこと、未定義 flags・範囲外・
//!   NaN を嘘の値にしないこと、離す・キャンセル・非アクティブ化・背景遷移で接触なしに戻すこと、
//!   install 失敗を成功値に畳まないこと
//! - 守らない: 1 フレーム内の複数点（coalesced / predicted touches）、主ポインタ 1 本以外、
//!   推定筆圧の後からの更新、方位角・ホバー、複数プロセス
//!
//! # 既知の天井・未確認
//!
//! - シミュレータでは Apple Pencil を再現できない（指として描ける）。実機確認は #354
//! - 観測専用 recognizer が winit view の touch 配送に影響しないかは実機未確認（#354）

use fandhe_device::pen_stylus::{altitude_from_ios_rad, PenSample, PointerKind};

const FLAG_PRESSURE: u32 = 1;
const FLAG_ALTITUDE: u32 = 2;

/// `.m` の生値を契約の値へ写す。未定義の flags は `None`（呼び出し側が接触なし扱いにし、嘘の値を置かない）。
/// 筆圧は flags が立ち有限のときだけ採る（丸めは `PenSample::new`）。傾きは flags が立ち
/// `0..=π/2` のときだけ度へ換算する（範囲検証は換算の前）。
fn decode(kind: i32, pressure: f32, altitude_rad: f32, flags: u32) -> Option<PenSample> {
    if flags & !(FLAG_PRESSURE | FLAG_ALTITUDE) != 0 {
        return None;
    }
    let kind = match kind {
        1 => PointerKind::Pen,
        2 => PointerKind::Touch,
        3 => PointerKind::Mouse,
        _ => PointerKind::Unknown,
    };
    let pressure = (flags & FLAG_PRESSURE != 0 && pressure.is_finite()).then_some(pressure);
    let altitude = if flags & FLAG_ALTITUDE != 0 {
        altitude_from_ios_rad(altitude_rad)
    } else {
        None
    };
    Some(PenSample::new(kind, pressure, altitude))
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::{Arc, OnceLock};

    use fandhe_device::pen_stylus::{PenSample, PenStylus, SharedPen};
    use fandhe_device::DeviceError;

    use super::decode;

    // SAFETY: `ios/fandhe_ios_pen_stylus.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_pen_stylus_install() -> i32;
    }

    /// `.m` の recognizer と `latest` が共有する最新値。
    static PEN: OnceLock<SharedPen> = OnceLock::new();

    fn pen() -> &'static SharedPen {
        PEN.get_or_init(SharedPen::new)
    }

    /// recognizer が主ポインタの値を置く（main thread、同期で戻る）。panic しない。
    #[allow(unsafe_code)]
    #[no_mangle]
    pub extern "C" fn fandhe_rust_pen_stylus_set(
        kind: i32,
        pressure: f32,
        altitude_rad: f32,
        flags: u32,
    ) {
        match decode(kind, pressure, altitude_rad, flags) {
            Some(s) => pen().set(s),
            None => pen().clear(),
        }
        crate::ios::request_repaint();
    }

    /// 接触の終了・キャンセル・非アクティブ化・背景遷移で接触なしに戻す。
    #[allow(unsafe_code)]
    #[no_mangle]
    pub extern "C" fn fandhe_rust_pen_stylus_clear() {
        pen().clear();
        crate::ios::request_repaint();
    }

    struct IosPenStylus {
        /// install 成功だけを握る。失敗は握らず次回再試行する。
        installed: OnceLock<()>,
    }

    impl PenStylus for IosPenStylus {
        #[allow(unsafe_code)]
        fn latest(&self) -> Result<Option<PenSample>, DeviceError> {
            if self.installed.get().is_none() {
                // SAFETY: 引数なし。`.m` は main thread 以外では取り付けず失敗コードを返す。
                match unsafe { fandhe_ios_pen_stylus_install() } {
                    0 => {
                        let _ = self.installed.set(());
                    }
                    _ => return Err(DeviceError::Internal("pen_stylus")),
                }
            }
            pen().latest()
        }
    }

    /// iOS の `PenStylus` 実装を返す（`device/pen_stylus.rs` の dispatch から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn PenStylus> {
        Arc::new(IosPenStylus {
            installed: OnceLock::new(),
        })
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// ホストのテストビルドにも載るための形。dispatch は iOS でしか呼ばない。
#[cfg(not(target_os = "ios"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::pen_stylus::PenStylus> {
    std::sync::Arc::new(fandhe_device::pen_stylus::UnsupportedPenStylus)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn undefined_flag_bits_are_none() {
        assert!(decode(1, 0.5, FRAC_PI_2, 4).is_none());
    }

    #[test]
    fn finger_and_pointer_have_no_pressure_or_tilt() {
        for k in [2, 3] {
            let s = decode(k, 1.0, FRAC_PI_2, 0).unwrap();
            assert_eq!(s.pressure, None);
            assert_eq!(s.altitude_deg, None);
        }
    }

    #[test]
    fn pen_with_pressure_and_upright() {
        let s = decode(1, 0.4, FRAC_PI_2, 3).unwrap();
        assert_eq!(s.kind, PointerKind::Pen);
        assert_eq!(s.pressure, Some(0.4));
        assert!((s.altitude_deg.unwrap() - 90.0).abs() < 1e-3);
    }

    #[test]
    fn nan_and_out_of_range_are_none_values() {
        let s = decode(1, f32::NAN, f32::NAN, 3).unwrap();
        assert_eq!((s.pressure, s.altitude_deg), (None, None));
        assert_eq!(decode(1, 0.5, 2.0, 3).unwrap().altitude_deg, None);
        assert_eq!(decode(1, 0.5, -0.1, 3).unwrap().altitude_deg, None);
    }

    #[test]
    fn finite_out_of_range_pressure_is_clamped() {
        assert_eq!(decode(1, 3.0, 0.0, 1).unwrap().pressure, Some(1.0));
    }

    #[test]
    fn unknown_kind_is_unknown() {
        assert_eq!(decode(0, 0.0, 0.0, 0).unwrap().kind, PointerKind::Unknown);
        assert_eq!(decode(9, 0.0, 0.0, 0).unwrap().kind, PointerKind::Unknown);
    }
}
