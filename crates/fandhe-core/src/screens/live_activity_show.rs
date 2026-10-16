//! Phase 11（端末機能の拡充ループ、親 #88）型サンプル「ライブアクティビティ」（Issue #294）。
//! 数値 1 つを OS のライブ表示（iOS はロック画面と Dynamic Island、Android は進行中通知）に出し、
//! 「開始」「更新」「終了」の 3 操作だけを行う 1 画面。`FANDHE_PROXY_APP=live_activity_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod live_activity_show;` と対）。同時に出すのは 1 件で、push 更新・
//! 複数同時・アプリ内ボタンは扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/live_activity.rs` が [`take_command`] で要求を観測し、
//! [`apply_result`] で結果を注入する（`call_ui_show.rs` と同型）。iOS・Android の案内も含め全対象 OS の
//! 状態・文言をここで確定済みで、後続の OS 実装（b 層）は本画面を触らない。
//!
//! ```text
//! Idle --「開始」--> Starting --Started--> Active{0}
//! Active{v} --「更新」--> Updating{v} --Updated--> Active{v+1}
//!                                    --Dismissed--> Idle（閉じられていた）
//! Active{v} --「終了」--> Ending{v} --Ended/Dismissed--> Idle
//! ```
//!
//! 失敗は error 側だけに出し、成功・閉じられていた・終了の文言は status 側だけに出す。世代の違う完了と、
//! 段階に合わない完了は捨てる。

use crate::{Screen, WidgetId};

const START_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const UPDATE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const END_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 画面タイトル。
pub const TITLE: &str = "ライブアクティビティ";
/// 開始ボタンのラベル。
pub const START_LABEL: &str = "開始";
/// 更新ボタンのラベル（押すたびに値を 1 増やす）。
pub const UPDATE_LABEL: &str = "更新（+1）";
/// 終了ボタンのラベル。
pub const END_LABEL: &str = "終了";
/// OS ごとの案内。
pub const HINT: &str = "iOS はロック画面と Dynamic Island、Android は通知領域に出ます。iOS は設定でライブアクティビティを切ると使えません";
/// 開始中の案内。
pub const STARTING_STATUS: &str = "開始しています…";
/// 表示中の案内の接頭辞（値を続ける）。
pub const ACTIVE_PREFIX: &str = "表示中: ";
/// 更新中の案内。
pub const UPDATING_STATUS: &str = "更新しています…";
/// 終了中の案内。
pub const ENDING_STATUS: &str = "終了しています…";
/// 終了したときの案内。
pub const ENDED_STATUS: &str = "終了しました";
/// 利用者やシステムが既に閉じていたときの案内（失敗ではない）。
pub const DISMISSED_STATUS: &str = "閉じられていました";
/// 非対応のときの案内。
pub const UNSUPPORTED_MESSAGE: &str =
    "この環境ではライブアクティビティを使えません（iOS は拡張を同梱したビルドが必要です）";
