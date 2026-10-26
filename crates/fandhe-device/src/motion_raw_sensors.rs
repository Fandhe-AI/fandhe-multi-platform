//! ジャイロ・磁気・線形加速度の生値の契約（親 #180、Issue #898）。端末の生センサー値を画面に出すための
//! 橋渡しで、`fandhe-egui-proxy` の `device/motion_raw_sensors.rs` が種類ごとに購読の開始・停止と
//! 間隔つきの [`MotionRawSensors::read`] を行う（pull 型。OS 側が最新値を持ち、画面側が読む）。
//! `environment_sensors` と同型で、種類（[`RawKind`]）を引数に持つ。`motion`（傾き）とは別の機能。
//!
//! # 契約
//!
//! - 結果は [`RawReading`] か [`RawSensorFailure`]。`DeviceError` へ variant を足すと既存の網羅 match が
//!   壊れるため機能固有の enum にする（`environment_sensors::EnvSensorFailure` と同じ判断）
//! - [`MotionRawSensors::read`] の `Ok(None)` は「未開始 / 停止後 / まだ値が無い / 非表示 / 古い値 /
//!   成分が欠けた」。0 や直前の値で代用しない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **`UnsupportedOnOs` と `NotPresent` の区別**: 前者は OS に公開 API が無い・使えない（Web の磁場など）。
//!   後者は API はあるが端末に無い（`getDefaultSensor` が null、`GetDefault` が null、
//!   `is*Available` が false）。API の結果をそのまま写し、事前判定はしない
//! - **単位はそろえない**: OS が出した単位のまま [`RawUnit`] を付ける。種類と単位の組は [`RawUnit::fits`]
//! - **値の検査**: 3 軸がすべて有限で、単位が種類に合うときだけ受ける（[`validate_reading`]）。
//!   検査は値を詰め替える前に行う（契約表 5）。値域の閾値は置かない
//! - **期限・古さの閾値を置かない**: 値が届かない間は待機のまま。古い値は OS が示す事実だけで捨てる
//! - **三値**（契約表 10）: 利用者がキャンセルする経路は無い。「停止」は失敗ではなく待機へ戻す。
//!   権限拒否・OS 非対応・端末非搭載・その他はそれぞれ別の失敗として表示する
//! - センサー値・HRESULT・NSError 等の OS 固有値をログへ出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 嘘の値（0・前回値・NaN・∞・単位違い）を出さない、「OS に API が無い」と「端末に無い」を
//! 取り違えない、種類ごとに購読を二重にしない、停止・失敗・背景遷移・破棄で解除する、古い世代の値を捨てる、
//! iOS の `CMMotionManager` を 1 つにする。
//! 守らない: 精度と較正、OS 間で単位・軸がそろうこと、画面回転、周期と表示の遅れ（最大 [`READ_INTERVAL`] +
//! センサー周期）、値が届かないまま待ち続ける場合、ブラウザ差、`tilt` と同時に使ったときの周期、
//! 複数タブ・複数プロセス。
//!
//! # 並行経路
//!
//! 経路は画面の driver 1 本に集約し、種類ごとの `wanted` の立ち上がり・立ち下がりでだけ
//! `start` / `stop` する。背景遷移は driver に持ち込まず、各 OS の実装が OS の事実で扱う。
//!
//! # 後続の OS 実装への要点
//!
//! - Windows（#899）: WinRT `Windows.Devices.Sensors` の `Gyrometer`（°/s）・`Magnetometer`（µT）などを
//!   `GetDefault` で取る。`GetCurrentReading` が null なら値なし、null の `GetDefault` は
//!   `NotPresent`、`E_ACCESSDENIED` は `PermissionDenied`。線形加速度の取り方は #899 で確定する
//! - iOS（#900）: `CMMotionManager`（`g_motion_manager` を `tilt` と共有）。ジャイロは rad/s、磁気は µT、
//!   線形加速度は `userAcceleration`（g）。`timestamp` と `systemUptime` を比べて古い値を捨てる。
//!   `is*Available` が false なら `NotPresent`
//! - Android（#901）: `Sensor.TYPE_GYROSCOPE`（rad/s）・`TYPE_MAGNETIC_FIELD`（µT）・
//!   `TYPE_LINEAR_ACCELERATION`（m/s²）。`onPause` で解除、`onDestroy` の後は `Failed`
//! - Web（#902、`fandhe-web-dom`）: `DeviceMotionEvent`（`rotationRate` は °/s、`acceleration` は m/s²）。
//!   磁場は `UnsupportedOnOs`。iOS Safari の `requestPermission()` は「開始」と同じ同期区間で呼ぶ。
//!   非表示のときと成分が null のときは値なし

use std::time::Duration;

/// 画面側が OS を読み直す間隔。
pub const READ_INTERVAL: Duration = Duration::from_millis(100);

/// センサーの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RawKind {
    /// 角速度（ジャイロ）。
    AngularVelocity,
    /// 磁場。
    MagneticField,
    /// 線形加速度（重力を除く）。
    LinearAcceleration,
}

