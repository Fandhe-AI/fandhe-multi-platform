//! Phase 11 の拡充ループ（親 #134）型サンプル「ログイン時の自動起動」（Issue #696）。
//! `FANDHE_PROXY_APP=launch_at_login_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod launch_at_login_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS の呼び出しと
//! `fandhe_device::launch_at_login::LoginItemStatus` からの写像は `fandhe-egui-proxy` の
//! effect ドライバ（`device/launch_at_login.rs`）の責務で、本モジュールの [`LoginItemState`] は
//! その写し。macOS・Windows の両方で使う画面で、OS ごとの文言は [`Platform`]
//! （ドライバが一度だけ注入）で出し分ける。
//!
//! 要求の受け渡しは `url_scheme_show` と同型の [`Phase`] と世代番号。ボタン押下で
//! `Requested` にし、ドライバが `mark_in_flight` で二重発行を防いでから別スレッドで OS を呼び、
//! [`finish_action`] で反映する。世代が違う・要求が無いときの完了は捨てる。
//!
//! 成功を装わない: 承認待ち・利用者による無効化は「有効にしました」と表示しない。
//! 現在の状態の表示は、OS から読み戻した値で常に上書きする。

use crate::{Screen, WidgetId};

/// OS から読み戻した自動起動の状態の写し（`LoginItemStatus` と 1 対 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginItemState {
    /// まだ読み取っていない。
    #[default]
    Unread,
    /// 登録済みで有効（有効扱いはこれだけ）。
    Enabled,
    /// 未登録。
    NotRegistered,
    /// macOS の承認待ち、または利用者がシステム設定で無効化した。
    RequiresApproval,
    /// Windows の StartupApproved が無効。
    DisabledByUser,
    /// Windows で別の場所の実行体が登録されている。
    RegisteredElsewhere,
    /// 判別できない。
    Unknown,
}

/// 文言を出し分ける OS（ドライバが一度だけ注入する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Platform {
    /// 未注入。
    #[default]
    Unknown,
    /// macOS。
    MacOs,
    /// Windows。
    Windows,
}

/// ボタンに対応する操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 状態の読み直し（画面を開いた直後にも 1 回走る）。
    Refresh,
    /// 自動起動を有効にする。
    Enable,
    /// 自動起動を無効にする。
    Disable,
    /// OS の設定画面を開く。
    OpenSettings,
}

/// ボタン操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 要求済みで、ドライバへまだ渡していない。
    Requested(Action),
    /// ドライバが OS 呼び出しを処理中。
    InFlight,
}

/// ドライバへ渡す要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// [`finish_action`] へそのまま渡す世代番号。
    pub generation: u64,
    /// 実行する操作。
    pub action: Action,
}

/// 操作が成功したときの結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionOutcome {
    /// OS から読み戻した状態。
    Status(LoginItemState),
    /// 設定画面を開くよう依頼した（開いたことは確かめられない）。
    SettingsRequested,
}

/// 操作の失敗種別。「使えない」「拒否」「失敗」を分ける。
#[derive(Debug, Clone, PartialEq)]
pub enum ActionFailure {
    /// この環境では使えない（失敗ではない）。
    Unsupported,
    /// OS に拒否された。
    PermissionDenied,
    /// 試みて失敗した（利用者向けの完成した文言）。
    Failed(String),
}

/// 「ログイン時の自動起動」画面の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchAtLoginShowState {
    current: LoginItemState,
    platform: Platform,
    unsupported: bool,
    phase: Phase,
    generation: u64,
    status: String,
    error: String,
}

impl Default for LaunchAtLoginShowState {
    /// 画面を開いた直後に 1 回読み取る（追加の送信は要らない）。
    fn default() -> Self {
        Self {
            current: LoginItemState::Unread,
            platform: Platform::Unknown,
            unsupported: false,
            phase: Phase::Requested(Action::Refresh),
            generation: 1,
            status: String::new(),
            error: String::new(),
        }
    }
}

impl LaunchAtLoginShowState {
    /// 非対応として確定済みか。
    pub fn is_unsupported(&self) -> bool {
        self.unsupported
    }

    /// 注入済みの OS（未注入なら [`Platform::Unknown`]）。
    pub fn platform(&self) -> Platform {
        self.platform
    }
}

/// この OS を反映する（ドライバが一度だけ送る）。
pub fn apply_platform(state: &mut LaunchAtLoginShowState, platform: Platform) {
    state.platform = platform;
}

/// この OS では使えないことを反映する。
pub fn apply_unsupported(state: &mut LaunchAtLoginShowState) {
    state.unsupported = true;
}

