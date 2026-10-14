//! Phase 11 の拡充ループ（親 #1154）型サンプル「方位（コンパス）」（Issue #1404）。
//! 「開始」「停止」で購読を始め・止め、現在の方位（整数の度）と 8 方位の文字を 1 画面に出す。
//! `FANDHE_PROXY_APP=compass_show` で選ばれ、`fandhe-egui-proxy` の `screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod compass_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `device/compass.rs`（Web は `fandhe-web-dom`）が行い、[`apply_reading`] / [`apply_failure`] で
//! 注入する。`fandhe_device::compass` の型と、度の丸め・8 方位の判定は依存方向の都合でここへ写す。
//!
//! 嘘の方位を出さない: 初期・待機・取得中・失敗・非対応は度を持たない（[`CompassView::Live`] だけが持つ）。
//! 古い世代の完了は [`CompassShowState::session`] で捨てる。

use crate::{Screen, WidgetId};

/// 「開始」ボタン。Web は `Screen` を作り直さず両ボタンを常時出すため `pub`。
pub const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタン。
pub const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「開始」の文言。
pub const START_LABEL: &str = "開始";
/// 「停止」の文言。
pub const STOP_LABEL: &str = "停止";
/// 待機中の案内。
pub const IDLE_TEXT: &str = "開始を押すと方位を表示します";
/// 取得中の案内。
pub const WAITING_TEXT: &str = "方位を取得中…";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境では方位を取得できません";
/// 権限拒否。
pub const PERMISSION_DENIED_TEXT: &str = "方位の利用が許可されませんでした";
/// 値が届かない。
pub const NO_READING_TEXT: &str = "取得できません（方位の値が届きません）";
/// それ以外の失敗。
pub const FAILED_TEXT: &str = "取得できません";
/// 注記（基準）。
pub const NOTE_TEXT: &str = "端末を水平に持った縦向きの方位（磁北）";

/// 精度（`fandhe_device::compass::Accuracy` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccuracyView {
    /// 不明。
    Unknown,
    /// 低い。
    Unreliable,
    /// 中。
    Approximate,
    /// 高い。
    High,
}

/// 方位（`fandhe_device::compass::Heading` の写し）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadingView {
    /// 磁北から時計回りの度。`[0.0, 360.0)`。
    pub degrees: f64,
    /// 精度。
    pub accuracy: AccuracyView,
}

/// 失敗の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompassFailureView {
    /// 非対応。
    Unsupported,
    /// 権限拒否。
    PermissionDenied,
    /// 期限内に値が届かない。
    NoReading,
    /// それ以外。
    Failed,
}

/// 表示状態。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CompassView {
    /// 停止中。
    #[default]
    Idle,
    /// 購読中でまだ値が無い。
    Waiting,
    /// 方位を表示中。
    Live(HeadingView),
    /// 失敗。
    Failed(CompassFailureView),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompassShowState {
    /// 表示状態。
    pub view: CompassView,
    /// 購読を望んでいるか。driver が立ち上がり・立ち下がりで OS の購読を開始・停止する。
    pub wanted: bool,
    /// 「開始」のたびに増える世代。遅れて届いた完了を捨てるために使う。
    pub session: u64,
}

/// 「開始」。冪等（購読中の二重押下では世代を進めない）。
pub fn start(s: &mut CompassShowState) {
    if s.wanted {
        return;
    }
    s.wanted = true;
    s.view = CompassView::Waiting;
    s.session = s.session.wrapping_add(1);
}

/// 「停止」。冪等。失敗ではなく待機へ戻す。
pub fn stop(s: &mut CompassShowState) {
    s.wanted = false;
    s.view = CompassView::Idle;
}

/// 読み取り結果を反映する。停止後・世代違いは捨てる。`None` は値なし（取得中へ戻す）。
pub fn apply_reading(s: &mut CompassShowState, session: u64, h: Option<HeadingView>) {
    if !s.wanted || s.session != session {
        return;
    }
    s.view = h.map_or(CompassView::Waiting, CompassView::Live);
}

/// 失敗を反映する。購読中で世代が一致したときだけ確定し、購読の希望を下ろす（再度「開始」できる）。
pub fn apply_failure(s: &mut CompassShowState, session: u64, f: CompassFailureView) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = CompassView::Failed(f);
}

/// 表示用の整数度（`fandhe_device::compass::display_degrees` の写し）。359.6 は 0。
pub fn display_degrees(degrees: f64) -> u16 {
    (degrees.round() as i64).rem_euclid(360) as u16
}

/// 8 方位の日本語表記（`fandhe_device::compass::compass_point` の写し）。
pub fn point_label(degrees: f64) -> &'static str {
    const ALL: [&str; 8] = ["北", "北東", "東", "南東", "南", "南西", "西", "北西"];
    ALL[(((degrees + 22.5) / 45.0).floor() as i64).rem_euclid(8) as usize]
}

