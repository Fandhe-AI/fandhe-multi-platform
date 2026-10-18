//! Phase 11（端末機能の拡充ループ、親 #99）型サンプル「外部機器への再生」（Issue #386）。
//! 「再生」「停止」「出力先を選ぶ」の 3 ボタンで、実行時に生成した音をループ再生し、OS の出力先
//! （AirPlay・Cast 等）へ切り替える 1 画面。`FANDHE_PROXY_APP=cast_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod cast_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 呼び出しは
//! `fandhe-egui-proxy` の `device/media_cast.rs` が [`take_command`] で要求を観測し、
//! [`apply_update`] で通知を注入する（`pip_show.rs` と同型の状態機械）。
//!
//! 全対象 OS（macOS / iOS / Windows / Web）の状態・文言をここで確定済みで、後続の OS 実装は本画面を
//! 触らない。ボタンは Web の `mount` が `Screen` を作り直さない制約のため状態によらず常に出す。
//! Apple は OS のボタンが画面の右下に重なるため（[`PickerView::Native`]）、「出力先を選ぶ」は要求を出さず
//! [`NATIVE_PICKER_GUIDE`] を案内する。
//!
//! ```text
//! 画面を開く: Idle --prepare--> Preparing --Playback(Ready)--> Ready
//! 「再生」: Ready/Stopped/Interrupted --> Starting --Playback(Playing)--> Playing
//! 「停止」: Starting/Playing --> Stopping --Playback(Stopped)--> Stopped
//! 経路: Local / Choosing / Connecting / Connected(名前) / Disconnected / NoDevices / NotReported
//! Preparing/Starting/Stopping と Connecting --期限切れ expire--> Failed(Timeout)
//! ```
//!
//! ピッカーを閉じた（キャンセル）は失敗にせず経路を元へ戻す。世代の違う通知、期限切れの後に届く通知、
//! 意味のない遷移は捨てる。出力先の名前は画面にだけ出し、ログへは出さない。

use crate::{Screen, WidgetId};

const PLAY_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const PICKER_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 画面タイトル。
pub const TITLE: &str = "外部機器への再生";
/// 「再生」ボタンのラベル。
pub const PLAY_LABEL: &str = "再生";
/// 「停止」ボタンのラベル。
pub const STOP_LABEL: &str = "停止";
/// 「出力先を選ぶ」ボタンのラベル。
pub const PICKER_LABEL: &str = "出力先を選ぶ";
/// 準備中の案内。
pub const PREPARING_STATUS: &str = "準備しています…";
/// 再生できるときの案内。
pub const READY_STATUS: &str = "再生できます";
/// 再生開始中の案内。
pub const STARTING_STATUS: &str = "再生を始めています…";
/// 再生中の案内。
pub const PLAYING_STATUS: &str = "再生中";
/// 停止中の案内。
pub const STOPPING_STATUS: &str = "停止しています…";
/// 停止した（失敗ではない）。
pub const STOPPED_STATUS: &str = "停止しました";
/// 背景遷移・`pagehide` で止まった（失敗ではない）。
pub const INTERRUPTED_STATUS: &str = "中断されました";
/// 経路が `Local` で再生中のときの案内。
pub const LOCAL_STATUS: &str = "この端末で再生中";
/// 出力先を選んでいる間の案内。
pub const CHOOSING_STATUS: &str = "出力先を選んでいます…";
/// 接続中の案内。
pub const CONNECTING_STATUS: &str = "接続しています…";
/// 外部で再生中（名前の接頭辞）。
pub const CONNECTED_PREFIX: &str = "外部で再生中: ";
/// 外部機器が出力先に選ばれているが再生していないとき（名前の接頭辞。「再生中」とは言わない）。
pub const ROUTE_PREFIX: &str = "出力先: ";
/// 同上で OS が名前を教えないとき。
pub const ROUTE_NO_NAME_STATUS: &str = "出力先: 外部機器（名前は OS が教えません）";
/// 外部で再生中だが OS が名前を教えないとき。
pub const CONNECTED_NO_NAME_STATUS: &str = "外部で再生中（出力先の名前は OS が教えません）";
/// 経路を OS から読めないとき（接続中とは言わない）。
pub const NOT_REPORTED_STATUS: &str = "出力先は OS の表示で確認してください";
/// 外部機器から外れたとき。
pub const DISCONNECTED_STATUS: &str = "切断されました";
/// 出力先が見つからないとき。
pub const NO_DEVICES_STATUS: &str = "出力先が見つかりません";
/// Apple の案内（OS のボタンが右下に重なる）。
pub const NATIVE_PICKER_GUIDE: &str = "右下のボタンから出力先を選んでください";
/// 非対応のときの案内。
pub const UNSUPPORTED_STATUS: &str = "この環境では外部機器へ送れません";
/// ブラウザ・OS に止められたときの案内。
pub const BLOCKED_STATUS: &str = "ブラウザに止められました。もう一度押してください";
/// 音源が送れないときの案内。
pub const SOURCE_NOT_SUPPORTED_STATUS: &str = "この音源は送れません";
/// 期限切れの案内。
pub const TIMEOUT_STATUS: &str = "外部機器へ送れませんでした（期限切れ）";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 画面側の再生状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CastShowPlayback {
    /// 未準備。
    #[default]
    Idle,
    /// 準備中。
    Preparing,
    /// 再生できる。
    Ready,
    /// 再生開始中。
    Starting,
    /// 再生中。
    Playing,
    /// 停止中。
    Stopping,
    /// 停止した。失敗ではない。
    Stopped,
    /// 背景遷移などで止まった。失敗ではない。
    Interrupted,
    /// 失敗。
    Failed(CastShowFailure),
}