/// 許可がないときの案内。
pub const DENIED_MESSAGE: &str = "ライブアクティビティが許可されていません";
/// 値が範囲外のときの案内。
pub const OUT_OF_RANGE_MESSAGE: &str = "値が範囲外です（0〜9999）";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 画面側の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LivePhase {
    /// 表示なし。
    #[default]
    Idle,
    /// 開始の結果待ち。
    Starting,
    /// 表示中。
    Active {
        /// 表示中の値。
        value: u32,
    },
    /// 更新の結果待ち。
    Updating {
        /// 更新前の値。
        value: u32,
    },
    /// 終了の結果待ち。
    Ending {
        /// 終了前の値。
        value: u32,
    },
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveCommand {
    /// 値を指定して開始する。
    Start(u32),
    /// 値を指定して更新する。
    Update(u32),
    /// 終了する。
    End,
}

/// effect ドライバから注入する結果（`fandhe_device::live_activity` の結果の写し。依存方向のため複製し、
/// 写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveResult {
    /// 開始を受理した。
    Started,
    /// 更新を受理した。
    Updated,
    /// 終了を受理した。
    Ended,
    /// 利用者やシステムが既に閉じていた（正常終了）。
    Dismissed,
    /// 許可がない。
    Denied,
    /// 非対応。
    Unsupported,
    /// 値が範囲外。
    OutOfRange,
    /// それ以外（固定文言のみ。OS の生コードを入れない）。
    Failed(String),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveActivityShowState {
    /// 進行状態。
    pub phase: LivePhase,
    /// 直近に立てた要求の世代。
    pub generation: u64,
    /// 未発送の要求。
    pub command: Option<LiveCommand>,
    /// `Idle` に戻ったときの案内（終了・閉じられていた）。
    pub status: String,
    /// 失敗の案内。
    pub error: String,
}

fn begin(state: &mut LiveActivityShowState, phase: LivePhase, command: LiveCommand) {
    state.generation = state.generation.wrapping_add(1);
    state.phase = phase;
    state.command = Some(command);
    state.status.clear();
    state.error.clear();
}

/// 「開始」。`Idle` のときだけ受け付ける。
pub fn start(state: &mut LiveActivityShowState) {
    if state.phase == LivePhase::Idle {
        begin(state, LivePhase::Starting, LiveCommand::Start(0));
    }
}

/// 「更新」。`Active` のときだけ受け付ける。9999 の次は 10000 のまま送り、ドライバの検証で
/// `OutOfRange` を返させる（入力不正の経路）。
pub fn update(state: &mut LiveActivityShowState) {
    if let LivePhase::Active { value } = state.phase {
        begin(
            state,
            LivePhase::Updating { value },
            LiveCommand::Update(value.saturating_add(1)),
        );
    }
}

/// 「終了」。`Active` のときだけ受け付ける。
pub fn end(state: &mut LiveActivityShowState) {
    if let LivePhase::Active { value } = state.phase {
        begin(state, LivePhase::Ending { value }, LiveCommand::End);
    }
}

/// 未発送の要求と世代を観測する getter（状態は変えない）。
pub fn take_command(state: &LiveActivityShowState) -> Option<(LiveCommand, u64)> {
    state.command.map(|c| (c, state.generation))
}

/// 要求を発送済みにする。世代が違えば何もしない。
pub fn mark_command_taken(state: &mut LiveActivityShowState, generation: u64) {
    if state.generation == generation {
        state.command = None;
    }
}

fn failure_message(result: LiveResult) -> String {
    match result {
        LiveResult::Denied => DENIED_MESSAGE.to_string(),
        LiveResult::Unsupported => UNSUPPORTED_MESSAGE.to_string(),
        LiveResult::OutOfRange => OUT_OF_RANGE_MESSAGE.to_string(),
        LiveResult::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        // 想定外の成功結果（段階に合わない Started 等）。成功へ倒さず失敗として見せる。
        _ => format!("{FAILED_PREFIX}想定外の結果です"),
    }
}

/// 結果を反映する。世代違いと、結果待ちでない段階（`Idle` / `Active`）で届いたものは捨てる。
pub fn apply_result(state: &mut LiveActivityShowState, generation: u64, result: LiveResult) {
    if state.generation != generation {
        return;
    }
    match (state.phase, result) {
        (LivePhase::Starting, LiveResult::Started) => state.phase = LivePhase::Active { value: 0 },
        (LivePhase::Starting, r) => {
            state.phase = LivePhase::Idle;
            state.error = failure_message(r);
        }
        (LivePhase::Updating { value }, LiveResult::Updated) => {
            state.phase = LivePhase::Active {
                value: value.saturating_add(1),
            }
        }
        (LivePhase::Updating { .. }, LiveResult::Dismissed) => {
            state.phase = LivePhase::Idle;
            state.status = DISMISSED_STATUS.to_string();
        }
        (LivePhase::Updating { value }, r @ (LiveResult::OutOfRange | LiveResult::Failed(_))) => {
            state.phase = LivePhase::Active { value };
            state.error = failure_message(r);
        }
        (LivePhase::Updating { .. }, r) => {
            state.phase = LivePhase::Idle;
            state.error = failure_message(r);
        }
        (LivePhase::Ending { .. }, LiveResult::Ended | LiveResult::Dismissed) => {
            state.phase = LivePhase::Idle;
            state.status = ENDED_STATUS.to_string();
        }
        (LivePhase::Ending { value }, r) => {
            // 終了をやり直せるよう表示中へ戻す。
            state.phase = LivePhase::Active { value };
            state.error = failure_message(r);
        }
        _ => {}
    }
}

/// 案内文言（通常色）。
pub fn status_text(state: &LiveActivityShowState) -> String {
    match state.phase {
        LivePhase::Idle => state.status.clone(),
        LivePhase::Starting => STARTING_STATUS.to_string(),
        LivePhase::Active { value } => format!("{ACTIVE_PREFIX}{value}"),
        LivePhase::Updating { .. } => UPDATING_STATUS.to_string(),
        LivePhase::Ending { .. } => ENDING_STATUS.to_string(),
    }
}

/// 失敗の案内（`TextRole::Error`）。失敗以外は空。
pub fn error_text(state: &LiveActivityShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。結果待ちの間はボタンを出さない。
pub fn current_screen(state: &LiveActivityShowState) -> Screen<LiveActivityShowState> {
    let screen = Screen::new(TITLE)
        .text(HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    match state.phase {
        LivePhase::Idle => screen.button(START_BUTTON_ID, START_LABEL, start),
        LivePhase::Active { .. } => screen
            .button(UPDATE_BUTTON_ID, UPDATE_LABEL, update)
            .button(END_BUTTON_ID, END_LABEL, end),
        LivePhase::Starting | LivePhase::Updating { .. } | LivePhase::Ending { .. } => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(s: &mut LiveActivityShowState, r: LiveResult) {
        let g = s.generation;
        apply_result(s, g, r);
    }

    fn active() -> LiveActivityShowState {
        let mut s = LiveActivityShowState::default();
        start(&mut s);
        res(&mut s, LiveResult::Started);
        s
    }

    #[test]
    fn normal_flow() {
        let mut s = LiveActivityShowState::default();
        start(&mut s);
        assert_eq!(take_command(&s), Some((LiveCommand::Start(0), 1)));
        res(&mut s, LiveResult::Started);
        assert_eq!(s.phase, LivePhase::Active { value: 0 });
        update(&mut s);
        assert_eq!(s.phase, LivePhase::Updating { value: 0 });
        assert_eq!(take_command(&s).map(|c| c.0), Some(LiveCommand::Update(1)));
        res(&mut s, LiveResult::Updated);
        assert_eq!(s.phase, LivePhase::Active { value: 1 });
        end(&mut s);
        assert_eq!(take_command(&s).map(|c| c.0), Some(LiveCommand::End));
        res(&mut s, LiveResult::Ended);
        assert_eq!(s.phase, LivePhase::Idle);
        assert_eq!(status_text(&s), ENDED_STATUS);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn invalid_phase_is_noop() {
        let mut s = LiveActivityShowState::default();
        update(&mut s);
        end(&mut s);
        assert_eq!(s, LiveActivityShowState::default());
        let mut a = active();
        let before = a.generation;
        start(&mut a);
        assert_eq!(a.generation, before);
        let mut p = LiveActivityShowState::default();
        start(&mut p);
        let g = p.generation;
        start(&mut p);
        update(&mut p);
        end(&mut p);
        assert_eq!(p.generation, g);
    }

    #[test]
    fn stale_and_idle_results_are_dropped() {
        let mut s = LiveActivityShowState::default();
        start(&mut s);
        let g = s.generation;
        apply_result(&mut s, g - 1, LiveResult::Started);
        assert_eq!(s.phase, LivePhase::Starting);
        let mut i = LiveActivityShowState::default();
        res(&mut i, LiveResult::Started);
        assert_eq!(i, LiveActivityShowState::default());
        let mut a = active();
        res(&mut a, LiveResult::Updated);
        assert_eq!(a.phase, LivePhase::Active { value: 0 });
    }

    #[test]
    fn failures_only_show_in_error() {
        for r in [
            LiveResult::Denied,
            LiveResult::Unsupported,
            LiveResult::OutOfRange,
            LiveResult::Failed("x".into()),
        ] {
            let mut s = LiveActivityShowState::default();
            start(&mut s);
            res(&mut s, r);
            assert_eq!(s.phase, LivePhase::Idle);
            assert!(!error_text(&s).is_empty());
            assert_eq!(status_text(&s), "");
        }
        // 開始で成功以外（Dismissed 含む）は成功文言にならない。
        let mut s = LiveActivityShowState::default();
        start(&mut s);
        res(&mut s, LiveResult::Dismissed);
        assert_eq!(s.phase, LivePhase::Idle);
        assert!(!error_text(&s).is_empty());
    }

    #[test]
    fn update_failures_keep_or_drop_active() {
        let mut s = active();
        update(&mut s);
        res(&mut s, LiveResult::Failed("x".into()));
        assert_eq!(s.phase, LivePhase::Active { value: 0 });
        assert!(!error_text(&s).is_empty());
        update(&mut s);
        res(&mut s, LiveResult::Denied);
        assert_eq!(s.phase, LivePhase::Idle);
        assert!(!error_text(&s).is_empty());
    }

    #[test]
    fn dismissed_is_not_an_error() {
        let mut s = active();
        update(&mut s);
        res(&mut s, LiveResult::Dismissed);
        assert_eq!(s.phase, LivePhase::Idle);
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), DISMISSED_STATUS);
        let mut e = active();
        end(&mut e);
        res(&mut e, LiveResult::Dismissed);
        assert_eq!(error_text(&e), "");
        assert_eq!(status_text(&e), ENDED_STATUS);
    }

    #[test]
    fn end_failure_allows_retry() {
        let mut s = active();
        end(&mut s);
        res(&mut s, LiveResult::Failed("x".into()));
        assert_eq!(s.phase, LivePhase::Active { value: 0 });
        end(&mut s);
        assert_eq!(s.phase, LivePhase::Ending { value: 0 });
    }

    #[test]
    fn update_after_max_requests_out_of_range_value() {
        let mut s = active();
        s.phase = LivePhase::Active { value: 9999 };
        update(&mut s);
        assert_eq!(
            take_command(&s).map(|c| c.0),
            Some(LiveCommand::Update(10000))
        );
    }

    #[test]
    fn mark_command_taken_checks_generation() {
        let mut s = LiveActivityShowState::default();
        start(&mut s);
        mark_command_taken(&mut s, 0);
        assert!(take_command(&s).is_some());
        mark_command_taken(&mut s, 1);
        assert_eq!(take_command(&s), None);
    }
}