fn accuracy_label(a: AccuracyView) -> &'static str {
    match a {
        AccuracyView::High => "高",
        AccuracyView::Approximate => "中",
        AccuracyView::Unreliable => "低",
        AccuracyView::Unknown => "不明",
    }
}

/// 主表示（`DynamicText` の `get`）。
pub fn heading_text(s: &CompassShowState) -> String {
    match s.view {
        CompassView::Idle => IDLE_TEXT.to_string(),
        CompassView::Waiting => WAITING_TEXT.to_string(),
        CompassView::Live(h) => format!(
            "方位 {}°（{}）",
            display_degrees(h.degrees),
            point_label(h.degrees)
        ),
        CompassView::Failed(CompassFailureView::Unsupported) => UNSUPPORTED_TEXT.to_string(),
        CompassView::Failed(CompassFailureView::PermissionDenied) => {
            PERMISSION_DENIED_TEXT.to_string()
        }
        CompassView::Failed(CompassFailureView::NoReading) => NO_READING_TEXT.to_string(),
        CompassView::Failed(CompassFailureView::Failed) => FAILED_TEXT.to_string(),
    }
}

/// 基準と精度の行。値が無いときは基準だけ。
pub fn detail_text(s: &CompassShowState) -> String {
    match s.view {
        CompassView::Live(h) => format!("磁北基準 / 精度: {}", accuracy_label(h.accuracy)),
        _ => NOTE_TEXT.to_string(),
    }
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。`wanted` に応じてボタンを 1 つだけ出す。
pub fn current_screen(s: &CompassShowState) -> Screen<CompassShowState> {
    let screen = Screen::new("方位")
        .dynamic_text(heading_text)
        .dynamic_text(detail_text);
    if s.wanted {
        screen.button(STOP_BUTTON_ID, STOP_LABEL, stop)
    } else {
        screen.button(START_BUTTON_ID, START_LABEL, start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(d: f64) -> HeadingView {
        HeadingView {
            degrees: d,
            accuracy: AccuracyView::High,
        }
    }

    fn started() -> CompassShowState {
        let mut s = CompassShowState::default();
        start(&mut s);
        s
    }

    #[test]
    fn start_waits_and_bumps_session_once() {
        let mut s = started();
        assert_eq!(
            (s.view, s.wanted, s.session),
            (CompassView::Waiting, true, 1)
        );
        start(&mut s);
        assert_eq!(s.session, 1);
        stop(&mut s);
        assert_eq!((s.view, s.wanted), (CompassView::Idle, false));
    }

    #[test]
    fn failure_resets_wanted_and_allows_restart() {
        let mut s = started();
        apply_failure(&mut s, 1, CompassFailureView::PermissionDenied);
        assert!(!s.wanted);
        assert_eq!(heading_text(&s), PERMISSION_DENIED_TEXT);
        start(&mut s);
        assert_eq!((s.view, s.session), (CompassView::Waiting, 2));
    }

    #[test]
    fn stale_session_and_stopped_results_are_dropped() {
        let mut s = started();
        stop(&mut s);
        start(&mut s); // session 2
        apply_reading(&mut s, 1, Some(h(10.0)));
        apply_failure(&mut s, 1, CompassFailureView::Failed);
        assert_eq!((s.view, s.wanted), (CompassView::Waiting, true));
        stop(&mut s);
        apply_reading(&mut s, 2, Some(h(10.0)));
        apply_failure(&mut s, 2, CompassFailureView::Failed);
        assert_eq!(s.view, CompassView::Idle);
    }

    #[test]
    fn none_reading_returns_to_waiting() {
        let mut s = started();
        apply_reading(&mut s, 1, Some(h(10.0)));
        apply_reading(&mut s, 1, None);
        assert_eq!(s.view, CompassView::Waiting);
    }

    #[test]
    fn only_live_shows_degrees() {
        let mut s = started();
        for f in [
            CompassFailureView::Unsupported,
            CompassFailureView::PermissionDenied,
            CompassFailureView::NoReading,
            CompassFailureView::Failed,
        ] {
            s.view = CompassView::Failed(f);
            assert!(!heading_text(&s).contains('°'));
            assert!(!detail_text(&s).contains('°'));
        }
        s.view = CompassView::Idle;
        assert!(!heading_text(&s).contains('°'));
        s.view = CompassView::Live(h(135.0));
        assert_eq!(heading_text(&s), "方位 135°（南東）");
        assert_eq!(detail_text(&s), "磁北基準 / 精度: 高");
    }

    #[test]
    fn never_shows_360() {
        let mut s = started();
        apply_reading(&mut s, 1, Some(h(359.6)));
        assert_eq!(heading_text(&s), "方位 0°（北）");
    }

    #[test]
    fn point_labels_cover_boundaries() {
        assert_eq!(point_label(22.5), "北東");
        assert_eq!(point_label(337.5), "北");
        assert_eq!(point_label(359.9), "北");
        assert_eq!(point_label(247.5), "西");
    }
}
