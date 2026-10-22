//! Phase 11 の拡充ループ（親 #123）型サンプル「気圧・照度・近接」（Issue #609）。
//! 気圧・照度の組と近接の組、それぞれ「開始」「停止」で購読を始め・止め、3 行に現在値を出す。
//! `FANDHE_PROXY_APP=environment_sensors_show` で選ばれ、`fandhe-egui-proxy` の `screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod environment_sensors_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `device/environment_sensors.rs` が行い、[`apply_reading`] / [`apply_failure`] で注入する。
//! `fandhe_device::environment_sensors` の型は依存方向の都合でここへ写す。
//!
//! 嘘の値を出さない: 待機・取得中・失敗は数値を持たない（[`SensorView::Live`] だけが持つ）。
//! 古い世代の完了は種類ごとの [`SensorRowState::session`] で捨てる。1 つの種類の失敗は他の種類へ波及させない。
//! 近接を別ボタンにするのは、iOS で画面が消え、Android で画面下のセンサーが点滅する機種があるため。

use crate::{Screen, WidgetId};

/// 気圧・照度の「開始」ボタン。
pub const AMBIENT_START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 気圧・照度の「停止」ボタン。
pub const AMBIENT_STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 近接の「開始」ボタン。
pub const PROXIMITY_START_BUTTON_ID: WidgetId = WidgetId::from_raw(3);
/// 近接の「停止」ボタン。
pub const PROXIMITY_STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(4);
/// 気圧・照度の「開始」の文言。
pub const AMBIENT_START_LABEL: &str = "気圧・照度を開始";
/// 気圧・照度の「停止」の文言。
pub const AMBIENT_STOP_LABEL: &str = "気圧・照度を停止";
/// 近接の「開始」の文言。
pub const PROXIMITY_START_LABEL: &str = "近接を開始";
/// 近接の「停止」の文言。
pub const PROXIMITY_STOP_LABEL: &str = "近接を停止";

/// 行名: 気圧。
pub const PRESSURE_NAME: &str = "気圧";
/// 行名: 照度。
pub const LIGHT_NAME: &str = "照度";
/// 行名: 近接。
pub const PROXIMITY_NAME: &str = "近接";

/// 停止中。
pub const IDLE_TEXT: &str = "停止中";
/// 取得中。
pub const WAITING_TEXT: &str = "取得中…";
/// OS に公開 API が無い。
pub const UNSUPPORTED_ON_OS_TEXT: &str = "この OS では取得できません";
/// API はあるが端末にセンサーが無い。
pub const NOT_PRESENT_TEXT: &str = "この端末にはセンサーがありません";
/// 権限拒否。
pub const PERMISSION_DENIED_TEXT: &str = "利用が許可されませんでした";
/// 値が届かない。
pub const NO_READING_TEXT: &str = "取得できません（値が届きません）";
/// それ以外の失敗。
pub const FAILED_TEXT: &str = "取得できません";
/// 近接: 近い。
pub const NEAR_TEXT: &str = "近い";
/// 近接: 遠い。
pub const FAR_TEXT: &str = "遠い";
/// 近接の注意書き。
pub const PROXIMITY_NOTE: &str =
    "近接を開始すると、端末によって画面が消えたりセンサーが点滅したりします";

/// センサーの種類（`fandhe_device::environment_sensors::SensorKind` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorKindView {
    /// 気圧。
    Pressure,
    /// 照度。
    Light,
    /// 近接。
    Proximity,
}

/// 近接の 2 値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProximityView {
    /// 近い。
    Near,
    /// 遠い。
    Far,
}

/// センサー値（`fandhe_device::environment_sensors::EnvReading` の写し）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReadingView {
    /// 気圧（hPa）。
    PressureHpa(f64),
    /// 照度（lx）。
    IlluminanceLux(f64),
    /// 近接。
    Proximity(ProximityView),
}

impl ReadingView {
    /// この値の種類。
    pub fn kind(&self) -> SensorKindView {
        match self {
            ReadingView::PressureHpa(_) => SensorKindView::Pressure,
            ReadingView::IlluminanceLux(_) => SensorKindView::Light,
            ReadingView::Proximity(_) => SensorKindView::Proximity,
        }
    }
}

/// 失敗の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorFailureView {
    /// OS に公開 API が無い。
    UnsupportedOnOs,
    /// 端末にセンサーが無い。
    NotPresent,
    /// 権限拒否。
    PermissionDenied,
    /// 期限内に値が届かない。
    NoReading,
    /// それ以外。
    Failed,
}

/// 1 行の表示状態。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SensorView {
    /// 停止中。
    #[default]
    Idle,
    /// 購読中でまだ値が無い。
    Waiting,
    /// 値を表示中。
    Live(ReadingView),
    /// 失敗。
    Failed(SensorFailureView),
}