/// 出力先の経路（`fandhe_device` の型から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CastShowRoute {
    /// この端末で鳴らしている。
    #[default]
    Local,
    /// 出力先を選んでいる。
    Choosing,
    /// 接続中。
    Connecting,
    /// 外部で再生中。名前は OS が教えたときだけ。
    Connected(Option<String>),
    /// 外部機器から外れた。
    Disconnected,
    /// 出力先が見つからない。
    NoDevices,
    /// 経路を読めない。
    NotReported,
}

/// 失敗の種別。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastShowFailure {
    /// 非対応。
    Unsupported,
    /// 止められた。
    Blocked,
    /// この音源は送れない。
    SourceNotSupported,
    /// 期限切れ。
    Timeout,
    /// それ以外（固定文言のみ。OS の生コードを入れない）。
    Failed(String),
}

/// 出力先の選び方（Apple は OS のボタン、Windows・Web はアプリのボタン）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerView {
    /// OS のボタンが画面に重なる。
    Native,
    /// アプリのボタンからピッカーを出す。
    App,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastCommand {
    /// 音源を準備する。
    Prepare,
    /// ループ再生を始める。
    Play,
    /// 再生を止める。
    Stop,
    /// 出力先のピッカーを出す。
    ShowPicker,
}

/// effect ドライバから注入する通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastShowUpdate {
    /// 再生状態。`Ready` / `Playing` / `Stopped` / `Interrupted` のみ意味を持つ。
    Playback(CastShowPlayback),
    /// 経路。
    Route(CastShowRoute),
    /// 出力先の選び方（snapshot から反映）。
    Picker(PickerView),
    /// 失敗。
    Failed(CastShowFailure),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CastShowState {
    /// 再生状態。
    pub playback: CastShowPlayback,
    /// 出力先の経路。
    pub route: CastShowRoute,
    /// 出力先の選び方（未確定は `None`）。
    pub picker: Option<PickerView>,
    /// 直近に立てた要求の世代。
    pub generation: u64,
    /// 未発送の要求。
    pub command: Option<CastCommand>,
}

fn issue(state: &mut CastShowState, command: CastCommand) {
    state.generation = state.generation.wrapping_add(1);
    state.command = Some(command);
}

