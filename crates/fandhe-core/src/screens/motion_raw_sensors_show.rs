//! Phase 11 の拡充ループ（親 #180）型サンプル「ジャイロ・磁気・線形加速度」（Issue #898）。
//! 「開始」「停止」の 1 組で 3 種類（角速度・磁場・線形加速度）の購読を始め・止め、3 行に現在値を出す。
//! `FANDHE_PROXY_APP=motion_raw_sensors_show` で選ばれ、`fandhe-egui-proxy` の `screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod motion_raw_sensors_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `device/motion_raw_sensors.rs` が行い、[`apply_reading`] / [`apply_failure`] で注入する。
//! 状態・定数・文言は全対象 OS 分をここで定義し、後続の OS 実装（#899〜#902）は画面を触らない。
//! OS 固有の操作 UI は置かない。Web の `requestPermission()` も「開始」と同じ同期区間で呼ぶため
//! 専用ボタンは要らない。
//!
//! 嘘の値を出さない: 待機・失敗は数値を持たない（[`RawSensorView::Live`] だけが持つ）。
//! 古い世代の完了は種類ごとの [`RawRowState::session`] で捨てる。1 つの種類の失敗は他へ波及させない。

use crate::{Screen, WidgetId};

/// 「開始」ボタン。
pub const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタン。
pub const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「開始」の文言（3 種類まとめて 1 組）。
pub const START_LABEL: &str = "開始";
/// 「停止」の文言。
pub const STOP_LABEL: &str = "停止";

/// 行名: 角速度。
pub const ANGULAR_VELOCITY_NAME: &str = "角速度";
/// 行名: 磁場。
pub const MAGNETIC_FIELD_NAME: &str = "磁場";
/// 行名: 線形加速度。
pub const LINEAR_ACCELERATION_NAME: &str = "線形加速度";

/// 停止中。
pub const IDLE_TEXT: &str = "停止中";
/// 開始済みでまだ値が無い。
pub const WAITING_TEXT: &str = "値を待っています";
/// OS に公開 API が無い。
pub const UNSUPPORTED_ON_OS_TEXT: &str = "この OS では取得できません";
/// API はあるが端末にセンサーが無い。
pub const NOT_PRESENT_TEXT: &str = "この端末にはセンサーがありません";
/// 権限拒否。
pub const PERMISSION_DENIED_TEXT: &str = "利用が許可されませんでした";
/// それ以外の失敗。
pub const FAILED_TEXT: &str = "取得できません";

/// センサーの種類（`fandhe_device::motion_raw_sensors::RawKind` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawKindView {
    /// 角速度。
    AngularVelocity,
    /// 磁場。
    MagneticField,
    /// 線形加速度。
    LinearAcceleration,
}

/// 値の単位（`RawUnit` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawUnitView {
    /// rad/s。
    RadPerSec,
    /// 度/秒。
    DegPerSec,
    /// マイクロテスラ。
    MicroTesla,
    /// g。
    StandardGravity,
    /// m/s²。
    MetersPerSec2,
}

impl RawUnitView {
    /// 種類に合う単位か。
    pub fn fits(self, kind: RawKindView) -> bool {
        match kind {
            RawKindView::AngularVelocity => {
                matches!(self, RawUnitView::RadPerSec | RawUnitView::DegPerSec)
            }
            RawKindView::MagneticField => self == RawUnitView::MicroTesla,
            RawKindView::LinearAcceleration => matches!(
                self,
                RawUnitView::StandardGravity | RawUnitView::MetersPerSec2
            ),
        }
    }

    fn label(self) -> &'static str {
        match self {
            RawUnitView::RadPerSec => "rad/s",
            RawUnitView::DegPerSec => "°/s",
            RawUnitView::MicroTesla => "µT",
            RawUnitView::StandardGravity => "g",
            RawUnitView::MetersPerSec2 => "m/s²",
        }
    }
}

/// 3 軸の値（`RawReading` の写し）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawReadingView {
    /// x 軸。
    pub x: f64,
    /// y 軸。
    pub y: f64,
    /// z 軸。
    pub z: f64,
    /// 単位。
    pub unit: RawUnitView,
}

