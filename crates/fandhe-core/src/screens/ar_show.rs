//! Phase 11 の拡充ループ（親 #90）型サンプル「AR 表示」（Issue #307）。
//! 「開始」「停止」で AR セッションを始め・止め、追跡状態・開始地点からの位置・水平面の数を
//! 3 行で出す。カメラ映像は `ar-camera-view` の範囲で、同じ画面・同じセッションの映像を
//! 3 行の上に出す（契約は `fandhe_device::ar` のモジュール doc）。`FANDHE_PROXY_APP=ar_show` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod ar_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `device/ar.rs`（Web は `fandhe-web-dom`）が行い、[`apply_reading`] / [`apply_failure`] /
//! [`apply_ended`] で注入する。`fandhe_device::ar` の型は依存方向の都合でここへ写す。
//!
//! 嘘の値を出さない: 位置は `Live` かつ追跡できているときだけ、水平面は数えられる環境だけ数字で出す
//! （`None` は「数えません」で、0 枚とは出さない）。カメラ映像も `Live` の間だけ出す。古い世代の完了は [`ArShowState::session`] で捨てる。

use crate::{RgbaPixels, Screen, WidgetId};

/// 「開始」ボタン。Web は `Screen` を作り直さず両ボタンを常時出すため `pub`。
pub const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタン。
pub const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「開始」の文言。
pub const START_LABEL: &str = "開始";
/// 「停止」の文言。
pub const STOP_LABEL: &str = "停止";
/// 待機中の案内。
pub const IDLE_TEXT: &str = "開始を押すと AR の状態を表示します";
/// 開始中の案内。
pub const STARTING_TEXT: &str = "AR を開始しています…";
/// 非対応。
pub const UNSUPPORTED_TEXT: &str = "この環境では AR を使えません";
/// 権限拒否。
pub const PERMISSION_DENIED_TEXT: &str = "カメラの利用が許可されませんでした";
/// 値が届かない。
pub const NO_READING_TEXT: &str = "取得できません（AR の値が届きません）";
/// それ以外の失敗。
pub const FAILED_TEXT: &str = "取得できません";
/// 水平面を数えない環境（Web）の表示。
pub const PLANES_UNCOUNTED_TEXT: &str = "水平面: このブラウザでは数えません";
/// カメラ映像の代替テキスト。
pub const FRAME_ALT: &str = "AR のカメラ映像";
/// 注記（座標系）。
pub const NOTE_TEXT: &str = "位置は開始地点が原点（y 上向き・-z が前方、単位 m）";

/// 制限付きの理由（`fandhe_device::ar::LimitedReason` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitedReasonView {
    /// 初期化中。
    Initializing,
    /// 動きが速すぎる。
    ExcessiveMotion,
    /// 特徴点が足りない。
    InsufficientFeatures,
    /// 位置を合わせ直している。
    Relocalizing,
    /// その他。
    Other,
}

/// 追跡状態（`fandhe_device::ar::Tracking` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingView {
    /// 追跡できない。
    NotAvailable,
    /// 制限付き。
    Limited(LimitedReasonView),
    /// 良好。
    Normal,
}

/// AR の状態（`fandhe_device::ar::ArReading` の写し）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArReadingView {
    /// 追跡状態。
    pub tracking: TrackingView,
    /// 開始地点を原点とする位置（m）。無いときは `None`。
    pub position: Option<[f64; 3]>,
    /// 水平面の数。数えられない環境では `None`。
    pub horizontal_planes: Option<u32>,
}

