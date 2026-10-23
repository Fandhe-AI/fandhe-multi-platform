//! 気圧・照度・近接センサーの契約（Issue #609、親 #123）。端末の環境センサーの値を画面に出すための
//! 橋渡しで、`fandhe-egui-proxy` の `device/environment_sensors.rs` が種類ごとに購読の開始・停止と
//! 間隔つきの [`EnvironmentSensors::read`] を行う（pull 型。OS 側が最新値を持ち、画面側が読む）。
//! 方位（`compass`）と同型で、種類（[`SensorKind`]）を引数に持つ。
//!
//! # 契約
//!
//! - 結果は [`EnvReading`] か [`EnvSensorFailure`]。`DeviceError` へ variant を足すと既存の網羅 match が
//!   壊れるため機能固有の enum にする（`compass::CompassFailure` と同じ判断）
//! - [`EnvironmentSensors::read`] の `Ok(None)` は「未開始 / 停止後 / まだ値が無い / 範囲外を捨てた後 /
//!   古い値」。0 や直前の値で代用しない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **`UnsupportedOnOs` と `NotPresent` の区別**: 前者は OS に公開 API が無い（iOS の照度など）。
//!   後者は API はあるが端末に無い（Android の `getDefaultSensor` が null、Windows の `GetDefault` が
//!   null や列挙 0 件、iOS で近接を有効にしても `NO` のまま）
//! - **値の検査**: 気圧は hPa の範囲 [`PRESSURE_HPA_RANGE`] で検査する（iOS は kPa のまま検査してから
//!   換算、[`pressure_hpa_from_kpa`]）。照度は lx の範囲 [`LUX_RANGE`]、近接は 2 値。範囲外は値なしにする。
//!   検査は変換・丸めの前に行う（契約表 5）
//! - **古さ**: 気圧は受信から [`PRESSURE_STALE_MS`] を超えたら `Ok(None)`（[`is_fresh`]）。
//!   照度・近接は on-change で値が変わらない間は届かないため、古さの閾値を置かない
//! - **三値**（契約表 10）: 利用者キャンセルの経路は無い（「停止」は失敗ではなく待機へ戻す）。
//!   権限拒否・期限切れ（[`NO_READING_TIMEOUT_MS`] の間値が届かない）・OS 非対応・端末非搭載・その他は
//!   それぞれ別の失敗として表示する
//! - センサー値・HRESULT・NSError 等の OS 固有値をログへ出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 嘘の値（0・前回値・範囲外）を出さない、「OS に API が無い」と「端末に無い」を取り違えない、
//! 種類ごとに購読を二重にしない、停止・失敗・背景遷移・破棄で解除する、古い世代の値を捨てる。
//! 守らない: 絶対精度（較正・海面補正・センサー位置・色の影響）、on-change が次の event まで値を
//! 出さない遅れ、近接の距離（cm）、OS 間で値がそろうこと、iOS の近接で画面が消えること自体、
//! 複数プロセス、表示の遅れ（最大 [`READ_INTERVAL`] + センサー周期）、Windows を最小化している間。
//!
//! # 並行経路
//!
//! 経路は画面の driver 1 本に集約し、種類ごとの `wanted` の立ち上がり・立ち下がりでだけ
//! `start` / `stop` する。OS 側の非同期（Windows の近接の列挙）は実装側で世代を付け、停止後の結果を捨てる。
//!
//! # 後続の OS 実装への要点
//!
//! - iOS（#610）: 気圧は `CMAltimeter`（kPa）、近接は `UIDevice` の proximity monitoring。照度は常に
//!   `UnsupportedOnOs`。拒否コードは 2 種とも拾い、main queue で切り替え `dispatch_sync` は使わない。背景で値を捨てる
//! - Android（#611）: Java ヘルパーと同期 JNI の署名を 3 か所で一致させる。`onPause` / `onResume` /
//!   `onDestroy` を扱い、気圧だけ [`is_fresh`] で古さを判定、近接は [`proximity_from_distance`]
//! - Windows（#612）: 気圧・照度は同期 pull、近接は作業スレッドで 1 回だけ列挙して世代を付ける。
//!   `E_ACCESSDENIED` は `PermissionDenied`
//!
//! 定数は仮置きで、実機確認（#602）で見直す。

use std::ops::RangeInclusive;
use std::time::Duration;

/// 画面側が OS を読み直す間隔。
pub const READ_INTERVAL: Duration = Duration::from_millis(100);

/// 購読を始めてから（または値なしに戻ってから）この時間、値が届かなければ失敗とする（仮置き）。
pub const NO_READING_TIMEOUT_MS: f64 = 5000.0;

