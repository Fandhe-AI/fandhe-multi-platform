//! 方位（コンパス）の契約（Issue #1404、親 #1154）。端末の向きを磁北基準の度で
//! 画面に出すための橋渡しで、`fandhe-egui-proxy` の `device/compass.rs` が購読の開始・停止と
//! 間隔つきの [`Compass::read`] を行う（pull 型。OS 側が最新値を持ち、画面側が読む）。
//!
//! # 契約
//!
//! - 結果は [`Heading`] か [`CompassFailure`]。`DeviceError` へ variant を足すと既存の網羅 match が
//!   壊れるため機能固有の enum にする（`pedometer::PedometerFailure` と同じ判断）
//! - [`Compass::read`] の `Ok(None)` は「まだ値が無い / 停止後 / 無効なサンプルを捨てた後」。
//!   0° や直前の値で代用しない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - 方位は磁北から時計回りの `[0.0, 360.0)`。端末を水平に持った縦向きの上辺が向く方位
//! - **三値**: 利用者キャンセルの経路は無い（「停止」は失敗ではなく待機へ戻す）。権限拒否・
//!   期限切れ（[`NO_READING_TIMEOUT_MS`] の間値が届かない）・非対応はそれぞれ別の失敗として表示する
//! - 方位・精度・HRESULT 等の OS 固有値をログへ出さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 嘘の値（0° や古い値）を出さない、二重に購読登録しない、停止・失敗で購読を解除する。
//! 守らない: 偏角（真北）、Web の絶対方位の基準、磁気の乱れ、端末を立てた姿勢・横向き、揺れ、
//! Windows の 2° 刻み、OS 間の値の一致、Permissions Policy の区別、複数タブ・複数プロセス、
//! 表示の遅れ（最大 [`READ_INTERVAL`] + センサー周期）。
//!
//! 契約表 10（三値）・11（並行経路）への対応: 経路は画面の driver 1 本に集約され、購読の
//! 開始・停止は `wanted` の立ち上がり・立ち下がりでだけ行う。

use std::time::Duration;

/// 画面側が OS を読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_millis(100);

/// 購読を始めてから（または値なしに戻ってから）この時間、値が届かなければ失敗とする。
pub const NO_READING_TIMEOUT_MS: f64 = 3000.0;

/// 方位の精度。表示するだけで判定には使わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accuracy {
    /// OS が精度を返さない。
    Unknown,
    /// 信頼できない。
    Unreliable,
    /// おおよそ。
    Approximate,
    /// 高い。
    High,
}

/// ある時点の方位。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heading {
    /// 磁北から時計回りの度。`[0.0, 360.0)`。
    pub degrees: f64,
    /// 精度。
    pub accuracy: Accuracy,
}

/// 方位取得の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompassFailure {
    /// センサーが無い・API が無い・OS 版が足りない。
    Unsupported,
    /// 利用者または OS 設定が方位の利用を拒否した。
    PermissionDenied,
    /// 上記以外の OS 呼び出し失敗。
    Failed,
}

/// 方位取得の契約（モジュール doc の「契約」節）。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait Compass: Send + Sync {
    /// 購読を始める。冪等。同期で分かる失敗（センサー無し・API 無し）はここで返す。
    fn start(&self) -> Result<(), CompassFailure>;
    /// 最新の方位を返す。未開始・停止後・値なしは `Ok(None)`。直前の値を返し続けない。
    fn read(&self) -> Result<Option<Heading>, CompassFailure>;
    /// 購読を止め、保持している最新値を捨てる。冪等で失敗しない。
    fn stop(&self);
}

/// 対象外 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedCompass;

impl Compass for UnsupportedCompass {
    fn start(&self) -> Result<(), CompassFailure> {
        Err(CompassFailure::Unsupported)
    }
    fn read(&self) -> Result<Option<Heading>, CompassFailure> {
        Err(CompassFailure::Unsupported)
    }
    fn stop(&self) {}
}

/// OS の生の値を `[0.0, 360.0)` の方位へ検査する。有限かつ `0.0..=360.0` だけを受け、
/// `360.0` は `0.0` にする。丸めや剰余で範囲外を通さない（`None` は無効サンプル）。
pub fn normalize_heading(raw: f64) -> Option<f64> {
    if !raw.is_finite() || !(0.0..=360.0).contains(&raw) {
        return None;
    }
    Some(if raw == 360.0 { 0.0 } else { raw })
}