/// 1 種類ぶんの状態。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SensorRowState {
    /// 表示状態。
    pub view: SensorView,
    /// 購読を望んでいるか。driver が立ち上がり・立ち下がりで OS の購読を開始・停止する。
    pub wanted: bool,
    /// 「開始」のたびに増える世代。遅れて届いた完了を捨てるために使う。
    pub session: u64,
}

/// 画面の状態（3 種類の行）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnvironmentSensorsShowState {
    /// 気圧。
    pub pressure: SensorRowState,
    /// 照度。
    pub light: SensorRowState,
    /// 近接。
    pub proximity: SensorRowState,
}

impl EnvironmentSensorsShowState {
    /// 種類の行。
    pub fn row(&self, kind: SensorKindView) -> &SensorRowState {
        match kind {
            SensorKindView::Pressure => &self.pressure,
            SensorKindView::Light => &self.light,
            SensorKindView::Proximity => &self.proximity,
        }
    }

    /// 種類の行（更新用）。
    pub fn row_mut(&mut self, kind: SensorKindView) -> &mut SensorRowState {
        match kind {
            SensorKindView::Pressure => &mut self.pressure,
            SensorKindView::Light => &mut self.light,
            SensorKindView::Proximity => &mut self.proximity,
        }
    }
}

fn start_row(r: &mut SensorRowState) {
    if r.wanted {
        return;
    }
    r.wanted = true;
    r.view = SensorView::Waiting;
    r.session = r.session.wrapping_add(1);
}

fn stop_row(r: &mut SensorRowState) {
    r.wanted = false;
    r.view = SensorView::Idle;
}

/// 気圧・照度の「開始」。未購読の種類だけ世代を進める。冪等。
pub fn start_ambient(s: &mut EnvironmentSensorsShowState) {
    start_row(&mut s.pressure);
    start_row(&mut s.light);
}

/// 気圧・照度の「停止」。冪等。失敗ではなく待機へ戻す。
pub fn stop_ambient(s: &mut EnvironmentSensorsShowState) {
    stop_row(&mut s.pressure);
    stop_row(&mut s.light);
}

/// 近接の「開始」。冪等。
pub fn start_proximity(s: &mut EnvironmentSensorsShowState) {
    start_row(&mut s.proximity);
}

/// 近接の「停止」。冪等。
pub fn stop_proximity(s: &mut EnvironmentSensorsShowState) {
    stop_row(&mut s.proximity);
}

/// 読み取り結果を反映する。停止後・世代違い・種類違いの値は捨てる。`None` は値なし（取得中へ戻す）。
pub fn apply_reading(
    s: &mut EnvironmentSensorsShowState,
    kind: SensorKindView,
    session: u64,
    reading: Option<ReadingView>,
) {
    let r = s.row_mut(kind);
    if !r.wanted || r.session != session {
        return;
    }
    match reading {
        None => r.view = SensorView::Waiting,
        Some(v) if v.kind() == kind => r.view = SensorView::Live(v),
        Some(_) => {}
    }
}

/// 失敗を反映する。購読中で世代が一致したときだけ確定し、その種類の希望を下ろす（再度「開始」できる）。
/// 他の種類には波及させない。
pub fn apply_failure(
    s: &mut EnvironmentSensorsShowState,
    kind: SensorKindView,
    session: u64,
    f: SensorFailureView,
) {
    let r = s.row_mut(kind);
    if !r.wanted || r.session != session {
        return;
    }
    r.wanted = false;
    r.view = SensorView::Failed(f);
}

/// 値が表示上同じか（driver が送信を省く判定に使う）。気圧は小数 1 桁、照度は整数で比べる。
pub fn display_equal(a: &ReadingView, b: &ReadingView) -> bool {
    match (a, b) {
        (ReadingView::PressureHpa(x), ReadingView::PressureHpa(y)) => {
            pressure_string(*x) == pressure_string(*y)
        }
        (ReadingView::IlluminanceLux(x), ReadingView::IlluminanceLux(y)) => {
            lux_string(*x) == lux_string(*y)
        }
        (ReadingView::Proximity(x), ReadingView::Proximity(y)) => x == y,
        _ => false,
    }
}

fn pressure_string(hpa: f64) -> String {
    format!("{hpa:.1}")
}

fn lux_string(lux: f64) -> String {
    format!("{}", lux.round() as i64)
}

fn name_of(kind: SensorKindView) -> &'static str {
    match kind {
        SensorKindView::Pressure => PRESSURE_NAME,
        SensorKindView::Light => LIGHT_NAME,
        SensorKindView::Proximity => PROXIMITY_NAME,
    }
}

