//! ペン・筆圧入力の契約（Issue #355、親 #95）。ペン / 指 / マウスの区別、筆圧、傾き（ペンと
//! 画面の角度）を、OS の薄い層から「いま接触している主ポインタの最新値」として画面へ渡す橋渡し。
//! `fandhe-egui-proxy` の `device/pen_stylus.rs` が押下中だけ [`PenStylus::latest`] を pull し、
//! 位置は egui のポインタから取る（winit / egui-winit はペンか指かの区別・傾きを渡さず、
//! macOS では筆圧も渡さないため、種類・筆圧・傾きだけを OS 層から取る）。
//!
//! # 契約
//!
//! - 結果は 3 種類: 値あり（`Ok(Some)`）/ 接触なし（`Ok(None)`）/ 失敗（`Err`）。
//!   非対応の OS は `Err(DeviceError::Unsupported("pen_stylus"))`、OS 呼び出しの失敗は
//!   `Err(DeviceError::Internal("pen_stylus"))`。権限は要らないので `PermissionDenied` は返さない
//! - 離す・キャンセル・背景遷移・近接の離脱・アプリの非アクティブ化では必ず `clear` し、
//!   最後の値を出し続けない（`device-bridge-contracts.md` 1・6）
//! - マウスの固定値（0.5 / 1.0）を筆圧として扱わない。ペン以外は `pressure: None` でよい
//! - 筆圧は `0..=1`、傾きは `altitude_deg`（`0..=90`、90 が画面に垂直）。非有限値は `None`、
//!   範囲外は丸める（[`PenSample::new`]）
//! - OS 側のイベント登録はプロセスで 1 回だけにする
//! - 筆圧・座標・描画内容をログへ出さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の順序、嘘の値を出さないこと（失敗を成功に畳まない・離したら消す）、
//! OS 側の登録の解放。
//! 守らない: 1 フレーム内の複数点（最新の筆圧を共有する）、主ポインタ 1 本以外、Web で傾きを
//! 報告しない機器の 0（垂直と区別できない）、Pencil の推定筆圧の後からの更新、方位角・ホバー・
//! 消しゴムの区別・coalesced events、静止したままの筆圧変化（次のフレームまで反映しない）、
//! 複数タブ・複数プロセス。
//!
//! 契約表 10（三値）・11（並行経路）への対応: 経路は画面の effect ドライバ 1 本に集約され、
//! 期限も OS 側の非同期確定も無い。キャンセルは [`SharedPen::clear`]（`Ok(None)`）で表す。

use std::sync::{Arc, Mutex};

use crate::DeviceError;

/// ポインタの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    /// ペン・スタイラス。
    Pen,
    /// 指。
    Touch,
    /// マウス・トラックパッド。
    Mouse,
    /// OS が種類を返さない。
    Unknown,
}

/// ある時点の入力。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenSample {
    /// 種類。
    pub kind: PointerKind,
    /// 筆圧（`0..=1`）。取れない・ペン以外は `None`。
    pub pressure: Option<f32>,
    /// ペンと画面の角度（度、`0..=90`、90 が画面に垂直）。取れなければ `None`。
    pub altitude_deg: Option<f32>,
}

/// 有限なら `lo..=hi` に丸め、非有限は `None`（`f32::clamp` は NaN を通すため先に判定する）。
fn clamp_finite(v: f32, lo: f32, hi: f32) -> Option<f32> {
    v.is_finite().then(|| v.clamp(lo, hi))
}

impl PenSample {
    /// 非有限値を `None` にし、範囲外を丸めて作る。OS の生の値は必ずここを通す。
    pub fn new(kind: PointerKind, pressure: Option<f32>, altitude_deg: Option<f32>) -> Self {
        Self {
            kind,
            pressure: pressure.and_then(|p| clamp_finite(p, 0.0, 1.0)),
            altitude_deg: altitude_deg.and_then(|a| clamp_finite(a, 0.0, 90.0)),
        }
    }
}

/// ペン入力取得の契約（モジュール doc の「契約」節）。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait PenStylus: Send + Sync {
    /// 最新の入力を返す。`Ok(None)` は接触なし（離した・キャンセル・背景遷移後）。
    fn latest(&self) -> Result<Option<PenSample>, DeviceError>;
}

/// 非対応 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedPenStylus;

impl PenStylus for UnsupportedPenStylus {
    fn latest(&self) -> Result<Option<PenSample>, DeviceError> {
        Err(DeviceError::Unsupported("pen_stylus"))
    }
}

/// push 型の OS（iOS・Android・Windows）が最新値を置く共有状態。判定と更新は同じ Mutex の内側で行う。
#[derive(Debug, Clone, Default)]
pub struct SharedPen {
    inner: Arc<Mutex<Option<PenSample>>>,
}