/// 気圧の受信からこの時間を超えた値は古いとして捨てる（仮置き）。
pub const PRESSURE_STALE_MS: f64 = 2000.0;

/// 受け入れる気圧（hPa、仮置き）。
pub const PRESSURE_HPA_RANGE: RangeInclusive<f64> = 300.0..=1100.0;

/// 受け入れる照度（lx、仮置き）。
pub const LUX_RANGE: RangeInclusive<f64> = 0.0..=200_000.0;

/// センサーの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensorKind {
    /// 気圧。
    Pressure,
    /// 照度。
    Light,
    /// 近接。
    Proximity,
}

impl SensorKind {
    /// 全種類。
    pub const ALL: [SensorKind; 3] = [
        SensorKind::Pressure,
        SensorKind::Light,
        SensorKind::Proximity,
    ];
}

/// 近接の 2 値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proximity {
    /// 近い。
    Near,
    /// 遠い。
    Far,
}

/// ある時点のセンサー値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnvReading {
    /// 気圧（hPa）。
    PressureHpa(f64),
    /// 照度（lx）。
    IlluminanceLux(f64),
    /// 近接。
    Proximity(Proximity),
}

impl EnvReading {
    /// この値の種類。
    pub fn kind(&self) -> SensorKind {
        match self {
            EnvReading::PressureHpa(_) => SensorKind::Pressure,
            EnvReading::IlluminanceLux(_) => SensorKind::Light,
            EnvReading::Proximity(_) => SensorKind::Proximity,
        }
    }
}

/// 取得の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvSensorFailure {
    /// OS に公開 API が無い・OS 版が足りない。
    UnsupportedOnOs,
    /// API はあるが端末にセンサーが無い。
    NotPresent,
    /// 利用者または OS 設定が利用を拒否した。
    PermissionDenied,
    /// 上記以外の OS 呼び出し失敗。
    Failed,
}

/// 環境センサー取得の契約（モジュール doc の「契約」節）。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait EnvironmentSensors: Send + Sync {
    /// 指定した種類の購読を始める。冪等。同期で分かる失敗はここで返す。
    fn start(&self, kind: SensorKind) -> Result<(), EnvSensorFailure>;
    /// 最新値を返す。未開始・停止後・値なし・古い値は `Ok(None)`。前回値や 0 で埋めない。
    fn read(&self, kind: SensorKind) -> Result<Option<EnvReading>, EnvSensorFailure>;
    /// 購読を止め、保持している値を捨てる。冪等で失敗しない。
    fn stop(&self, kind: SensorKind);
}

/// 対象外 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedEnvironmentSensors;

impl EnvironmentSensors for UnsupportedEnvironmentSensors {
    fn start(&self, _kind: SensorKind) -> Result<(), EnvSensorFailure> {
        Err(EnvSensorFailure::UnsupportedOnOs)
    }
    fn read(&self, _kind: SensorKind) -> Result<Option<EnvReading>, EnvSensorFailure> {
        Err(EnvSensorFailure::UnsupportedOnOs)
    }
    fn stop(&self, _kind: SensorKind) {}
}

/// 気圧（hPa）を検査する。有限かつ [`PRESSURE_HPA_RANGE`] の中だけ受ける。
pub fn validate_pressure_hpa(raw: f64) -> Option<f64> {
    (raw.is_finite() && PRESSURE_HPA_RANGE.contains(&raw)).then_some(raw)
}

/// iOS の kPa を hPa へ換算する。範囲検査（30〜110 kPa）を換算の前に行う。
pub fn pressure_hpa_from_kpa(kpa: f64) -> Option<f64> {
    if !kpa.is_finite() || !(30.0..=110.0).contains(&kpa) {
        return None;
    }
    Some(kpa * 10.0)
}

/// 照度（lx）を検査する。有限かつ [`LUX_RANGE`] の中だけ受ける。
pub fn validate_lux(raw: f64) -> Option<f64> {
    (raw.is_finite() && LUX_RANGE.contains(&raw)).then_some(raw)
}

/// Android の近接（距離と最大測定範囲）を 2 値へ写す。両方が有限、`max_range > 0`、
/// `0 <= distance <= max_range` のときだけ受け、`distance < max_range` なら近い。
pub fn proximity_from_distance(distance: f64, max_range: f64) -> Option<Proximity> {
    if !distance.is_finite() || !max_range.is_finite() || max_range <= 0.0 {
        return None;
    }
    if !(0.0..=max_range).contains(&distance) {
        return None;
    }
    Some(if distance < max_range {
        Proximity::Near
    } else {
        Proximity::Far
    })
}