fn retryable(state: &CastShowState) -> bool {
    matches!(&state.playback, CastShowPlayback::Failed(f) if *f != CastShowFailure::Unsupported)
}

/// 準備を始める。`Idle` と、`Unsupported` 以外の `Failed`（やり直し）のときだけ受け付ける。
pub fn prepare(state: &mut CastShowState) {
    if state.playback != CastShowPlayback::Idle && !retryable(state) {
        return;
    }
    issue(state, CastCommand::Prepare);
    state.playback = CastShowPlayback::Preparing;
    state.route = CastShowRoute::Local;
}

/// 「再生」。`Ready` / `Stopped` / `Interrupted` のときだけ再生へ進む。`Idle` と `Unsupported` 以外の
/// `Failed` は準備からやり直す。遷移中・再生中・`Unsupported` では何もしない。
pub fn play(state: &mut CastShowState) {
    match state.playback {
        CastShowPlayback::Ready | CastShowPlayback::Stopped | CastShowPlayback::Interrupted => {
            issue(state, CastCommand::Play);
            state.playback = CastShowPlayback::Starting;
        }
        CastShowPlayback::Idle | CastShowPlayback::Failed(_) => prepare(state),
        _ => {}
    }
}

/// 「停止」。`Starting` / `Playing` のときだけ受け付ける。失敗ではなく `Stopped` へ向かう。
/// 未発送の `Play` を取り消す場合は OS を介さず直ちに `Stopped` にする。
pub fn stop(state: &mut CastShowState) {
    if !matches!(
        state.playback,
        CastShowPlayback::Starting | CastShowPlayback::Playing
    ) {
        return;
    }
    if state.playback == CastShowPlayback::Starting && state.command == Some(CastCommand::Play) {
        state.generation = state.generation.wrapping_add(1);
        state.command = None;
        state.playback = CastShowPlayback::Stopped;
        return;
    }
    issue(state, CastCommand::Stop);
    state.playback = CastShowPlayback::Stopping;
}

/// 「出力先を選ぶ」。[`PickerView::Native`] は要求を出さない（OS のボタンから選ぶ案内を出す）。
/// 準備済みのときだけ要求を出す（`Starting` 中は再生開始の期限追跡を壊さないため無視する）。選択中・接続中の二重押下は無視する。失敗（`Unsupported` 以外）からは準備をやり直す。
pub fn show_picker(state: &mut CastShowState) {
    if state.picker == Some(PickerView::Native) {
        return;
    }
    if matches!(
        state.route,
        CastShowRoute::Choosing | CastShowRoute::Connecting
    ) {
        return;
    }
    match state.playback {
        CastShowPlayback::Ready
        | CastShowPlayback::Playing
        | CastShowPlayback::Stopped
        | CastShowPlayback::Interrupted => issue(state, CastCommand::ShowPicker),
        CastShowPlayback::Idle | CastShowPlayback::Failed(_) => prepare(state),
        _ => {}
    }
}

/// 未発送の要求と世代を観測する getter（状態は変えない）。
pub fn take_command(state: &CastShowState) -> Option<(CastCommand, u64)> {
    state.command.map(|c| (c, state.generation))
}

/// 要求を発送済みにする。世代が違えば何もしない。
pub fn mark_command_taken(state: &mut CastShowState, generation: u64) {
    if state.generation == generation {
        state.command = None;
    }
}

fn live(p: &CastShowPlayback) -> bool {
    !matches!(
        p,
        CastShowPlayback::Idle | CastShowPlayback::Preparing | CastShowPlayback::Failed(_)
    )
}