/// 失敗の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawFailureView {
    /// OS に公開 API が無い。
    UnsupportedOnOs,
    /// 端末にセンサーが無い。
    NotPresent,
    /// 権限拒否。
    PermissionDenied,
    /// それ以外。
    Failed,
}

/// 1 行の表示状態。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RawSensorView {
    /// 停止中。
    #[default]
    Idle,
    /// 購読中でまだ値が無い。
    Waiting,
    /// 値を表示中。
    Live(RawReadingView),
    /// 失敗。
    Failed(RawFailureView),
}

/// 1 種類ぶんの状態。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RawRowState {
    /// 表示状態。
    pub view: RawSensorView,
    /// 購読を望んでいるか。driver が立ち上がり・立ち下がりで OS の購読を開始・停止する。
    pub wanted: bool,
    /// 「開始」のたびに増える世代。遅れて届いた完了を捨てるために使う。
    pub session: u64,
}

/// 画面の状態（3 種類の行）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MotionRawSensorsShowState {
    /// 角速度。
    pub angular_velocity: RawRowState,
    /// 磁場。
    pub magnetic_field: RawRowState,
    /// 線形加速度。
    pub linear_acceleration: RawRowState,
}

impl MotionRawSensorsShowState {
    /// 種類の行。
    pub fn row(&self, kind: RawKindView) -> &RawRowState {
        match kind {
            RawKindView::AngularVelocity => &self.angular_velocity,
            RawKindView::MagneticField => &self.magnetic_field,
            RawKindView::LinearAcceleration => &self.linear_acceleration,
        }
    }

    /// 種類の行（更新用）。
    pub fn row_mut(&mut self, kind: RawKindView) -> &mut RawRowState {
        match kind {
            RawKindView::AngularVelocity => &mut self.angular_velocity,
            RawKindView::MagneticField => &mut self.magnetic_field,
            RawKindView::LinearAcceleration => &mut self.linear_acceleration,
        }
    }

    fn rows_mut(&mut self) -> [&mut RawRowState; 3] {
        [
            &mut self.angular_velocity,
            &mut self.magnetic_field,
            &mut self.linear_acceleration,
        ]
    }

    fn any_wanted(&self) -> bool {
        self.angular_velocity.wanted
            || self.magnetic_field.wanted
            || self.linear_acceleration.wanted
    }
}

/// 「開始」。未購読の種類だけ世代を進める。冪等。
pub fn start_all(s: &mut MotionRawSensorsShowState) {
    for r in s.rows_mut() {
        if !r.wanted {
            r.wanted = true;
            r.view = RawSensorView::Waiting;
            r.session = r.session.wrapping_add(1);
        }
    }
}

/// 「停止」。冪等。失敗ではなく待機へ戻す。
pub fn stop_all(s: &mut MotionRawSensorsShowState) {
    for r in s.rows_mut() {
        r.wanted = false;
        r.view = RawSensorView::Idle;
    }
}

/// 読み取り結果を反映する。停止後・世代違い・単位違い・有限でない値は捨てる。`None` は値なし（待機へ戻す）。
pub fn apply_reading(
    s: &mut MotionRawSensorsShowState,
    kind: RawKindView,
    session: u64,
    reading: Option<RawReadingView>,
) {
    let r = s.row_mut(kind);
    if !r.wanted || r.session != session {
        return;
    }
    match reading {
        None => r.view = RawSensorView::Waiting,
        Some(v) if v.unit.fits(kind) && v.x.is_finite() && v.y.is_finite() && v.z.is_finite() => {
            r.view = RawSensorView::Live(v)
        }
        Some(_) => {}
    }
}

/// 失敗を反映する。購読中で世代が一致したときだけ確定し、その種類の希望を下ろす（再度「開始」できる）。
/// 他の種類には波及させない。
pub fn apply_failure(
    s: &mut MotionRawSensorsShowState,
    kind: RawKindView,
    session: u64,
    f: RawFailureView,
) {
    let r = s.row_mut(kind);
    if !r.wanted || r.session != session {
        return;
    }
    r.wanted = false;
    r.view = RawSensorView::Failed(f);
}

