//! Phase 11 の拡充ループ（親 #1056）型サンプル「ジオフェンス」（Issue #1057）。
//! `FANDHE_PROXY_APP=geofence_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod geofence_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。判定器 `GeofenceTracker` は
//! `fandhe-device` 側の型のため、本モジュールは持たない。effect ドライバ
//! （`fandhe-egui-proxy` の `device/geofence.rs`）が判定器を保持し、結果を
//! 表示用スナップショット [`GeofenceView`] として注入する。ボタンは世代番号
//! [`GeofenceShowState::register_seq`] を進めるだけで、実際の登録は effect が行う。

use crate::screens::background_task::format_utc;
use crate::screens::location_show::{
    DENIED_TEXT, FailureKind, RETRYING_TEXT, SERVICE_DISABLED_TEXT, UNAVAILABLE_TEXT,
};
use crate::{Screen, WidgetId};

/// 近い方の登録半径（m）。
pub const RADIUS_NEAR_M: f64 = 100.0;
/// 広い方の登録半径（m）。
pub const RADIUS_FAR_M: f64 = 1_000.0;

const NEAR_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const FAR_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 登録中の地点の表示用スナップショット。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionView {
    pub lat: f64,
    pub lon: f64,
    pub radius_m: f64,
}

/// 出入りイベントの表示用スナップショット（`entered` が真なら「入った」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventView {
    pub entered: bool,
    pub at_unix_ms: u64,
}

/// 登録要求が受け付けられなかった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterNotice {
    /// 現在地が未取得（偽の地点で登録しない）。
    NoFix,
    /// 現在地の精度が登録半径より悪い（判定に使えない）。新しい Fix を待ってもう一度押す。
    Inaccurate,
    /// 値が不正で判定器に拒否された。
    Invalid,
}

/// effect が注入する表示スナップショット。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeofenceView {
    pub region: Option<RegionView>,
    /// `None` は未確定。
    pub inside: Option<bool>,
    /// 古い順。
    pub events: Vec<EventView>,
    pub failure: Option<FailureKind>,
    pub notice: Option<RegisterNotice>,
    /// 最新 Fix を受け取ってからの経過秒。`None` は未取得（鮮度は判定せず表示のみ）。
    pub fix_age_secs: Option<u64>,
}

/// ジオフェンス画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeofenceShowState {
    pub view: GeofenceView,
    /// 登録ボタンだけが進める世代番号。effect が処理済みの値と比べて新規要求を検知する。
    pub register_seq: u64,
    /// 直近の登録要求の半径（m）。
    pub requested_radius_m: f64,
}

fn request(state: &mut GeofenceShowState, radius_m: f64) {
    state.requested_radius_m = radius_m;
    state.register_seq = state.register_seq.wrapping_add(1);
}

/// 「今いる場所を半径 100 m で登録」のハンドラ。
pub fn register_near(state: &mut GeofenceShowState) {
    request(state, RADIUS_NEAR_M);
}

/// 「今いる場所を半径 1 km で登録」のハンドラ。
pub fn register_far(state: &mut GeofenceShowState) {
    request(state, RADIUS_FAR_M);
}

/// 登録地点の文言。
pub fn region_text(state: &GeofenceShowState) -> String {
    match state.view.region {
        None => "登録地点: なし".to_string(),
        Some(r) => format!(
            "登録地点: 緯度 {:.6} / 経度 {:.6} / 半径 {:.0} m",
            r.lat, r.lon, r.radius_m
        ),
    }
}

/// 現在の状態の文言。失敗中は最後の内外を出さず失敗を示す（状態を偽らない）。
pub fn status_text(state: &GeofenceShowState) -> String {
    let v = &state.view;
    if let Some(kind) = v.failure {
        return match kind {
            FailureKind::PermissionDenied => DENIED_TEXT,
            FailureKind::ServiceDisabled => SERVICE_DISABLED_TEXT,
            FailureKind::Unsupported => UNAVAILABLE_TEXT,
            FailureKind::Retryable => RETRYING_TEXT,
        }
        .to_string();
    }
    if v.region.is_none() {
        return "状態: 未登録".to_string();
    }
    match v.inside {
        None => "状態: 未確定（測位待ち）".to_string(),
        Some(true) => "状態: 中にいる".to_string(),
        Some(false) => "状態: 外にいる".to_string(),
    }
}

/// 位置の受信からの経過時間の文言（未取得なら空）。古さの判断は利用者に任せる。
pub fn fix_age_text(state: &GeofenceShowState) -> String {
    match state.view.fix_age_secs {
        None => String::new(),
        Some(s) => format!("位置の受信: {s} 秒前"),
    }
}