/// 8 方位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompassPoint {
    /// 北。
    North,
    /// 北東。
    NorthEast,
    /// 東。
    East,
    /// 南東。
    SouthEast,
    /// 南。
    South,
    /// 南西。
    SouthWest,
    /// 西。
    West,
    /// 北西。
    NorthWest,
}

/// 方位から 8 方位を決める（丸める前の値で判定。境界は時計回り側に含める）。
pub fn compass_point(degrees: f64) -> CompassPoint {
    use CompassPoint::*;
    const ALL: [CompassPoint; 8] = [
        North, NorthEast, East, SouthEast, South, SouthWest, West, NorthWest,
    ];
    ALL[(((degrees + 22.5) / 45.0).floor() as i64).rem_euclid(8) as usize]
}

/// 表示用の整数度。359.6 は 0 になり「360°」とは出さない。
pub fn display_degrees(degrees: f64) -> u16 {
    (degrees.round() as i64).rem_euclid(360) as u16
}

/// 値が届かない時間の追跡（時刻はミリ秒の `f64`。wasm32 で `Instant::now()` が panic するため
/// 呼び出し側が単調時計から作って渡す）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WaitTracker {
    since: Option<f64>,
}

impl WaitTracker {
    /// 起点を置く（購読の開始時）。
    pub fn begin(&mut self, now_ms: f64) {
        self.since = Some(now_ms);
    }

    /// 1 回の読み取り結果を記録する。値があれば起点を消し、値なしで起点が無ければ今を起点にする。
    /// 起点から [`NO_READING_TIMEOUT_MS`] を超えたら `true`（期限切れ）。
    pub fn observe(&mut self, has_value: bool, now_ms: f64) -> bool {
        if has_value {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now_ms);
        now_ms - since > NO_READING_TIMEOUT_MS
    }

    /// 起点を捨てる（停止時）。
    pub fn reset(&mut self) {
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_invalid_and_wraps_360() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 360.1] {
            assert_eq!(normalize_heading(bad), None);
        }
        assert_eq!(normalize_heading(360.0), Some(0.0));
        assert_eq!(normalize_heading(0.0), Some(0.0));
        assert_eq!(normalize_heading(359.999), Some(359.999));
    }

    #[test]
    fn points_at_boundaries() {
        use CompassPoint::*;
        assert_eq!(compass_point(0.0), North);
        assert_eq!(compass_point(22.4), North);
        assert_eq!(compass_point(22.5), NorthEast);
        assert_eq!(compass_point(67.5), East);
        assert_eq!(compass_point(112.5), SouthEast);
        assert_eq!(compass_point(157.5), South);
        assert_eq!(compass_point(202.5), SouthWest);
        assert_eq!(compass_point(247.5), West);
        assert_eq!(compass_point(292.5), NorthWest);
        assert_eq!(compass_point(337.5), North);
        assert_eq!(compass_point(359.9), North);
    }

    #[test]
    fn display_never_shows_360() {
        assert_eq!(display_degrees(359.6), 0);
        assert_eq!(display_degrees(359.4), 359);
        assert_eq!(display_degrees(0.0), 0);
    }

    #[test]
    fn tracker_expires_only_after_timeout() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        assert!(!t.observe(false, NO_READING_TIMEOUT_MS));
        assert!(t.observe(false, NO_READING_TIMEOUT_MS + 1.0));
    }

    #[test]
    fn tracker_restarts_after_live_and_clears_on_value() {
        let mut t = WaitTracker::default();
        t.begin(0.0);
        assert!(!t.observe(true, 10_000.0));
        // Live から値なしに戻った時刻が新しい起点になる。
        assert!(!t.observe(false, 20_000.0));
        assert!(!t.observe(false, 22_000.0));
        assert!(t.observe(false, 23_001.0));
        t.reset();
        assert!(!t.observe(false, 99_999.0));
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(UnsupportedCompass.start(), Err(CompassFailure::Unsupported));
        assert_eq!(UnsupportedCompass.read(), Err(CompassFailure::Unsupported));
        UnsupportedCompass.stop();
    }
}