fn failure_text(f: SensorFailureView) -> &'static str {
    match f {
        SensorFailureView::UnsupportedOnOs => UNSUPPORTED_ON_OS_TEXT,
        SensorFailureView::NotPresent => NOT_PRESENT_TEXT,
        SensorFailureView::PermissionDenied => PERMISSION_DENIED_TEXT,
        SensorFailureView::NoReading => NO_READING_TEXT,
        SensorFailureView::Failed => FAILED_TEXT,
    }
}

/// 1 行の表示文（`DynamicText` の `get` 相当）。「気圧: 1013.2 hPa」「照度: 350 lx」「近接: 近い」。
/// `Live` だけが数値・値を持つ。
pub fn row_text(s: &EnvironmentSensorsShowState, kind: SensorKindView) -> String {
    let body = match s.row(kind).view {
        SensorView::Idle => IDLE_TEXT.to_string(),
        SensorView::Waiting => WAITING_TEXT.to_string(),
        SensorView::Live(ReadingView::PressureHpa(v)) => format!("{} hPa", pressure_string(v)),
        SensorView::Live(ReadingView::IlluminanceLux(v)) => format!("{} lx", lux_string(v)),
        SensorView::Live(ReadingView::Proximity(ProximityView::Near)) => NEAR_TEXT.to_string(),
        SensorView::Live(ReadingView::Proximity(ProximityView::Far)) => FAR_TEXT.to_string(),
        SensorView::Failed(f) => failure_text(f).to_string(),
    };
    format!("{}: {body}", name_of(kind))
}

fn pressure_text(s: &EnvironmentSensorsShowState) -> String {
    row_text(s, SensorKindView::Pressure)
}

fn light_text(s: &EnvironmentSensorsShowState) -> String {
    row_text(s, SensorKindView::Light)
}

