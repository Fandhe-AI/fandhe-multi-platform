//! Phase 11 の拡充ループ（親 #129）型サンプル「UWB による距離・方向」（Issue #653）。
//! 端末の測距能力（距離・方向を測れるか）を常に出し、「開始」で近くの相手を探して token を交換し、
//! 相手までの距離（m）を出し続ける。`FANDHE_PROXY_APP=uwb_ranging_show` で選ばれ、
//! `fandhe-egui-proxy` の `screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod uwb_ranging_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `device/uwb_ranging.rs` が行い、[`apply_capability`] / [`apply_poll`] / [`apply_end`] /
//! [`apply_failure`] で注入する。`fandhe_device::uwb_ranging` の型は依存方向の都合でここへ写す。
//!
//! 嘘の距離を出さない: 距離は [`RangingView::Measured`] だけが持つ（センチメートルの `u32`）。
//! 圏外（[`RangingView::NoMeasurement`]）・探索中・終端・失敗は距離を持たず、0 m で埋めない。
//! 古い世代の完了は [`UwbRangingShowState::session`] で捨てる。能力が読めていない・測れない端末では
//! 「開始」を出さない。文言と操作 UI は全対象 OS（iOS）の分をここで定義する。

use crate::{Screen, WidgetId};

/// 「開始」ボタン。
pub const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタン。
pub const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「開始」の文言。
pub const START_LABEL: &str = "開始";
/// 「停止」の文言。
pub const STOP_LABEL: &str = "停止";
/// 待機中の案内。
pub const IDLE_TEXT: &str = "開始を押すと近くの相手を探して距離を表示します";
/// 探索中。
pub const SEARCHING_TEXT: &str = "近くの相手を探しています…";
/// token 交換中。
pub const EXCHANGING_TEXT: &str = "相手と接続中…";
/// 許可待ち。
pub const AWAITING_PERMISSION_TEXT: &str = "近くのデバイスとのやり取りの許可を待っています";
/// 圏外・遮蔽。
pub const NO_MEASUREMENT_TEXT: &str = "距離を測れません（圏外または遮られています）";
/// 相手が測距を終了した。
pub const PEER_ENDED_TEXT: &str = "相手が測距を終了しました";
/// 相手が見つからなかった。
pub const NO_PEER_TEXT: &str = "相手が見つかりませんでした";
/// 中断。
pub const INTERRUPTED_TEXT: &str = "中断しました（背景へ移ったため）";
/// 非対応 OS。
pub const UNSUPPORTED_TEXT: &str = "この OS では使えません";
/// 非搭載。
pub const NOT_CAPABLE_TEXT: &str = "この端末は UWB に対応していません";
/// 権限拒否。
pub const PERMISSION_DENIED_TEXT: &str = "許可されませんでした";
/// 時間切れ。
pub const TIMEOUT_TEXT: &str = "時間切れ（相手と接続できませんでした）";
/// それ以外の失敗。
pub const FAILED_TEXT: &str = "取得できません";
/// 能力を読む前の表示。
pub const CAPABILITY_PENDING_TEXT: &str = "端末の能力: 確認中…";

/// 能力の 1 項目（`fandhe_device::uwb_ranging::Support` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportView {
    /// 使える。
    Yes,
    /// 使えない。
    No,
    /// 判定できない。
    Unknown,
}

/// 失敗の種別（`UwbFailure` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UwbFailureView {
    /// OS に API が無い。
    UnsupportedOnOs,
    /// 端末に UWB が無い。
    NotCapable,
    /// 許可が拒否された。
    PermissionDenied,
    /// 窓の中で交換が終わらない。
    Timeout,
    /// それ以外。
    Failed,
}

/// 能力の表示状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapabilityView {
    /// まだ読んでいない。
    #[default]
    NotRead,
    /// 読めた。
    Known {
        /// 距離を測れるか。
        distance: SupportView,
        /// 方向を測れるか。
        direction: SupportView,
    },
    /// 読めなかった。
    Unavailable(UwbFailureView),
}

/// 読み取り結果の写し（`UwbPoll` のうち終端でないもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangingPollView {
    /// 探索中。
    Searching,
    /// token 交換中。
    Exchanging,
    /// 許可待ち。
    AwaitingPermission,
    /// 距離あり。
    Measured {
        /// 距離（センチメートル）。検証済みの値だけが届く。
        cm: u32,
        /// 受信番号。同じ番号は二重に反映しない。
        seq: u64,
    },
    /// 距離なし。
    NoMeasurement,
}