/// 失敗の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArFailureView {
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
pub enum ArView {
    /// 停止中。
    #[default]
    Idle,
    /// 開始中でまだ値が無い。
    Starting,
    /// 表示中。
    Live(ArReadingView),
    /// 失敗。
    Failed(ArFailureView),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArShowState {
    /// 表示状態。
    pub view: ArView,
    /// セッションを望んでいるか。driver が立ち上がり・立ち下がりで OS のセッションを開始・停止する。
    pub wanted: bool,
    /// 「開始」のたびに増える世代。遅れて届いた完了を捨てるために使う。
    pub session: u64,
    /// 最新のカメラ映像。`Live` の間だけ持ち、開始・停止・失敗・値なしで捨てる。
    pub frame: Option<RgbaPixels>,
}

/// 「開始」。冪等（実行中の二重押下では世代を進めない）。
pub fn start(s: &mut ArShowState) {
    if s.wanted {
        return;
    }
    s.wanted = true;
    s.view = ArView::Starting;
    s.frame = None;
    s.session = s.session.wrapping_add(1);
}

/// 「停止」。冪等。失敗ではなく待機へ戻す。
pub fn stop(s: &mut ArShowState) {
    s.wanted = false;
    s.view = ArView::Idle;
    s.frame = None;
}

/// 読み取り結果を反映する。停止後・世代違いは捨てる。`None` は値なし（開始中へ戻す）。
pub fn apply_reading(s: &mut ArShowState, session: u64, r: Option<ArReadingView>) {
    if !s.wanted || s.session != session {
        return;
    }
    if r.is_none() {
        s.frame = None;
    }
    s.view = r.map_or(ArView::Starting, ArView::Live);
}

/// 映像を反映する。`Live` で世代が一致したときだけ受ける。`None` は直前の映像を消す。
pub fn apply_frame(s: &mut ArShowState, session: u64, frame: Option<RgbaPixels>) {
    if !s.wanted || s.session != session || !matches!(s.view, ArView::Live(_)) {
        return;
    }
    s.frame = frame;
}

/// 失敗を反映する。実行中で世代が一致したときだけ確定し、希望を下ろす（再度「開始」できる）。
pub fn apply_failure(s: &mut ArShowState, session: u64, f: ArFailureView) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = ArView::Failed(f);
    s.frame = None;
}

/// OS 側でセッションが終わった（Web の `end`＝戻る操作）。失敗にせず待機へ戻す。世代違いは捨てる。
pub fn apply_ended(s: &mut ArShowState, session: u64) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = ArView::Idle;
    s.frame = None;
}

fn limited_label(r: LimitedReasonView) -> &'static str {
    match r {
        LimitedReasonView::Initializing => "初期化中",
        LimitedReasonView::ExcessiveMotion => "動きが速すぎます",
        LimitedReasonView::InsufficientFeatures => "特徴点が足りません",
        LimitedReasonView::Relocalizing => "位置を合わせ直しています",
        LimitedReasonView::Other => "その他",
    }
}

/// 丸め後に 0 になる値を `-0.00` と出さない。
fn meters(v: f64) -> String {
    let t = format!("{v:.2}");
    if t.parse::<f64>() == Ok(0.0) {
        "0.00".to_string()
    } else {
        t
    }
}

/// `Live` の表示 3 行（追跡・位置・水平面）。driver の送信省略判定にも使う。
pub fn reading_lines(r: &ArReadingView) -> [String; 3] {
    let tracking = match r.tracking {
        TrackingView::Normal => "追跡: 良好".to_string(),
        TrackingView::Limited(why) => format!("追跡: 制限付き（{}）", limited_label(why)),
        TrackingView::NotAvailable => "追跡: 使えません".to_string(),
    };
    let position = match r.position {
        Some([x, y, z]) => format!(
            "位置: x {} m / y {} m / z {} m",
            meters(x),
            meters(y),
            meters(z)
        ),
        None => "位置: —".to_string(),
    };
    let planes = match r.horizontal_planes {
        Some(n) => format!("水平面: {n} 枚"),
        None => PLANES_UNCOUNTED_TEXT.to_string(),
    };
    [tracking, position, planes]
}