fn request(state: &mut LaunchAtLoginShowState, action: Action) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「自動起動を有効にする」ボタンのハンドラ。処理中は no-op（二重要求の防止）。
pub fn enable(state: &mut LaunchAtLoginShowState) {
    request(state, Action::Enable);
}

/// 「自動起動を無効にする」ボタンのハンドラ。処理中は no-op。
pub fn disable(state: &mut LaunchAtLoginShowState) {
    request(state, Action::Disable);
}

/// 設定画面ボタンのハンドラ。処理中は no-op。
pub fn open_settings(state: &mut LaunchAtLoginShowState) {
    request(state, Action::OpenSettings);
}

/// 「状態を読み直す」ボタンのハンドラ。処理中は no-op。
pub fn refresh(state: &mut LaunchAtLoginShowState) {
    request(state, Action::Refresh);
}

/// ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &LaunchAtLoginShowState) -> Option<Request> {
    match state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action,
        }),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut LaunchAtLoginShowState) {
    if matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 操作結果を反映する。世代が違う、または要求が無いときの完了は捨てる。
/// 失敗は `error` にだけ入れ、成功文言を出さない。
pub fn finish_action(
    state: &mut LaunchAtLoginShowState,
    generation: u64,
    action: Action,
    result: Result<ActionOutcome, ActionFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    match result {
        Ok(ActionOutcome::SettingsRequested) => {
            state.status = "設定画面を開くよう依頼しました".to_string();
        }
        Ok(ActionOutcome::Status(now)) => {
            state.current = now;
            use LoginItemState::*;
            match (action, now) {
                (Action::Enable, Enabled) => state.status = "自動起動を有効にしました".to_string(),
                (Action::Enable, RequiresApproval | DisabledByUser) => {
                    state.status = "登録しましたが、まだ有効ではありません".to_string();
                }
                (Action::Enable, _) => state.error = "有効にできませんでした".to_string(),
                (Action::Disable, NotRegistered) => {
                    state.status = "自動起動を無効にしました".to_string();
                }
                (Action::Disable, RegisteredElsewhere) => {
                    state.status = "このアプリの登録はありません（別の場所の登録は消していません）"
                        .to_string();
                }
                (Action::Disable, _) => state.error = "無効にできませんでした".to_string(),
                // 読み直しと設定画面は現在の状態の上書きだけ。
                _ => {}
            }
        }
        Err(ActionFailure::Unsupported) => state.unsupported = true,
        Err(ActionFailure::PermissionDenied) => state.error = "OS に拒否されました".to_string(),
        Err(ActionFailure::Failed(message)) => state.error = message,
    }
}

/// 現在の状態の状態行。
pub fn state_text(state: &LaunchAtLoginShowState) -> String {
    if state.unsupported {
        return "この OS では未対応です".to_string();
    }
    match state.current {
        LoginItemState::Unread => "読み取り中",
        LoginItemState::Enabled => "自動起動: 有効です",
        LoginItemState::NotRegistered => "自動起動: 登録されていません",
        LoginItemState::RequiresApproval => "自動起動: 承認待ちまたは無効化されています",
        LoginItemState::DisabledByUser => "自動起動: 利用者が無効にしています",
        LoginItemState::RegisteredElsewhere => "自動起動: 別の場所の登録があります",
        LoginItemState::Unknown => "自動起動: 状態を判別できません",
    }
    .to_string()
}

/// 承認待ち・無効化のときの OS 別の案内（それ以外は空）。
pub fn guidance_text(state: &LaunchAtLoginShowState) -> String {
    if state.unsupported
        || !matches!(
            state.current,
            LoginItemState::RequiresApproval | LoginItemState::DisabledByUser
        )
    {
        return String::new();
    }
    match state.platform {
        Platform::MacOs => "システム設定 > 一般 > ログイン項目 で許可すると有効になります",
        Platform::Windows => "設定 > アプリ > スタートアップ で有効にできます",
        Platform::Unknown => "",
    }
    .to_string()
}

/// 直近の操作の結果文言（未操作・失敗なら空）。
pub fn result_text(state: &LaunchAtLoginShowState) -> String {
    state.status.clone()
}

/// 直近の操作の失敗文言（`TextRole::Error` で描画される）。
pub fn error_text(state: &LaunchAtLoginShowState) -> String {
    state.error.clone()
}

/// 設定画面ボタンのラベル（OS 別）。
fn settings_label(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "設定のスタートアップ アプリを開く",
        Platform::MacOs | Platform::Unknown => "システム設定のログイン項目を開く",
    }
}