/// 通知を反映する。世代違いは捨てる。現在の状態から意味のある遷移だけを受ける。
/// 経路は準備済みの間だけ受ける（失敗・未準備・期限切れの後に届く `Connected` は捨てる）。
pub fn apply_update(state: &mut CastShowState, generation: u64, update: CastShowUpdate) {
    if state.generation != generation {
        return;
    }
    use CastShowPlayback as P;
    match update {
        CastShowUpdate::Playback(next) => {
            let ok = matches!(
                (&state.playback, &next),
                (P::Preparing, P::Ready)
                    | (P::Starting, P::Playing | P::Stopped | P::Interrupted)
                    | (P::Playing | P::Stopping, P::Stopped | P::Interrupted)
            );
            if ok {
                state.playback = next;
            }
        }
        CastShowUpdate::Route(r) => {
            if live(&state.playback) {
                state.route = r;
            }
        }
        CastShowUpdate::Picker(p) => {
            if state.playback != P::Idle {
                state.picker = Some(p);
            }
        }
        CastShowUpdate::Failed(f) => {
            if matches!(
                state.playback,
                P::Preparing | P::Ready | P::Starting | P::Playing | P::Stopping
            ) {
                state.playback = P::Failed(f);
                state.route = CastShowRoute::Local;
            }
        }
    }
}

/// 期限切れ。`Preparing` / `Starting` / `Stopping`、または経路が `Connecting` で世代一致のときだけ失敗にし、
/// 世代を進める（遅れて届く通知を捨てる）。
pub fn expire(state: &mut CastShowState, generation: u64) {
    if state.generation != generation {
        return;
    }
    let transitioning = matches!(
        state.playback,
        CastShowPlayback::Preparing | CastShowPlayback::Starting | CastShowPlayback::Stopping
    );
    if transitioning || state.route == CastShowRoute::Connecting {
        state.generation = state.generation.wrapping_add(1);
        state.playback = CastShowPlayback::Failed(CastShowFailure::Timeout);
        state.route = CastShowRoute::Local;
    }
}

/// 再生状態の案内（通常色）。`Stopped` / `Interrupted` はここに出す。
pub fn status_text(state: &CastShowState) -> String {
    match state.playback {
        CastShowPlayback::Idle | CastShowPlayback::Failed(_) => "",
        CastShowPlayback::Preparing => PREPARING_STATUS,
        CastShowPlayback::Ready => READY_STATUS,
        CastShowPlayback::Starting => STARTING_STATUS,
        CastShowPlayback::Playing => PLAYING_STATUS,
        CastShowPlayback::Stopping => STOPPING_STATUS,
        CastShowPlayback::Stopped => STOPPED_STATUS,
        CastShowPlayback::Interrupted => INTERRUPTED_STATUS,
    }
    .to_string()
}

/// 経路の案内。Apple では選び方の案内を添える。失敗・未準備では空。
pub fn route_text(state: &CastShowState) -> String {
    if !live(&state.playback) {
        return String::new();
    }
    let route = match &state.route {
        CastShowRoute::Local if state.playback == CastShowPlayback::Playing => {
            LOCAL_STATUS.to_string()
        }
        CastShowRoute::Local => String::new(),
        CastShowRoute::Choosing => CHOOSING_STATUS.to_string(),
        CastShowRoute::Connecting => CONNECTING_STATUS.to_string(),
        CastShowRoute::Connected(name) => {
            let playing = state.playback == CastShowPlayback::Playing;
            match (name, playing) {
                (Some(n), true) => format!("{CONNECTED_PREFIX}{n}"),
                (None, true) => CONNECTED_NO_NAME_STATUS.to_string(),
                (Some(n), false) => format!("{ROUTE_PREFIX}{n}"),
                (None, false) => ROUTE_NO_NAME_STATUS.to_string(),
            }
        }
        CastShowRoute::Disconnected => DISCONNECTED_STATUS.to_string(),
        CastShowRoute::NoDevices => NO_DEVICES_STATUS.to_string(),
        CastShowRoute::NotReported => NOT_REPORTED_STATUS.to_string(),
    };
    if state.picker == Some(PickerView::Native) {
        if route.is_empty() {
            NATIVE_PICKER_GUIDE.to_string()
        } else {
            format!("{route}\n{NATIVE_PICKER_GUIDE}")
        }
    } else {
        route
    }
}