/// 主表示（1 行目）。
pub fn main_text(s: &ArShowState) -> String {
    match &s.view {
        ArView::Idle => IDLE_TEXT.to_string(),
        ArView::Starting => STARTING_TEXT.to_string(),
        ArView::Live(r) => {
            let [l, _, _] = reading_lines(r);
            l
        }
        ArView::Failed(ArFailureView::Unsupported) => UNSUPPORTED_TEXT.to_string(),
        ArView::Failed(ArFailureView::PermissionDenied) => PERMISSION_DENIED_TEXT.to_string(),
        ArView::Failed(ArFailureView::NoReading) => NO_READING_TEXT.to_string(),
        ArView::Failed(ArFailureView::Failed) => FAILED_TEXT.to_string(),
    }
}

/// 位置の行。`Live` 以外は注記だけ（数値を出さない）。
pub fn position_text(s: &ArShowState) -> String {
    match &s.view {
        ArView::Live(r) => reading_lines(r)[1].clone(),
        _ => NOTE_TEXT.to_string(),
    }
}

/// 水平面の行。`Live` 以外は空。
pub fn planes_text(s: &ArShowState) -> String {
    match &s.view {
        ArView::Live(r) => reading_lines(r)[2].clone(),
        _ => String::new(),
    }
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。`wanted` に応じてボタンを 1 つだけ出す。
pub fn current_screen(s: &ArShowState) -> Screen<ArShowState> {
    let mut screen = Screen::new("AR 表示");
    if let (ArView::Live(_), Some(pixels)) = (&s.view, &s.frame) {
        screen = screen.rgba_image(pixels.clone(), FRAME_ALT);
    }
    let screen = screen
        .dynamic_text(main_text)
        .dynamic_text(position_text)
        .dynamic_text(planes_text);
    if s.wanted {
        screen.button(STOP_BUTTON_ID, STOP_LABEL, stop)
    } else {
        screen.button(START_BUTTON_ID, START_LABEL, start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: TrackingView, p: Option<[f64; 3]>, n: Option<u32>) -> ArReadingView {
        ArReadingView {
            tracking: t,
            position: p,
            horizontal_planes: n,
        }
    }

    fn started() -> ArShowState {
        let mut s = ArShowState::default();
        start(&mut s);
        s
    }

    #[test]
    fn start_is_idempotent_and_stop_is_idle() {
        let mut s = started();
        start(&mut s);
        assert_eq!((s.view, s.wanted, s.session), (ArView::Starting, true, 1));
        stop(&mut s);
        assert_eq!((s.view, s.wanted), (ArView::Idle, false));
    }

    #[test]
    fn failure_allows_restart_with_new_session() {
        let mut s = started();
        apply_failure(&mut s, 1, ArFailureView::PermissionDenied);
        assert!(!s.wanted);
        assert_eq!(main_text(&s), PERMISSION_DENIED_TEXT);
        start(&mut s);
        assert_eq!((s.view, s.session), (ArView::Starting, 2));
    }

    #[test]
    fn stale_and_stopped_results_are_dropped() {
        let mut s = started();
        stop(&mut s);
        start(&mut s); // session 2
        let live = r(TrackingView::Normal, Some([0.0; 3]), Some(1));
        apply_reading(&mut s, 1, Some(live));
        apply_failure(&mut s, 1, ArFailureView::Failed);
        apply_ended(&mut s, 1);
        assert_eq!((s.view, s.wanted), (ArView::Starting, true));
        stop(&mut s);
        apply_reading(&mut s, 2, Some(live));
        apply_failure(&mut s, 2, ArFailureView::Failed);
        assert_eq!(s.view, ArView::Idle);
    }

    #[test]
    fn ended_returns_to_idle_not_failed() {
        let mut s = started();
        apply_ended(&mut s, 1);
        assert_eq!((s.view, s.wanted), (ArView::Idle, false));
    }

    #[test]
    fn none_reading_returns_to_starting() {
        let mut s = started();
        apply_reading(&mut s, 1, Some(r(TrackingView::Normal, None, None)));
        apply_reading(&mut s, 1, None);
        assert_eq!(s.view, ArView::Starting);
    }

    #[test]
    fn only_live_shows_numbers() {
        let mut s = started();
        for f in [
            ArFailureView::Unsupported,
            ArFailureView::PermissionDenied,
            ArFailureView::NoReading,
            ArFailureView::Failed,
        ] {
            s.view = ArView::Failed(f);
            assert!(!position_text(&s).contains("x "));
            assert_eq!(planes_text(&s), "");
        }
        s.view = ArView::Live(r(TrackingView::Normal, Some([0.12, -0.03, -1.2]), Some(2)));
        assert_eq!(main_text(&s), "追跡: 良好");
        assert_eq!(position_text(&s), "位置: x 0.12 m / y -0.03 m / z -1.20 m");
        assert_eq!(planes_text(&s), "水平面: 2 枚");
    }

    fn px() -> RgbaPixels {
        RgbaPixels::new(1, 1, vec![0; 4]).unwrap()
    }

    fn live_state() -> ArShowState {
        let mut s = started();
        apply_reading(&mut s, 1, Some(r(TrackingView::Normal, None, None)));
        s
    }

    #[test]
    fn frame_only_in_live_same_session() {
        let mut s = started();
        apply_frame(&mut s, 1, Some(px()));
        assert!(s.frame.is_none());
        let mut s = live_state();
        apply_frame(&mut s, 2, Some(px()));
        assert!(s.frame.is_none());
        apply_frame(&mut s, 1, Some(px()));
        assert!(s.frame.is_some());
        apply_frame(&mut s, 1, None);
        assert!(s.frame.is_none());
        let mut s = live_state();
        s.wanted = false;
        apply_frame(&mut s, 1, Some(px()));
        assert!(s.frame.is_none());
    }

    #[test]
    fn frame_cleared_on_every_exit() {
        let with = || {
            let mut s = live_state();
            apply_frame(&mut s, 1, Some(px()));
            s
        };
        let mut s = with();
        stop(&mut s);
        assert!(s.frame.is_none());
        for f in [
            ArFailureView::PermissionDenied,
            ArFailureView::Unsupported,
            ArFailureView::NoReading,
            ArFailureView::Failed,
        ] {
            let mut s = with();
            apply_failure(&mut s, 1, f);
            assert!(s.frame.is_none());
        }
        let mut s = with();
        apply_ended(&mut s, 1);
        assert!(s.frame.is_none());
        let mut s = with();
        apply_reading(&mut s, 1, None);
        assert!(s.frame.is_none());
        let mut s = with();
        s.wanted = false;
        start(&mut s);
        assert!(s.frame.is_none());
    }

    #[test]
    fn image_only_when_live_with_frame_and_above_lines() {
        let count = |s: &ArShowState| {
            current_screen(s)
                .iter()
                .filter(|n| matches!(n, crate::Node::RgbaImage { .. }))
                .count()
        };
        let mut s = live_state();
        assert_eq!(count(&s), 0);
        apply_frame(&mut s, 1, Some(px()));
        assert_eq!(count(&s), 1);
        assert!(matches!(
            current_screen(&s).iter().next(),
            Some(crate::Node::RgbaImage { .. })
        ));
        for v in [
            ArView::Idle,
            ArView::Starting,
            ArView::Failed(ArFailureView::Failed),
        ] {
            s.view = v;
            assert_eq!(count(&s), 0);
        }
    }

    #[test]
    fn not_available_has_no_position_and_none_planes_is_not_zero() {
        let l = reading_lines(&r(TrackingView::NotAvailable, None, None));
        assert_eq!(l[0], "追跡: 使えません");
        assert_eq!(l[1], "位置: —");
        assert_eq!(l[2], PLANES_UNCOUNTED_TEXT);
        assert!(!l[2].contains("0 枚"));
    }

    #[test]
    fn no_negative_zero_and_limited_reason() {
        let l = reading_lines(&r(
            TrackingView::Limited(LimitedReasonView::ExcessiveMotion),
            Some([-0.001, 0.0, -0.0]),
            Some(0),
        ));
        assert_eq!(l[0], "追跡: 制限付き（動きが速すぎます）");
        assert_eq!(l[1], "位置: x 0.00 m / y 0.00 m / z 0.00 m");
        assert_eq!(l[2], "水平面: 0 枚");
    }
}