impl RawKind {
    /// 全種類。
    pub const ALL: [RawKind; 3] = [
        RawKind::AngularVelocity,
        RawKind::MagneticField,
        RawKind::LinearAcceleration,
    ];
}

/// 値の単位（OS が出した単位のまま持つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawUnit {
    /// rad/s。
    RadPerSec,
    /// 度/秒。
    DegPerSec,
    /// マイクロテスラ。
    MicroTesla,
    /// 標準重力加速度 g。
    StandardGravity,
    /// m/s²。
    MetersPerSec2,
}

impl RawUnit {
    /// この単位が `kind` の値として妥当か。
    pub fn fits(self, kind: RawKind) -> bool {
        match kind {
            RawKind::AngularVelocity => matches!(self, RawUnit::RadPerSec | RawUnit::DegPerSec),
            RawKind::MagneticField => self == RawUnit::MicroTesla,
            RawKind::LinearAcceleration => {
                matches!(self, RawUnit::StandardGravity | RawUnit::MetersPerSec2)
            }
        }
    }
}

/// ある時点の 3 軸の値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawReading {
    /// x 軸。
    pub x: f64,
    /// y 軸。
    pub y: f64,
    /// z 軸。
    pub z: f64,
    /// 単位。
    pub unit: RawUnit,
}

/// 取得の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawSensorFailure {
    /// OS に公開 API が無い・使えない。
    UnsupportedOnOs,
    /// API はあるが端末にセンサーが無い。
    NotPresent,
    /// 利用者または OS 設定が利用を拒否した。
    PermissionDenied,
    /// 上記以外の OS 呼び出し失敗。
    Failed,
}

/// 生センサー取得の契約（モジュール doc の「契約」節）。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait MotionRawSensors: Send + Sync {
    /// 指定した種類の購読を始める。冪等。同期で分かる失敗はここで返す。
    fn start(&self, kind: RawKind) -> Result<(), RawSensorFailure>;
    /// 最新値を返す。未開始・停止後・値なし・古い値・非表示は `Ok(None)`。前回値や 0 で埋めない。
    fn read(&self, kind: RawKind) -> Result<Option<RawReading>, RawSensorFailure>;
    /// 購読を止め、保持している値を捨てる。冪等で失敗しない。
    fn stop(&self, kind: RawKind);
}

/// 対象外 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedMotionRawSensors;

impl MotionRawSensors for UnsupportedMotionRawSensors {
    fn start(&self, _kind: RawKind) -> Result<(), RawSensorFailure> {
        Err(RawSensorFailure::UnsupportedOnOs)
    }
    fn read(&self, _kind: RawKind) -> Result<Option<RawReading>, RawSensorFailure> {
        Err(RawSensorFailure::UnsupportedOnOs)
    }
    fn stop(&self, _kind: RawKind) {}
}

/// 3 軸がすべて有限で、単位が種類に合う値だけを返す。外れた値は `None`（値なし）。
pub fn validate_reading(kind: RawKind, r: RawReading) -> Option<RawReading> {
    (r.x.is_finite() && r.y.is_finite() && r.z.is_finite() && r.unit.fits(kind)).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd(x: f64, y: f64, z: f64, unit: RawUnit) -> RawReading {
        RawReading { x, y, z, unit }
    }

    #[test]
    fn unsupported_never_succeeds() {
        for k in RawKind::ALL {
            assert_eq!(
                UnsupportedMotionRawSensors.start(k),
                Err(RawSensorFailure::UnsupportedOnOs)
            );
            assert_eq!(
                UnsupportedMotionRawSensors.read(k),
                Err(RawSensorFailure::UnsupportedOnOs)
            );
            UnsupportedMotionRawSensors.stop(k);
        }
    }

    #[test]
    fn non_finite_on_any_axis_is_rejected() {
        let k = RawKind::AngularVelocity;
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for r in [
                rd(bad, 0.0, 0.0, RawUnit::RadPerSec),
                rd(0.0, bad, 0.0, RawUnit::RadPerSec),
                rd(0.0, 0.0, bad, RawUnit::RadPerSec),
            ] {
                assert_eq!(validate_reading(k, r), None);
            }
        }
    }

    #[test]
    fn mismatched_unit_is_rejected() {
        assert_eq!(
            validate_reading(
                RawKind::MagneticField,
                rd(1.0, 2.0, 3.0, RawUnit::RadPerSec)
            ),
            None
        );
        assert_eq!(
            validate_reading(
                RawKind::LinearAcceleration,
                rd(1.0, 2.0, 3.0, RawUnit::MicroTesla)
            ),
            None
        );
    }

    #[test]
    fn valid_values_pass() {
        let cases = [
            (RawKind::AngularVelocity, RawUnit::RadPerSec),
            (RawKind::AngularVelocity, RawUnit::DegPerSec),
            (RawKind::MagneticField, RawUnit::MicroTesla),
            (RawKind::LinearAcceleration, RawUnit::StandardGravity),
            (RawKind::LinearAcceleration, RawUnit::MetersPerSec2),
        ];
        for (k, u) in cases {
            let r = rd(0.1, -0.2, 0.0, u);
            assert_eq!(validate_reading(k, r), Some(r));
        }
    }
}