/// 登録できなかったときの案内文言（なければ空）。
pub fn notice_text(state: &GeofenceShowState) -> String {
    match state.view.notice {
        None => String::new(),
        Some(RegisterNotice::NoFix) => "現在地が未取得のため登録できません".to_string(),
        Some(RegisterNotice::Inaccurate) => {
            "現在地の精度が登録半径より低いため登録できません。少し待ってからもう一度押してください。位置情報を「おおよそ」だけ許可している場合は、設定で正確な位置の使用を許可してください"
                .to_string()
        }
        Some(RegisterNotice::Invalid) => "登録できない値です".to_string(),
    }
}

/// 出入り履歴の文言（新しい順）。
pub fn history_text(state: &GeofenceShowState) -> String {
    if state.view.events.is_empty() {
        return "履歴: まだありません".to_string();
    }
    let lines: Vec<String> = state
        .view
        .events
        .iter()
        .rev()
        .map(|e| {
            format!(
                "{} {}",
                if e.entered { "入った" } else { "出た" },
                format_utc(e.at_unix_ms)
            )
        })
        .collect();
    format!("履歴:\n{}", lines.join("\n"))
}

/// ジオフェンス画面を組み立てる。`registry.rs` の `effect` 区画から呼ばれる。
pub fn current_screen(_state: &GeofenceShowState) -> Screen<GeofenceShowState> {
    Screen::new("ジオフェンス")
        .button(NEAR_BUTTON_ID, "今いる場所を登録（100 m）", register_near)
        .button(FAR_BUTTON_ID, "今いる場所を登録（1 km）", register_far)
        .dynamic_text(region_text)
        .dynamic_text(status_text)
        .dynamic_text(fix_age_text)
        .dynamic_text(notice_text)
        .dynamic_text(history_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    fn registered() -> GeofenceShowState {
        let mut s = GeofenceShowState::default();
        s.view.region = Some(RegionView {
            lat: 1.0,
            lon: 2.0,
            radius_m: 100.0,
        });
        s
    }

    #[test]
    fn default_is_unregistered_without_failure_or_history() {
        let s = GeofenceShowState::default();
        assert_eq!(status_text(&s), "状態: 未登録");
        assert_eq!(history_text(&s), "履歴: まだありません");
        assert!(notice_text(&s).is_empty());
    }

    #[test]
    fn fix_age_is_shown_only_when_received() {
        let mut s = GeofenceShowState::default();
        assert!(fix_age_text(&s).is_empty());
        s.view.fix_age_secs = Some(7);
        assert_eq!(fix_age_text(&s), "位置の受信: 7 秒前");
    }

    #[test]
    fn buttons_advance_seq_and_set_radius() {
        let mut s = GeofenceShowState::default();
        let screen = current_screen(&s);
        assert!(click_button(&screen, "今いる場所を登録（100 m）", &mut s));
        assert_eq!((s.register_seq, s.requested_radius_m), (1, RADIUS_NEAR_M));
        assert!(click_button(&screen, "今いる場所を登録（1 km）", &mut s));
        assert_eq!((s.register_seq, s.requested_radius_m), (2, RADIUS_FAR_M));
    }

    #[test]
    fn failure_hides_last_inside_state() {
        let mut s = registered();
        s.view.inside = Some(true);
        s.view.failure = Some(FailureKind::PermissionDenied);
        assert_eq!(status_text(&s), DENIED_TEXT);
    }

    #[test]
    fn history_is_newest_first_and_all_states_render() {
        let mut s = registered();
        s.view.events = vec![
            EventView {
                entered: true,
                at_unix_ms: 0,
            },
            EventView {
                entered: false,
                at_unix_ms: 60_000,
            },
        ];
        let h = history_text(&s);
        assert!(h.find("出た").unwrap() < h.find("入った").unwrap());
        for inside in [None, Some(true), Some(false)] {
            s.view.inside = inside;
            for notice in [
                None,
                Some(RegisterNotice::NoFix),
                Some(RegisterNotice::Inaccurate),
                Some(RegisterNotice::Invalid),
            ] {
                s.view.notice = notice;
                if notice == Some(RegisterNotice::Inaccurate) {
                    // 「おおよそ」のみ許可・Precise OFF は待っても直らないため、理由を示す。
                    assert!(notice_text(&s).contains("おおよそ"));
                }
                let _ = (
                    region_text(&s),
                    status_text(&s),
                    notice_text(&s),
                    current_screen(&s),
                );
            }
        }
    }
}