fn proximity_text(s: &EnvironmentSensorsShowState) -> String {
    row_text(s, SensorKindView::Proximity)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。気圧・照度の組は片方でも `wanted` なら「停止」、
/// 近接の組は近接の `wanted` で「開始」「停止」を 1 つだけ出す。
pub fn current_screen(s: &EnvironmentSensorsShowState) -> Screen<EnvironmentSensorsShowState> {
    let screen = Screen::new("気圧・照度・近接")
        .dynamic_text(pressure_text)
        .dynamic_text(light_text)
        .dynamic_text(proximity_text)
        .text(PROXIMITY_NOTE);
    let screen = if s.pressure.wanted || s.light.wanted {
        screen.button(AMBIENT_STOP_BUTTON_ID, AMBIENT_STOP_LABEL, stop_ambient)
    } else {
        screen.button(AMBIENT_START_BUTTON_ID, AMBIENT_START_LABEL, start_ambient)
    };
    if s.proximity.wanted {
        screen.button(
            PROXIMITY_STOP_BUTTON_ID,
            PROXIMITY_STOP_LABEL,
            stop_proximity,
        )
    } else {
        screen.button(
            PROXIMITY_START_BUTTON_ID,
            PROXIMITY_START_LABEL,
            start_proximity,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SensorKindView::*;

    const ALL: [SensorKindView; 3] = [Pressure, Light, Proximity];

    fn ambient_started() -> EnvironmentSensorsShowState {
        let mut s = EnvironmentSensorsShowState::default();
        start_ambient(&mut s);
        s
    }

    #[test]
    fn start_is_idempotent_and_bumps_session_per_kind() {
        let mut s = ambient_started();
        assert_eq!((s.pressure.session, s.light.session), (1, 1));
        assert_eq!(s.pressure.view, SensorView::Waiting);
        start_ambient(&mut s);
        assert_eq!((s.pressure.session, s.light.session), (1, 1));
        // 片方だけ失敗した後の再開始は、未購読の種類だけ世代が進む。
        apply_failure(&mut s, Light, 1, SensorFailureView::Failed);
        start_ambient(&mut s);
        assert_eq!((s.pressure.session, s.light.session), (1, 2));
    }

    #[test]
    fn proximity_is_independent_of_ambient() {
        let mut s = ambient_started();
        assert!(!s.proximity.wanted);
        start_proximity(&mut s);
        stop_ambient(&mut s);
        assert!(s.proximity.wanted);
        assert_eq!(s.proximity.view, SensorView::Waiting);
        assert_eq!(s.pressure.view, SensorView::Idle);
        stop_proximity(&mut s);
        assert_eq!(s.proximity.view, SensorView::Idle);
    }

    #[test]
    fn failure_does_not_spread() {
        let mut s = ambient_started();
        start_proximity(&mut s);
        apply_failure(&mut s, Pressure, 1, SensorFailureView::NotPresent);
        assert_eq!(
            s.pressure.view,
            SensorView::Failed(SensorFailureView::NotPresent)
        );
        assert!(!s.pressure.wanted);
        assert!(s.light.wanted && s.proximity.wanted);
        assert_eq!(s.light.view, SensorView::Waiting);
    }

    #[test]
    fn stale_stopped_and_mismatched_results_are_dropped() {
        let mut s = ambient_started();
        stop_ambient(&mut s);
        start_ambient(&mut s); // session 2
        apply_reading(&mut s, Pressure, 1, Some(ReadingView::PressureHpa(1000.0)));
        apply_failure(&mut s, Pressure, 1, SensorFailureView::Failed);
        assert_eq!(s.pressure.view, SensorView::Waiting);
        // 種類違いの値。
        apply_reading(&mut s, Pressure, 2, Some(ReadingView::IlluminanceLux(10.0)));
        assert_eq!(s.pressure.view, SensorView::Waiting);
        stop_ambient(&mut s);
        apply_reading(&mut s, Pressure, 2, Some(ReadingView::PressureHpa(1000.0)));
        apply_failure(&mut s, Pressure, 2, SensorFailureView::Failed);
        assert_eq!(s.pressure.view, SensorView::Idle);
    }

    #[test]
    fn none_reading_returns_to_waiting() {
        let mut s = ambient_started();
        apply_reading(&mut s, Light, 1, Some(ReadingView::IlluminanceLux(350.0)));
        assert_eq!(row_text(&s, Light), "照度: 350 lx");
        apply_reading(&mut s, Light, 1, None);
        assert_eq!(s.light.view, SensorView::Waiting);
    }

    #[test]
    fn buttons_follow_wanted() {
        // 片方失敗中でも、もう片方が購読中なら「停止」のまま。両方失敗で「開始」に戻る。
        let mut s = ambient_started();
        apply_failure(&mut s, Pressure, 1, SensorFailureView::Failed);
        assert!(s.pressure.wanted || s.light.wanted);
        apply_failure(&mut s, Light, 1, SensorFailureView::Failed);
        assert!(!(s.pressure.wanted || s.light.wanted));
        // 画面生成が panic しない（両分岐）。
        let _ = current_screen(&s);
        let _ = current_screen(&ambient_started());
        let mut p = EnvironmentSensorsShowState::default();
        start_proximity(&mut p);
        let _ = current_screen(&p);
    }

    #[test]
    fn only_live_shows_numbers() {
        let mut s = ambient_started();
        for kind in ALL {
            for f in [
                SensorFailureView::UnsupportedOnOs,
                SensorFailureView::NotPresent,
                SensorFailureView::PermissionDenied,
                SensorFailureView::NoReading,
                SensorFailureView::Failed,
            ] {
                s.row_mut(kind).view = SensorView::Failed(f);
                assert!(!row_text(&s, kind).chars().any(|c| c.is_ascii_digit()));
            }
            for v in [SensorView::Idle, SensorView::Waiting] {
                s.row_mut(kind).view = v;
                assert!(!row_text(&s, kind).chars().any(|c| c.is_ascii_digit()));
            }
        }
    }

    #[test]
    fn live_formats() {
        let mut s = ambient_started();
        s.pressure.view = SensorView::Live(ReadingView::PressureHpa(1013.24));
        assert_eq!(row_text(&s, Pressure), "気圧: 1013.2 hPa");
        s.light.view = SensorView::Live(ReadingView::IlluminanceLux(349.6));
        assert_eq!(row_text(&s, Light), "照度: 350 lx");
        s.proximity.view = SensorView::Live(ReadingView::Proximity(ProximityView::Near));
        assert_eq!(row_text(&s, Proximity), "近接: 近い");
        s.proximity.view = SensorView::Live(ReadingView::Proximity(ProximityView::Far));
        assert_eq!(row_text(&s, Proximity), "近接: 遠い");
    }

    #[test]
    fn unsupported_on_os_and_not_present_differ() {
        assert_ne!(UNSUPPORTED_ON_OS_TEXT, NOT_PRESENT_TEXT);
        assert_ne!(
            failure_text(SensorFailureView::UnsupportedOnOs),
            failure_text(SensorFailureView::NotPresent)
        );
    }

    #[test]
    fn display_equal_compares_shown_values() {
        use ReadingView::*;
        assert!(display_equal(&PressureHpa(1013.21), &PressureHpa(1013.24)));
        assert!(!display_equal(&PressureHpa(1013.2), &PressureHpa(1013.3)));
        assert!(display_equal(
            &IlluminanceLux(350.2),
            &IlluminanceLux(349.9)
        ));
        assert!(!display_equal(&PressureHpa(1.0), &IlluminanceLux(1.0)));
    }
}