impl SharedPen {
    /// 空（接触なし）で作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// 最新値を置く。
    pub fn set(&self, sample: PenSample) {
        *self.inner.lock().unwrap_or_else(|p| p.into_inner()) = Some(sample);
    }

    /// 接触なしに戻す。
    pub fn clear(&self) {
        *self.inner.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

impl PenStylus for SharedPen {
    fn latest(&self) -> Result<Option<PenSample>, DeviceError> {
        Ok(*self.inner.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

/// W3C Pointer Events の `tiltX` / `tiltY`（度、各 `-90..=90`）から高度角（度）を求める。
/// macOS（`tilt × 90°`）・Windows・Web が使う。範囲外・非有限は `None`（変換の前に検証する）。
pub fn altitude_from_tilt_deg(tilt_x: f32, tilt_y: f32) -> Option<f32> {
    let ok = |t: f32| t.is_finite() && (-90.0..=90.0).contains(&t);
    if !ok(tilt_x) || !ok(tilt_y) {
        return None;
    }
    if tilt_x == 0.0 && tilt_y == 0.0 {
        return Some(90.0);
    }
    if tilt_x.abs() == 90.0 || tilt_y.abs() == 90.0 {
        return Some(0.0);
    }
    let tx = f64::from(tilt_x).to_radians().tan();
    let ty = f64::from(tilt_y).to_radians().tan();
    let alt = (1.0 / (tx * tx + ty * ty).sqrt()).atan().to_degrees();
    Some(alt as f32)
}

/// Android の `AXIS_TILT`（ラジアン、`0..=π/2`、0 が垂直）から高度角（度）を求める。
pub fn altitude_from_android_tilt_rad(rad: f32) -> Option<f32> {
    if !rad.is_finite() || !(0.0..=std::f32::consts::FRAC_PI_2).contains(&rad) {
        return None;
    }
    Some(90.0 - rad.to_degrees())
}

/// iOS の `UITouch.altitudeAngle`（ラジアン、`0..=π/2`、π/2 が垂直）から高度角（度）を求める。
pub fn altitude_from_ios_rad(rad: f32) -> Option<f32> {
    if !rad.is_finite() || !(0.0..=std::f32::consts::FRAC_PI_2).contains(&rad) {
        return None;
    }
    Some(rad.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> PenSample {
        PenSample::new(PointerKind::Pen, Some(0.5), Some(45.0))
    }

    #[test]
    fn shared_set_latest_clear() {
        let p = SharedPen::new();
        assert_eq!(p.latest().unwrap(), None);
        p.set(s());
        assert_eq!(p.latest().unwrap(), Some(s()));
        p.clear();
        assert_eq!(p.latest().unwrap(), None);
    }

    #[test]
    fn clone_shares_state() {
        let p = SharedPen::new();
        let q = p.clone();
        p.set(s());
        assert_eq!(q.latest().unwrap(), Some(s()));
    }

    #[test]
    fn new_drops_nan_and_clamps() {
        let x = PenSample::new(PointerKind::Pen, Some(f32::NAN), Some(f32::INFINITY));
        assert_eq!((x.pressure, x.altitude_deg), (None, None));
        let y = PenSample::new(PointerKind::Pen, Some(2.0), Some(-5.0));
        assert_eq!((y.pressure, y.altitude_deg), (Some(1.0), Some(0.0)));
    }

    #[test]
    fn unsupported_is_never_success() {
        assert!(matches!(
            UnsupportedPenStylus.latest(),
            Err(DeviceError::Unsupported("pen_stylus"))
        ));
    }

    #[test]
    fn tilt_conversion_boundaries() {
        assert_eq!(altitude_from_tilt_deg(0.0, 0.0), Some(90.0));
        assert_eq!(altitude_from_tilt_deg(90.0, 0.0), Some(0.0));
        assert_eq!(altitude_from_tilt_deg(0.0, -90.0), Some(0.0));
        let a = altitude_from_tilt_deg(45.0, 0.0).unwrap();
        assert!((a - 45.0).abs() < 1e-3);
        assert_eq!(altitude_from_tilt_deg(f32::NAN, 0.0), None);
        assert_eq!(altitude_from_tilt_deg(91.0, 0.0), None);
    }

    #[test]
    fn android_and_ios_conversion() {
        use std::f32::consts::FRAC_PI_2;
        assert_eq!(altitude_from_android_tilt_rad(0.0), Some(90.0));
        assert!((altitude_from_android_tilt_rad(FRAC_PI_2).unwrap()).abs() < 1e-3);
        assert!((altitude_from_ios_rad(FRAC_PI_2).unwrap() - 90.0).abs() < 1e-3);
        assert_eq!(altitude_from_ios_rad(0.0), Some(0.0));
        assert_eq!(altitude_from_android_tilt_rad(-0.1), None);
        assert_eq!(altitude_from_ios_rad(2.0), None);
        assert_eq!(altitude_from_ios_rad(f32::NAN), None);
    }
}
