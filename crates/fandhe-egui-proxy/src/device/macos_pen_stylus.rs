//! macOS 向けペン・筆圧入力実装（Issue #355、親 #95）。`macos/fandhe_macos_pen_stylus.m`
//! （`NSEvent` の local monitor、`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/pen_stylus.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::pen_stylus`。
//!
//! pull 型: `.m` が最新の種類・筆圧・傾きを static に持ち、UI スレッド（main thread）の
//! [`PenStylus::latest`] が読む。登録（local monitor）は初回の `latest` でプロセスに 1 回だけ行う。
//! `.m` が返す生値は [`decode`] で契約の値へ写す。写像はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。`.m` は main thread 以外では失敗を返す（`cargo test` のワーカー
//! スレッドで実 FFI を呼ぶと `Internal` になる。成功系は #354 の Mac 目視）。
//!
//! # 既知の天井・未確認
//!
//! - `NSEvent.tilt`（`-1..1`）を `× 90°` で W3C の `tiltX/Y`（度）に見立てて高度角へ換算する。
//!   実機のタブレットでの軸の向き・換算の妥当性は未確認（#354）
//! - winit 経由のイベントが local monitor に届く順序（egui のフレームより前か）は未確認（#354）
//! - 位置は扱わない（egui のポインタから取る）。ホバー・消しゴムの区別はしない

use fandhe_device::pen_stylus::{altitude_from_tilt_deg, PenSample, PointerKind};
use fandhe_device::DeviceError;

const FLAG_PRESSURE: u32 = 1;
const FLAG_TILT: u32 = 2;

/// `.m` の `FandhePenRaw` と同じレイアウト。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Raw {
    pub(super) kind: i32,
    pub(super) pressure: f32,
    pub(super) tilt_x: f32,
    pub(super) tilt_y: f32,
    pub(super) flags: u32,
}

fn internal() -> DeviceError {
    DeviceError::Internal("pen_stylus")
}

/// 有限かつ `-1..=1` のときだけ度（`× 90`）へ。範囲検証は換算の前に行う。
fn tilt_deg(t: f32) -> Option<f32> {
    (t.is_finite() && (-1.0..=1.0).contains(&t)).then_some(t * 90.0)
}

/// `.m` の戻り値と生値を契約の結果へ写す。負のコード・未知のコード・未定義の flags は
/// 嘘の成功にせず `Internal`。非有限の値と範囲外の傾きは `None`、有限の筆圧は 0〜1 に丸める。
pub(super) fn decode(code: i32, raw: &Raw) -> Result<Option<PenSample>, DeviceError> {
    match code {
        0 => return Ok(None),
        1 => {}
        _ => return Err(internal()),
    }
    if raw.flags & !(FLAG_PRESSURE | FLAG_TILT) != 0 {
        return Err(internal());
    }
    let kind = match raw.kind {
        1 => PointerKind::Pen,
        2 => PointerKind::Touch,
        3 => PointerKind::Mouse,
        _ => PointerKind::Unknown,
    };
    // 範囲への丸めは `PenSample::new` に任せる（共通の契約）。ここでは非有限だけを未取得にする。
    let pressure =
        (raw.flags & FLAG_PRESSURE != 0 && raw.pressure.is_finite()).then_some(raw.pressure);
    let altitude = if raw.flags & FLAG_TILT != 0 {
        tilt_deg(raw.tilt_x)
            .zip(tilt_deg(raw.tilt_y))
            .and_then(|(x, y)| altitude_from_tilt_deg(x, y))
    } else {
        None
    };
    Ok(Some(PenSample::new(kind, pressure, altitude)))
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_pen_stylus.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_pen_stylus_install() -> i32;
        fn fandhe_macos_pen_stylus_read(out: *mut Raw) -> i32;
    }

    /// 0 = 登録済み。main thread 以外では失敗コードを返す。
    #[allow(unsafe_code)]
    pub(super) fn install() -> i32 {
        // SAFETY: 引数なし。`.m` は main thread 以外では登録せず失敗コードを返す。
        unsafe { fandhe_macos_pen_stylus_install() }
    }

    /// `out` へ最新値を書く。戻り値は `.m` 冒頭コメントのとおり。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は有効で排他的な書き込み先。`.m` はポインタを保持しない。
        unsafe { fandhe_macos_pen_stylus_read(out) }
    }
}

#[cfg(target_os = "macos")]
struct MacPenStylus {
    /// 登録の結果（初回だけ行い、結果を握る。失敗は毎回 `Internal`）。
    installed: std::sync::OnceLock<bool>,
}