/// 正常終了の理由（`EndReason` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReasonView {
    /// 相手が止めた。
    PeerEnded,
    /// 中断。
    Interrupted,
    /// 相手が見つからなかった。
    NoPeer,
}

/// 表示状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RangingView {
    /// 停止中。
    #[default]
    Idle,
    /// 探索中。
    Searching,
    /// 交換中。
    Exchanging,
    /// 許可待ち。
    AwaitingPermission,
    /// 距離を表示中。
    Measured {
        /// 距離（センチメートル）。
        cm: u32,
        /// 受信番号。
        seq: u64,
    },
    /// 距離なし（圏外）。
    NoMeasurement,
    /// 正常終了。
    Ended(EndReasonView),
    /// 失敗。
    Failed(UwbFailureView),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UwbRangingShowState {
    /// 端末の能力。
    pub capability: CapabilityView,
    /// 測距の表示状態。
    pub view: RangingView,
    /// 測距を望んでいるか。driver が立ち上がり・立ち下がりで OS の開始・停止を行う。
    pub wanted: bool,
    /// 「開始」のたびに増える世代。遅れて届いた完了を捨てるために使う。
    pub session: u64,
    /// 最後に反映した受信番号。
    pub last_seq: Option<u64>,
}

/// 「開始」。冪等（測距中の二重押下では世代を進めない）。
pub fn start(s: &mut UwbRangingShowState) {
    if s.wanted {
        return;
    }
    s.wanted = true;
    s.view = RangingView::Searching;
    s.session = s.session.wrapping_add(1);
    s.last_seq = None;
}

/// 「停止」。冪等。キャンセルであって失敗ではないので待機へ戻し、距離は残さない。
pub fn stop(s: &mut UwbRangingShowState) {
    s.wanted = false;
    s.view = RangingView::Idle;
}

/// 能力を反映する。世代には依存しない（許可なしで読める端末の属性）。
pub fn apply_capability(s: &mut UwbRangingShowState, c: CapabilityView) {
    s.capability = c;
}

/// 読み取り結果を反映する。停止後・世代違いは捨てる。同じ受信番号は二重に反映しない。
pub fn apply_poll(s: &mut UwbRangingShowState, session: u64, poll: RangingPollView) {
    if !s.wanted || s.session != session {
        return;
    }
    s.view = match poll {
        RangingPollView::Searching => RangingView::Searching,
        RangingPollView::Exchanging => RangingView::Exchanging,
        RangingPollView::AwaitingPermission => RangingView::AwaitingPermission,
        RangingPollView::NoMeasurement => RangingView::NoMeasurement,
        RangingPollView::Measured { cm, seq } => {
            if s.last_seq == Some(seq) {
                return;
            }
            s.last_seq = Some(seq);
            RangingView::Measured { cm, seq }
        }
    };
}

/// 正常終了を反映する。世代が一致したときだけ確定し、測距の希望を下ろす（再度「開始」できる）。
pub fn apply_end(s: &mut UwbRangingShowState, session: u64, reason: EndReasonView) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = RangingView::Ended(reason);
}

/// 失敗を反映する。世代が一致したときだけ確定し、測距の希望を下ろす（再度「開始」できる）。
pub fn apply_failure(s: &mut UwbRangingShowState, session: u64, f: UwbFailureView) {
    if !s.wanted || s.session != session {
        return;
    }
    s.wanted = false;
    s.view = RangingView::Failed(f);
}

/// 「開始」を出してよいか。能力が読めていて、距離を測れないと分かっている端末では出さない。
pub fn can_start(s: &UwbRangingShowState) -> bool {
    matches!(
        s.capability,
        CapabilityView::Known { distance, .. } if distance != SupportView::No
    )
}

/// 失敗の文言。
pub fn failure_text(f: UwbFailureView) -> &'static str {
    match f {
        UwbFailureView::UnsupportedOnOs => UNSUPPORTED_TEXT,
        UwbFailureView::NotCapable => NOT_CAPABLE_TEXT,
        UwbFailureView::PermissionDenied => PERMISSION_DENIED_TEXT,
        UwbFailureView::Timeout => TIMEOUT_TEXT,
        UwbFailureView::Failed => FAILED_TEXT,
    }
}