/// 種類の不一致と範囲外を `None` にする。driver が実装の取りこぼしに対する多重防御に使う。
pub fn validate_reading(kind: SensorKind, reading: EnvReading) -> Option<EnvReading> {
    if reading.kind() != kind {
        return None;
    }
    match reading {
        EnvReading::PressureHpa(v) => validate_pressure_hpa(v).map(EnvReading::PressureHpa),
        EnvReading::IlluminanceLux(v) => validate_lux(v).map(EnvReading::IlluminanceLux),
        EnvReading::Proximity(_) => Some(reading),
    }
}

/// 受信から `now` までの経過が `0..=stale_ms` のときだけ新しいとする。時刻は単調時計のミリ秒。
pub fn is_fresh(received_ms: f64, now_ms: f64, stale_ms: f64) -> bool {
    let d = now_ms - received_ms;
    d.is_finite() && (0.0..=stale_ms).contains(&d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        for k in SensorKind::ALL {
            assert_eq!(
                UnsupportedEnvironmentSensors.start(k),
                Err(EnvSensorFailure::UnsupportedOnOs)
            );
            assert_eq!(
                UnsupportedEnvironmentSensors.read(k),
                Err(EnvSensorFailure::UnsupportedOnOs)
            );
            UnsupportedEnvironmentSensors.stop(k);
        }
    }

    #[test]
    fn pressure_bounds() {
        for bad in [299.9, 1100.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(validate_pressure_hpa(bad), None);
        }
        assert_eq!(validate_pressure_hpa(300.0), Some(300.0));
        assert_eq!(validate_pressure_hpa(1100.0), Some(1100.0));
    }

    #[test]
    fn kpa_checked_before_conversion() {
        assert_eq!(pressure_hpa_from_kpa(29.9), None);
        assert_eq!(pressure_hpa_from_kpa(110.1), None);
        assert_eq!(pressure_hpa_from_kpa(f64::NAN), None);
        let v = pressure_hpa_from_kpa(101.325).unwrap();
        assert!((v - 1013.25).abs() < 1e-9);
    }

    #[test]
    fn lux_bounds() {
        for bad in [-0.1, 200_000.1, f64::NAN, f64::INFINITY] {
            assert_eq!(validate_lux(bad), None);
        }
        assert_eq!(validate_lux(0.0), Some(0.0));
        assert_eq!(validate_lux(200_000.0), Some(200_000.0));
    }

    #[test]
    fn proximity_mapping() {
        assert_eq!(proximity_from_distance(0.0, 5.0), Some(Proximity::Near));
        assert_eq!(proximity_from_distance(5.0, 5.0), Some(Proximity::Far));
        assert_eq!(proximity_from_distance(4.9, 5.0), Some(Proximity::Near));
        for (d, m) in [
            (f64::NAN, 5.0),
            (1.0, f64::NAN),
            (-1.0, 5.0),
            (1.0, 0.0),
            (1.0, -5.0),
            (5.1, 5.0),
            (f64::INFINITY, f64::INFINITY),
        ] {
            assert_eq!(proximity_from_distance(d, m), None);
        }
    }

    #[test]
    fn validate_reading_rejects_mismatch_and_range() {
        use EnvReading as R;
        assert_eq!(
            validate_reading(SensorKind::Light, R::PressureHpa(1000.0)),
            None
        );
        assert_eq!(
            validate_reading(SensorKind::Pressure, R::PressureHpa(10.0)),
            None
        );
        assert_eq!(
            validate_reading(SensorKind::Light, R::IlluminanceLux(-1.0)),
            None
        );
        assert_eq!(
            validate_reading(SensorKind::Proximity, R::Proximity(Proximity::Near)),
            Some(R::Proximity(Proximity::Near))
        );
        assert_eq!(
            validate_reading(SensorKind::Pressure, R::PressureHpa(1013.0)),
            Some(R::PressureHpa(1013.0))
        );
    }

    #[test]
    fn freshness_bounds() {
        assert!(is_fresh(0.0, 2000.0, PRESSURE_STALE_MS));
        assert!(!is_fresh(0.0, 2000.1, PRESSURE_STALE_MS));
        assert!(!is_fresh(100.0, 99.0, PRESSURE_STALE_MS));
        assert!(!is_fresh(f64::NAN, 1.0, PRESSURE_STALE_MS));
    }

    #[test]
    fn reading_kind() {
        assert_eq!(EnvReading::PressureHpa(1.0).kind(), SensorKind::Pressure);
        assert_eq!(EnvReading::IlluminanceLux(1.0).kind(), SensorKind::Light);
        assert_eq!(
            EnvReading::Proximity(Proximity::Far).kind(),
            SensorKind::Proximity
        );
    }
}