/// 失敗の案内（`TextRole::Error`）。失敗以外は空。
pub fn error_text(state: &CastShowState) -> String {
    match &state.playback {
        CastShowPlayback::Failed(CastShowFailure::Unsupported) => UNSUPPORTED_STATUS.to_string(),
        CastShowPlayback::Failed(CastShowFailure::Blocked) => BLOCKED_STATUS.to_string(),
        CastShowPlayback::Failed(CastShowFailure::SourceNotSupported) => {
            SOURCE_NOT_SUPPORTED_STATUS.to_string()
        }
        CastShowPlayback::Failed(CastShowFailure::Timeout) => TIMEOUT_STATUS.to_string(),
        CastShowPlayback::Failed(CastShowFailure::Failed(m)) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 画面を組み立てる。ボタンは状態によらず 3 つとも出す。
pub fn current_screen(_state: &CastShowState) -> Screen<CastShowState> {
    Screen::new(TITLE)
        .dynamic_text(status_text)
        .dynamic_text(route_text)
        .error_text(error_text)
        .button(PLAY_BUTTON_ID, PLAY_LABEL, play)
        .button(STOP_BUTTON_ID, STOP_LABEL, stop)
        .button(PICKER_BUTTON_ID, PICKER_LABEL, show_picker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(playback: CastShowPlayback) -> CastShowState {
        CastShowState {
            playback,
            generation: 3,
            ..Default::default()
        }
    }

    fn upd(s: &mut CastShowState, u: CastShowUpdate) {
        let g = s.generation;
        apply_update(s, g, u);
    }

    #[test]
    fn happy_path_and_commands() {
        let mut s = CastShowState::default();
        prepare(&mut s);
        assert_eq!(take_command(&s), Some((CastCommand::Prepare, 1)));
        mark_command_taken(&mut s, 1);
        upd(&mut s, CastShowUpdate::Playback(CastShowPlayback::Ready));
        play(&mut s);
        assert_eq!(s.playback, CastShowPlayback::Starting);
        upd(&mut s, CastShowUpdate::Playback(CastShowPlayback::Playing));
        upd(
            &mut s,
            CastShowUpdate::Route(CastShowRoute::Connected(Some("x".into()))),
        );
        assert_eq!(route_text(&s), format!("{CONNECTED_PREFIX}x"));
        stop(&mut s);
        assert_eq!(s.playback, CastShowPlayback::Stopping);
        upd(&mut s, CastShowUpdate::Playback(CastShowPlayback::Stopped));
        assert_eq!(status_text(&s), STOPPED_STATUS);
        assert_eq!(error_text(&s), "");
        play(&mut s);
        assert_eq!(s.playback, CastShowPlayback::Starting);
    }

    #[test]
    fn stale_generation_update_is_dropped() {
        let mut s = at(CastShowPlayback::Starting);
        apply_update(
            &mut s,
            2,
            CastShowUpdate::Playback(CastShowPlayback::Playing),
        );
        apply_update(&mut s, 2, CastShowUpdate::Failed(CastShowFailure::Blocked));
        assert_eq!(s.playback, CastShowPlayback::Starting);
    }

    #[test]
    fn connected_after_expire_is_dropped() {
        let mut s = at(CastShowPlayback::Starting);
        expire(&mut s, 3);
        assert_eq!(
            s.playback,
            CastShowPlayback::Failed(CastShowFailure::Timeout)
        );
        // 世代が進むので旧世代の通知は捨てる。同世代で届いても失敗中は経路を受けない
        apply_update(
            &mut s,
            3,
            CastShowUpdate::Route(CastShowRoute::Connected(None)),
        );
        upd(
            &mut s,
            CastShowUpdate::Route(CastShowRoute::Connected(None)),
        );
        assert_eq!(s.route, CastShowRoute::Local);
    }

    #[test]
    fn connecting_expires_to_timeout() {
        let mut s = at(CastShowPlayback::Playing);
        s.route = CastShowRoute::Connecting;
        expire(&mut s, 3);
        assert_eq!(
            s.playback,
            CastShowPlayback::Failed(CastShowFailure::Timeout)
        );
        let mut s = at(CastShowPlayback::Playing);
        expire(&mut s, 3);
        assert_eq!(s.playback, CastShowPlayback::Playing);
    }

    #[test]
    fn picker_cancel_returns_to_route_without_failure() {
        let mut s = at(CastShowPlayback::Playing);
        upd(&mut s, CastShowUpdate::Route(CastShowRoute::Choosing));
        assert_eq!(route_text(&s), CHOOSING_STATUS);
        upd(&mut s, CastShowUpdate::Route(CastShowRoute::Local));
        assert_eq!(s.playback, CastShowPlayback::Playing);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn unsupported_rejects_prepare_and_play() {
        let mut s = at(CastShowPlayback::Failed(CastShowFailure::Unsupported));
        prepare(&mut s);
        play(&mut s);
        show_picker(&mut s);
        assert_eq!(s.command, None);
        assert_eq!(s.generation, 3);
        assert_eq!(error_text(&s), UNSUPPORTED_STATUS);
    }

    #[test]
    fn failure_other_than_unsupported_retries_prepare() {
        let mut s = at(CastShowPlayback::Failed(CastShowFailure::Blocked));
        play(&mut s);
        assert_eq!(s.playback, CastShowPlayback::Preparing);
        assert_eq!(s.command, Some(CastCommand::Prepare));
    }

    #[test]
    fn native_picker_issues_no_command_and_shows_guide() {
        let mut s = at(CastShowPlayback::Ready);
        upd(&mut s, CastShowUpdate::Picker(PickerView::Native));
        show_picker(&mut s);
        assert_eq!(s.command, None);
        assert_eq!(route_text(&s), NATIVE_PICKER_GUIDE);
        let mut s = at(CastShowPlayback::Ready);
        show_picker(&mut s);
        assert_eq!(s.command, Some(CastCommand::ShowPicker));
    }

    #[test]
    fn picker_during_starting_is_ignored() {
        let mut s = at(CastShowPlayback::Ready);
        play(&mut s);
        let g = s.generation;
        show_picker(&mut s);
        assert_eq!(s.generation, g);
        assert_eq!(s.command, Some(CastCommand::Play));
    }

    #[test]
    fn busy_route_ignores_second_picker_press() {
        let mut s = at(CastShowPlayback::Ready);
        s.route = CastShowRoute::Choosing;
        show_picker(&mut s);
        assert_eq!(s.command, None);
    }

    #[test]
    fn double_stop_is_ignored() {
        let mut s = at(CastShowPlayback::Playing);
        stop(&mut s);
        let g = s.generation;
        stop(&mut s);
        assert_eq!(s.playback, CastShowPlayback::Stopping);
        assert_eq!(s.generation, g);
    }

    #[test]
    fn stop_cancels_unsent_play_straight_to_stopped() {
        let mut s = at(CastShowPlayback::Ready);
        play(&mut s);
        stop(&mut s);
        assert_eq!(s.playback, CastShowPlayback::Stopped);
        assert_eq!(take_command(&s), None);
    }

    #[test]
    fn connected_while_not_playing_is_not_called_playing() {
        let mut s = at(CastShowPlayback::Ready);
        s.route = CastShowRoute::Connected(Some("x".into()));
        assert_eq!(route_text(&s), format!("{ROUTE_PREFIX}x"));
        assert!(!route_text(&s).contains("再生中"));
    }

    #[test]
    fn route_is_not_shown_for_unconfirmed_names() {
        let mut s = at(CastShowPlayback::Playing);
        s.route = CastShowRoute::Connected(None);
        assert_eq!(route_text(&s), CONNECTED_NO_NAME_STATUS);
        s.route = CastShowRoute::NotReported;
        assert_eq!(route_text(&s), NOT_REPORTED_STATUS);
        assert!(!route_text(&s).contains(CONNECTED_PREFIX));
    }
}