fn support_label(v: SupportView) -> &'static str {
    match v {
        SupportView::Yes => "測れる",
        SupportView::No => "測れない",
        SupportView::Unknown => "不明",
    }
}

/// 能力の行（`DynamicText` の `get`）。
pub fn capability_text(s: &UwbRangingShowState) -> String {
    match s.capability {
        CapabilityView::NotRead => CAPABILITY_PENDING_TEXT.to_string(),
        CapabilityView::Known {
            distance,
            direction,
        } => format!(
            "距離: {} / 方向: {}",
            support_label(distance),
            support_label(direction)
        ),
        CapabilityView::Unavailable(f) => failure_text(f).to_string(),
    }
}

/// 測距の状態の行。
pub fn status_text(s: &UwbRangingShowState) -> String {
    match s.view {
        RangingView::Idle => IDLE_TEXT,
        RangingView::Searching => SEARCHING_TEXT,
        RangingView::Exchanging => EXCHANGING_TEXT,
        RangingView::AwaitingPermission => AWAITING_PERMISSION_TEXT,
        RangingView::Measured { .. } => "測距中",
        RangingView::NoMeasurement => NO_MEASUREMENT_TEXT,
        RangingView::Ended(EndReasonView::PeerEnded) => PEER_ENDED_TEXT,
        RangingView::Ended(EndReasonView::NoPeer) => NO_PEER_TEXT,
        RangingView::Ended(EndReasonView::Interrupted) => INTERRUPTED_TEXT,
        RangingView::Failed(f) => failure_text(f),
    }
    .to_string()
}

/// センチメートルを「1.23 m」の形にする。
pub fn format_distance(cm: u32) -> String {
    format!("{}.{:02} m", cm / 100, cm % 100)
}