#[cfg(target_os = "macos")]
impl fandhe_device::pen_stylus::PenStylus for MacPenStylus {
    fn latest(&self) -> Result<Option<PenSample>, DeviceError> {
        // main thread 以外（コード 2）は登録結果を握らず、次回の main thread で登録し直す。
        if self.installed.get().is_none() {
            match ffi::install() {
                0 => {
                    let _ = self.installed.set(true);
                }
                2 => return Err(internal()),
                _ => {
                    let _ = self.installed.set(false);
                }
            }
        }
        if self.installed.get() != Some(&true) {
            return Err(internal());
        }
        let mut raw = Raw::default();
        let code = ffi::read(&mut raw);
        decode(code, &raw)
    }
}

/// macOS の `PenStylus` 実装を返す（`device/pen_stylus.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::pen_stylus::PenStylus> {
    std::sync::Arc::new(MacPenStylus {
        installed: std::sync::OnceLock::new(),
    })
}

/// ホストのテストビルドにも載るための形。dispatch は macOS でしか呼ばない。
#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::pen_stylus::PenStylus> {
    std::sync::Arc::new(fandhe_device::pen_stylus::UnsupportedPenStylus)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(kind: i32, pressure: f32, tx: f32, ty: f32, flags: u32) -> Raw {
        Raw {
            kind,
            pressure,
            tilt_x: tx,
            tilt_y: ty,
            flags,
        }
    }

    /// main thread 以外では実 FFI が失敗を返す（`install` が 2 を返し `Internal`）。
    #[cfg(target_os = "macos")]
    #[test]
    fn off_main_thread_is_internal_error() {
        let r = implementation().latest();
        assert!(matches!(r, Err(DeviceError::Internal("pen_stylus"))));
    }

    #[test]
    fn code_zero_is_no_contact() {
        assert_eq!(decode(0, &Raw::default()).unwrap(), None);
    }

    #[test]
    fn negative_and_unknown_codes_are_internal() {
        for c in [-1, -2, -3, 2, 99] {
            assert!(matches!(
                decode(c, &Raw::default()),
                Err(DeviceError::Internal("pen_stylus"))
            ));
        }
    }

    #[test]
    fn undefined_flag_bits_are_internal() {
        assert!(decode(1, &raw(1, 0.5, 0.0, 0.0, 4)).is_err());
    }

    #[test]
    fn pen_with_pressure_and_tilt() {
        let s = decode(1, &raw(1, 0.4, 0.0, 0.0, 3)).unwrap().unwrap();
        assert_eq!(s.kind, PointerKind::Pen);
        assert_eq!(s.pressure, Some(0.4));
        assert_eq!(s.altitude_deg, Some(90.0));
        let t = decode(1, &raw(1, 0.4, 0.5, 0.0, 3)).unwrap().unwrap();
        assert!((t.altitude_deg.unwrap() - 45.0).abs() < 1e-3);
    }

    #[test]
    fn mouse_has_no_pressure_or_tilt() {
        let s = decode(1, &raw(3, 0.0, 0.0, 0.0, 0)).unwrap().unwrap();
        assert_eq!(
            (s.kind, s.pressure, s.altitude_deg),
            (PointerKind::Mouse, None, None)
        );
    }

    #[test]
    fn unknown_kind_maps_to_unknown() {
        let s = decode(1, &raw(42, 0.0, 0.0, 0.0, 0)).unwrap().unwrap();
        assert_eq!(s.kind, PointerKind::Unknown);
    }

    #[test]
    fn out_of_range_or_nan_values_become_none() {
        let s = decode(1, &raw(1, f32::NAN, 2.0, 0.0, 3)).unwrap().unwrap();
        assert_eq!((s.pressure, s.altitude_deg), (None, None));
        let s = decode(1, &raw(1, f32::INFINITY, f32::NAN, 0.0, 3))
            .unwrap()
            .unwrap();
        assert_eq!((s.pressure, s.altitude_deg), (None, None));
    }

    #[test]
    fn finite_out_of_range_pressure_is_clamped() {
        let s = decode(1, &raw(1, 1.5, 0.0, 0.0, 1)).unwrap().unwrap();
        assert_eq!(s.pressure, Some(1.0));
        let s = decode(1, &raw(1, -0.2, 0.0, 0.0, 1)).unwrap().unwrap();
        assert_eq!(s.pressure, Some(0.0));
    }
}