/// 値が表示上同じか（driver が送信を省く判定に使う）。単位と 3 軸の表示文字列で比べる。
pub fn display_equal(a: &RawReadingView, b: &RawReadingView) -> bool {
    a.unit == b.unit && axes_string(a) == axes_string(b)
}

// ponytail: 数値の書式は全単位で小数 3 桁に固定する。#885 の目視で見づらければ単位ごとに見直す。
fn axes_string(r: &RawReadingView) -> String {
    format!("x {:.3} / y {:.3} / z {:.3}", r.x, r.y, r.z)
}

fn name_of(kind: RawKindView) -> &'static str {
    match kind {
        RawKindView::AngularVelocity => ANGULAR_VELOCITY_NAME,
        RawKindView::MagneticField => MAGNETIC_FIELD_NAME,
        RawKindView::LinearAcceleration => LINEAR_ACCELERATION_NAME,
    }
}

fn failure_text(f: RawFailureView) -> &'static str {
    match f {
        RawFailureView::UnsupportedOnOs => UNSUPPORTED_ON_OS_TEXT,
        RawFailureView::NotPresent => NOT_PRESENT_TEXT,
        RawFailureView::PermissionDenied => PERMISSION_DENIED_TEXT,
        RawFailureView::Failed => FAILED_TEXT,
    }
}

/// 1 行の表示文。例「角速度: x 0.012 / y -0.003 / z 0.001 rad/s」。`Live` だけが数値を持つ。
pub fn row_text(s: &MotionRawSensorsShowState, kind: RawKindView) -> String {
    let body = match s.row(kind).view {
        RawSensorView::Idle => IDLE_TEXT.to_string(),
        RawSensorView::Waiting => WAITING_TEXT.to_string(),
        RawSensorView::Live(r) => format!("{} {}", axes_string(&r), r.unit.label()),
        RawSensorView::Failed(f) => failure_text(f).to_string(),
    };
    format!("{}: {body}", name_of(kind))
}

fn angular_velocity_text(s: &MotionRawSensorsShowState) -> String {
    row_text(s, RawKindView::AngularVelocity)
}

fn magnetic_field_text(s: &MotionRawSensorsShowState) -> String {
    row_text(s, RawKindView::MagneticField)
}