/// 距離の行。距離が無いときは空文字（0 m で埋めない）。
pub fn distance_text(s: &UwbRangingShowState) -> String {
    match s.view {
        RangingView::Measured { cm, .. } => format!("距離: {}", format_distance(cm)),
        _ => String::new(),
    }
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。測距中は「停止」、待機中は
/// 開始できる端末でだけ「開始」を出す。
pub fn current_screen(s: &UwbRangingShowState) -> Screen<UwbRangingShowState> {
    let screen = Screen::new("UWB 測距")
        .dynamic_text(capability_text)
        .dynamic_text(status_text)
        .dynamic_text(distance_text);
    if s.wanted {
        screen.button(STOP_BUTTON_ID, STOP_LABEL, stop)
    } else if can_start(s) {
        screen.button(START_BUTTON_ID, START_LABEL, start)
    } else {
        screen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> CapabilityView {
        CapabilityView::Known {
            distance: SupportView::Yes,
            direction: SupportView::Unknown,
        }
    }

    fn started() -> UwbRangingShowState {
        let mut s = UwbRangingShowState::default();
        apply_capability(&mut s, known());
        start(&mut s);
        s
    }

    fn m(cm: u32, seq: u64) -> RangingPollView {
        RangingPollView::Measured { cm, seq }
    }

    const FAILURES: [UwbFailureView; 5] = [
        UwbFailureView::UnsupportedOnOs,
        UwbFailureView::NotCapable,
        UwbFailureView::PermissionDenied,
        UwbFailureView::Timeout,
        UwbFailureView::Failed,
    ];

    #[test]
    fn start_is_idempotent_and_bumps_session_once() {
        let mut s = started();
        assert_eq!(
            (s.view, s.wanted, s.session),
            (RangingView::Searching, true, 1)
        );
        start(&mut s);
        assert_eq!(s.session, 1);
    }

    #[test]
    fn stop_returns_to_idle_without_failure_text() {
        let mut s = started();
        apply_poll(&mut s, 1, m(123, 1));
        stop(&mut s);
        assert_eq!((s.view, s.wanted), (RangingView::Idle, false));
        assert_eq!(status_text(&s), IDLE_TEXT);
        assert_eq!(distance_text(&s), "");
        assert!(!status_text(&s).contains("取得できません"));
    }

    #[test]
    fn stale_and_stopped_results_are_dropped() {
        let mut s = started();
        stop(&mut s);
        start(&mut s);
        apply_poll(&mut s, 1, m(100, 1));
        apply_end(&mut s, 1, EndReasonView::PeerEnded);
        apply_failure(&mut s, 1, UwbFailureView::Failed);
        assert_eq!((s.view, s.wanted), (RangingView::Searching, true));
        stop(&mut s);
        apply_poll(&mut s, 2, m(100, 1));
        apply_end(&mut s, 2, EndReasonView::PeerEnded);
        apply_failure(&mut s, 2, UwbFailureView::Failed);
        assert_eq!(s.view, RangingView::Idle);
    }

    #[test]
    fn same_seq_is_not_applied_twice() {
        let mut s = started();
        apply_poll(&mut s, 1, m(100, 5));
        apply_poll(&mut s, 1, m(999, 5));
        assert_eq!(s.view, RangingView::Measured { cm: 100, seq: 5 });
        apply_poll(&mut s, 1, m(150, 6));
        assert_eq!(s.view, RangingView::Measured { cm: 150, seq: 6 });
    }

    #[test]
    fn no_measurement_has_no_distance_row() {
        let mut s = started();
        apply_poll(&mut s, 1, RangingPollView::NoMeasurement);
        assert_eq!(distance_text(&s), "");
        assert!(!status_text(&s).contains('0'));
    }

    #[test]
    fn value_disappears_after_no_measurement() {
        let mut s = started();
        apply_poll(&mut s, 1, m(250, 1));
        assert_eq!(distance_text(&s), "距離: 2.50 m");
        apply_poll(&mut s, 1, RangingPollView::NoMeasurement);
        assert_eq!(s.view, RangingView::NoMeasurement);
        assert_eq!(distance_text(&s), "");
    }

    #[test]
    fn progress_texts() {
        let mut s = started();
        assert_eq!(status_text(&s), SEARCHING_TEXT);
        apply_poll(&mut s, 1, RangingPollView::Exchanging);
        assert_eq!(status_text(&s), EXCHANGING_TEXT);
        apply_poll(&mut s, 1, RangingPollView::AwaitingPermission);
        assert_eq!(status_text(&s), AWAITING_PERMISSION_TEXT);
        apply_poll(&mut s, 1, RangingPollView::Searching);
        assert_eq!(status_text(&s), SEARCHING_TEXT);
    }

    #[test]
    fn ends_are_not_failures() {
        for r in [
            EndReasonView::PeerEnded,
            EndReasonView::Interrupted,
            EndReasonView::NoPeer,
        ] {
            let mut s = started();
            apply_poll(&mut s, 1, m(100, 1));
            apply_end(&mut s, 1, r);
            assert!(!s.wanted);
            let t = status_text(&s);
            assert!(!t.contains("失敗") && !t.contains("取得できません"));
            assert_eq!(distance_text(&s), "");
            start(&mut s);
            assert_eq!(s.session, 2);
        }
    }

    #[test]
    fn failures_drop_wanted_and_value() {
        for f in FAILURES {
            let mut s = started();
            apply_poll(&mut s, 1, m(100, 1));
            apply_failure(&mut s, 1, f);
            assert!(!s.wanted);
            assert_eq!(status_text(&s), failure_text(f));
            assert_eq!(distance_text(&s), "");
        }
    }

    #[test]
    fn start_button_requires_known_distance_capability() {
        let mut s = UwbRangingShowState::default();
        assert!(!can_start(&s));
        for f in FAILURES {
            apply_capability(&mut s, CapabilityView::Unavailable(f));
            assert!(!can_start(&s));
        }
        apply_capability(
            &mut s,
            CapabilityView::Known {
                distance: SupportView::No,
                direction: SupportView::No,
            },
        );
        assert!(!can_start(&s));
        apply_capability(&mut s, known());
        assert!(can_start(&s));
        apply_capability(
            &mut s,
            CapabilityView::Known {
                distance: SupportView::Unknown,
                direction: SupportView::Unknown,
            },
        );
        assert!(can_start(&s));
    }

    #[test]
    fn capability_texts() {
        let mut s = UwbRangingShowState::default();
        assert_eq!(capability_text(&s), CAPABILITY_PENDING_TEXT);
        apply_capability(&mut s, known());
        assert_eq!(capability_text(&s), "距離: 測れる / 方向: 不明");
        apply_capability(
            &mut s,
            CapabilityView::Unavailable(UwbFailureView::UnsupportedOnOs),
        );
        assert_eq!(capability_text(&s), UNSUPPORTED_TEXT);
    }

    #[test]
    fn format_distance_shape() {
        assert_eq!(format_distance(123), "1.23 m");
        assert_eq!(format_distance(5), "0.05 m");
        assert_eq!(format_distance(5000), "50.00 m");
    }
}