const TOGGLE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const SETTINGS_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
const REFRESH_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 「ログイン時の自動起動」画面（文言のみ）。
pub fn launch_at_login_show_screen() -> Screen<LaunchAtLoginShowState> {
    Screen::new("ログイン時の自動起動")
        .dynamic_text(state_text)
        .dynamic_text(guidance_text)
        .dynamic_text(result_text)
        .error_text(error_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
/// 非対応か処理中（`Idle` 以外）の間はボタンを出さない。
pub fn current_screen(state: &LaunchAtLoginShowState) -> Screen<LaunchAtLoginShowState> {
    let screen = launch_at_login_show_screen();
    if state.unsupported || state.phase != Phase::Idle {
        return screen;
    }
    let screen = match state.current {
        LoginItemState::Enabled
        | LoginItemState::RequiresApproval
        | LoginItemState::DisabledByUser => {
            screen.button(TOGGLE_BUTTON_ID, "自動起動を無効にする", disable)
        }
        LoginItemState::Unread
        | LoginItemState::NotRegistered
        | LoginItemState::RegisteredElsewhere
        | LoginItemState::Unknown => {
            screen.button(TOGGLE_BUTTON_ID, "自動起動を有効にする", enable)
        }
    };
    screen
        .button(
            SETTINGS_BUTTON_ID,
            settings_label(state.platform),
            open_settings,
        )
        .button(REFRESH_BUTTON_ID, "状態を読み直す", refresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn run(
        state: &mut LaunchAtLoginShowState,
        start: fn(&mut LaunchAtLoginShowState),
        result: Result<ActionOutcome, ActionFailure>,
    ) {
        // 初期の読み取り要求を片付けてから操作する。
        if state.phase != Phase::Idle {
            let r = take_request(state).expect("要求がある");
            mark_in_flight(state);
            finish_action(
                state,
                r.generation,
                r.action,
                Ok(ActionOutcome::Status(LoginItemState::NotRegistered)),
            );
        }
        start(state);
        let r = take_request(state).expect("要求がある");
        mark_in_flight(state);
        finish_action(state, r.generation, r.action, result);
    }

    fn ok(s: LoginItemState) -> Result<ActionOutcome, ActionFailure> {
        Ok(ActionOutcome::Status(s))
    }

    fn lines(state: &LaunchAtLoginShowState) -> String {
        render_headless(&current_screen(state), state).join("\n")
    }

    #[test]
    fn initial_state_requests_one_refresh() {
        let s = LaunchAtLoginShowState::default();
        let r = take_request(&s).expect("初回読み取り");
        assert_eq!(r.action, Action::Refresh);
        assert_eq!(state_text(&s), "読み取り中");
        assert!(!lines(&s).contains("無効にする"));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = LaunchAtLoginShowState::default();
        let r = take_request(&s).unwrap();
        mark_in_flight(&mut s);
        finish_action(
            &mut s,
            r.generation + 1,
            Action::Enable,
            ok(LoginItemState::Enabled),
        );
        assert_eq!(s.phase, Phase::InFlight);
        assert_eq!(result_text(&s), "");
        assert_eq!(state_text(&s), "読み取り中");
    }

    #[test]
    fn completion_while_idle_is_ignored() {
        let mut s = LaunchAtLoginShowState::default();
        run(&mut s, enable, ok(LoginItemState::Enabled));
        assert_eq!(s.phase, Phase::Idle);
        let g = s.generation;
        finish_action(
            &mut s,
            g,
            Action::Disable,
            ok(LoginItemState::NotRegistered),
        );
        assert_eq!(s.current, LoginItemState::Enabled);
    }

    #[test]
    fn request_while_busy_is_noop() {
        let mut s = LaunchAtLoginShowState::default();
        let g = s.generation;
        enable(&mut s);
        disable(&mut s);
        open_settings(&mut s);
        refresh(&mut s);
        assert_eq!(s.generation, g);
        assert_eq!(s.phase, Phase::Requested(Action::Refresh));
    }

    #[test]
    fn enable_requires_approval_is_not_reported_as_enabled() {
        for st in [
            LoginItemState::RequiresApproval,
            LoginItemState::DisabledByUser,
        ] {
            let mut s = LaunchAtLoginShowState::default();
            run(&mut s, enable, ok(st));
            assert!(!result_text(&s).contains("有効にしました"));
            assert!(result_text(&s).contains("まだ有効ではありません"));
            assert_eq!(error_text(&s), "");
            assert!(!state_text(&s).contains("有効です"));
        }
    }

    #[test]
    fn enable_failure_states_are_errors() {
        for st in [
            LoginItemState::NotRegistered,
            LoginItemState::RegisteredElsewhere,
            LoginItemState::Unknown,
        ] {
            let mut s = LaunchAtLoginShowState::default();
            run(&mut s, enable, ok(st));
            assert_eq!(error_text(&s), "有効にできませんでした");
            assert_eq!(result_text(&s), "");
        }
    }

    #[test]
    fn enable_success_and_disable_outcomes() {
        let mut s = LaunchAtLoginShowState::default();
        run(&mut s, enable, ok(LoginItemState::Enabled));
        assert_eq!(result_text(&s), "自動起動を有効にしました");
        assert!(state_text(&s).contains("有効です"));
        run(&mut s, disable, ok(LoginItemState::NotRegistered));
        assert_eq!(result_text(&s), "自動起動を無効にしました");
        run(&mut s, disable, ok(LoginItemState::RegisteredElsewhere));
        assert!(result_text(&s).contains("別の場所の登録は消していません"));
        for st in [
            LoginItemState::Enabled,
            LoginItemState::RequiresApproval,
            LoginItemState::DisabledByUser,
        ] {
            run(&mut s, disable, ok(st));
            assert_eq!(error_text(&s), "無効にできませんでした");
            assert_eq!(result_text(&s), "");
        }
    }

    #[test]
    fn failures_never_leave_success_text() {
        let mut s = LaunchAtLoginShowState::default();
        run(&mut s, enable, Err(ActionFailure::PermissionDenied));
        assert_eq!(error_text(&s), "OS に拒否されました");
        assert_eq!(result_text(&s), "");
        run(
            &mut s,
            enable,
            Err(ActionFailure::Failed("失敗しました: x".into())),
        );
        assert_eq!(error_text(&s), "失敗しました: x");
        assert_eq!(result_text(&s), "");
    }

    #[test]
    fn unsupported_hides_buttons_and_shows_state() {
        let mut s = LaunchAtLoginShowState::default();
        run(&mut s, refresh, Err(ActionFailure::Unsupported));
        assert!(s.is_unsupported());
        assert_eq!(state_text(&s), "この OS では未対応です");
        assert_eq!(error_text(&s), "");
        let out = lines(&s);
        assert!(!out.contains("自動起動を"));
        assert!(!out.contains("読み直す"));
        let mut t = LaunchAtLoginShowState::default();
        apply_unsupported(&mut t);
        assert!(t.is_unsupported());
    }

    #[test]
    fn state_texts_are_distinct() {
        use LoginItemState::*;
        let all = [
            Unread,
            Enabled,
            NotRegistered,
            RequiresApproval,
            DisabledByUser,
            RegisteredElsewhere,
            Unknown,
        ];
        let texts: Vec<String> = all
            .iter()
            .map(|st| {
                let s = LaunchAtLoginShowState {
                    current: *st,
                    ..Default::default()
                };
                state_text(&s)
            })
            .collect();
        for i in 0..texts.len() {
            for j in 0..i {
                assert_ne!(texts[i], texts[j]);
            }
            if all[i] != Enabled {
                assert!(!texts[i].contains("有効です"));
            }
        }
    }

    #[test]
    fn platform_switches_label_and_guidance() {
        let mut s = LaunchAtLoginShowState::default();
        run(&mut s, refresh, ok(LoginItemState::RequiresApproval));
        apply_platform(&mut s, Platform::MacOs);
        assert_eq!(s.platform(), Platform::MacOs);
        let mac = lines(&s);
        assert!(mac.contains("システム設定のログイン項目を開く"));
        assert!(mac.contains("一般 > ログイン項目"));
        apply_platform(&mut s, Platform::Windows);
        let win = lines(&s);
        assert!(win.contains("設定のスタートアップ アプリを開く"));
        assert!(win.contains("アプリ > スタートアップ"));
        assert!(!win.contains("ログイン項目"));
    }

    #[test]
    fn toggle_button_follows_state() {
        let mut s = LaunchAtLoginShowState::default();
        run(&mut s, refresh, ok(LoginItemState::Enabled));
        assert!(lines(&s).contains("自動起動を無効にする"));
        run(&mut s, refresh, ok(LoginItemState::NotRegistered));
        assert!(lines(&s).contains("自動起動を有効にする"));
        enable(&mut s);
        assert!(!lines(&s).contains("自動起動を有効にする"));
    }

    #[test]
    fn settings_request_reports_request_only() {
        let mut s = LaunchAtLoginShowState::default();
        run(&mut s, open_settings, Ok(ActionOutcome::SettingsRequested));
        assert_eq!(result_text(&s), "設定画面を開くよう依頼しました");
    }
}