fn linear_acceleration_text(s: &MotionRawSensorsShowState) -> String {
    row_text(s, RawKindView::LinearAcceleration)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。いずれかの行が `wanted` なら「停止」、
/// そうでなければ「開始」を 1 つだけ出す。
pub fn current_screen(s: &MotionRawSensorsShowState) -> Screen<MotionRawSensorsShowState> {
    let screen = Screen::new("ジャイロ・磁気・線形加速度")
        .dynamic_text(angular_velocity_text)
        .dynamic_text(magnetic_field_text)
        .dynamic_text(linear_acceleration_text);
    if s.any_wanted() {
        screen.button(STOP_BUTTON_ID, STOP_LABEL, stop_all)
    } else {
        screen.button(START_BUTTON_ID, START_LABEL, start_all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RawKindView::*;

    const ALL: [RawKindView; 3] = [AngularVelocity, MagneticField, LinearAcceleration];

    fn started() -> MotionRawSensorsShowState {
        let mut s = MotionRawSensorsShowState::default();
        start_all(&mut s);
        s
    }

    fn rd(x: f64, unit: RawUnitView) -> RawReadingView {
        RawReadingView {
            x,
            y: -0.003,
            z: 0.001,
            unit,
        }
    }

    #[test]
    fn start_and_stop_are_idempotent() {
        let mut s = started();
        assert!(ALL.iter().all(|k| s.row(*k).session == 1));
        start_all(&mut s);
        assert!(ALL.iter().all(|k| s.row(*k).session == 1));
        stop_all(&mut s);
        stop_all(&mut s);
        assert!(ALL.iter().all(|k| s.row(*k).view == RawSensorView::Idle));
        assert!(!s.any_wanted());
    }

    #[test]
    fn stale_and_stopped_results_are_dropped() {
        let mut s = started();
        stop_all(&mut s);
        start_all(&mut s); // session 2
        let v = Some(rd(1.0, RawUnitView::RadPerSec));
        apply_reading(&mut s, AngularVelocity, 1, v);
        apply_failure(&mut s, AngularVelocity, 1, RawFailureView::Failed);
        assert_eq!(s.angular_velocity.view, RawSensorView::Waiting);
        stop_all(&mut s);
        apply_reading(&mut s, AngularVelocity, 2, v);
        apply_failure(&mut s, AngularVelocity, 2, RawFailureView::Failed);
        assert_eq!(s.angular_velocity.view, RawSensorView::Idle);
    }

    #[test]
    fn failure_does_not_spread_and_restart_bumps_session() {
        let mut s = started();
        apply_failure(&mut s, MagneticField, 1, RawFailureView::NotPresent);
        assert_eq!(
            s.magnetic_field.view,
            RawSensorView::Failed(RawFailureView::NotPresent)
        );
        assert!(!s.magnetic_field.wanted);
        assert!(s.angular_velocity.wanted && s.linear_acceleration.wanted);
        start_all(&mut s);
        assert_eq!(s.magnetic_field.session, 2);
        assert_eq!(s.angular_velocity.session, 1);
    }

    #[test]
    fn all_failure_kinds_have_distinct_text() {
        let texts = [
            failure_text(RawFailureView::UnsupportedOnOs),
            failure_text(RawFailureView::NotPresent),
            failure_text(RawFailureView::PermissionDenied),
            failure_text(RawFailureView::Failed),
        ];
        for (i, a) in texts.iter().enumerate() {
            for b in &texts[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn only_live_shows_numbers() {
        let mut s = started();
        for kind in ALL {
            for f in [
                RawFailureView::UnsupportedOnOs,
                RawFailureView::NotPresent,
                RawFailureView::PermissionDenied,
                RawFailureView::Failed,
            ] {
                s.row_mut(kind).view = RawSensorView::Failed(f);
                assert!(!row_text(&s, kind).chars().any(|c| c.is_ascii_digit()));
            }
            for v in [RawSensorView::Idle, RawSensorView::Waiting] {
                s.row_mut(kind).view = v;
                assert!(!row_text(&s, kind).chars().any(|c| c.is_ascii_digit()));
            }
        }
    }

    #[test]
    fn mismatched_unit_and_nan_are_dropped() {
        let mut s = started();
        let bad_unit = Some(rd(1.0, RawUnitView::MicroTesla));
        apply_reading(&mut s, AngularVelocity, 1, bad_unit);
        assert_eq!(s.angular_velocity.view, RawSensorView::Waiting);
        let nan = Some(rd(f64::NAN, RawUnitView::MicroTesla));
        apply_reading(&mut s, MagneticField, 1, nan);
        assert_eq!(s.magnetic_field.view, RawSensorView::Waiting);
    }

    #[test]
    fn live_format_and_none_returns_to_waiting() {
        let mut s = started();
        let v = Some(rd(0.0123, RawUnitView::RadPerSec));
        apply_reading(&mut s, AngularVelocity, 1, v);
        assert_eq!(
            row_text(&s, AngularVelocity),
            "角速度: x 0.012 / y -0.003 / z 0.001 rad/s"
        );
        apply_reading(&mut s, AngularVelocity, 1, None);
        assert_eq!(s.angular_velocity.view, RawSensorView::Waiting);
    }

    #[test]
    fn display_equal_compares_shown_values() {
        assert!(display_equal(
            &rd(0.0121, RawUnitView::RadPerSec),
            &rd(0.0124, RawUnitView::RadPerSec)
        ));
        assert!(!display_equal(
            &rd(0.012, RawUnitView::RadPerSec),
            &rd(0.012, RawUnitView::DegPerSec)
        ));
    }

    #[test]
    fn screen_builds_in_both_states() {
        let _ = current_screen(&MotionRawSensorsShowState::default());
        let _ = current_screen(&started());
    }
}
