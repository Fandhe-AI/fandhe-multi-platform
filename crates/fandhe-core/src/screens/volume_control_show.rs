//! Phase 11 の拡充ループ型サンプル「システム音量」（親 #166、Issue #785）。
//! 既定の出力の音量と消音状態を表示し、「＋」「－」で 1 段階ずつ変える 1 画面。
//! `FANDHE_PROXY_APP=volume_control_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod volume_control_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS への要求と読み戻しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/volume_control.rs`）が [`take_step`] / [`wants_read`] で観測し、[`begin`] →
//! [`finish_step`] / [`apply_read`] / [`expire`] で結果を注入する。
//!
//! # 嘘の成功を避ける
//!
//! 表示は常に OS の読み戻しで決め、要求値を写さない。「変わらなかった」（上限・下限）は失敗に
//! しない。変えられない環境（音量固定・音量を持たない出力機器・iOS）では「＋」「－」を出さない。
//! 古い世代の完了と、期限後に届いた完了は ticket の不一致で丸ごと捨てる。連打は最新 1 件に畳み、
//! OS 呼び出しの最中に押された操作は捨てずに保留して、完了後に取り出す。
//!
//! # OS ごとの違い（全対象 OS 分をここで定義する。後続の OS 実装はこの画面を変えない）
//!
//! - iOS: 音量は OS のスライダーで変える。[`OS_SLIDER_NOTE`] を出し、ボタンは出さない
//! - Android: おやすみモードで変更が拒否されたら [`PERMISSION_DENIED_REASON`] を出す
//! - 音量を持たない出力機器・音量固定: [`NOT_CHANGEABLE_STATUS`]、音量を読めないときは
//!   [`NO_LEVEL_STATUS`]

use crate::{Screen, WidgetId};

/// 「＋」ボタンの `WidgetId`。
pub const UP_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「－」ボタンの `WidgetId`。
pub const DOWN_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「読み直す」ボタンの `WidgetId`。
pub const RELOAD_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// 音量表示の接頭辞（後ろに `{n}%`）。
pub const LEVEL_PREFIX: &str = "音量: ";
/// 消音中の案内。
pub const MUTED_STATUS: &str = "消音中";
/// 消音していないときの案内。
pub const NOT_MUTED_STATUS: &str = "消音していません";
/// 初回の読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では音量を扱えません";
/// 音量固定・音量を持たない出力機器の案内。
pub const NOT_CHANGEABLE_STATUS: &str = "この出力では音量を変えられません";
/// 音量を読めない出力機器の案内。
pub const NO_LEVEL_STATUS: &str = "この出力の音量は読み取れません";
/// iOS の案内（音量は OS のスライダーで変える）。
pub const OS_SLIDER_NOTE: &str = "音量は端末の音量スライダーや音量ボタンで変えてください";
/// 上限で変わらなかったときの案内（失敗ではない）。
pub const AT_MAX_NOTE: &str = "これ以上は上げられません";
/// 下限で変わらなかったときの案内（失敗ではない）。
pub const AT_MIN_NOTE: &str = "これ以上は下げられません";
/// 上限・下限と読み取れないまま変わらなかったときの案内（理由は言わない）。
pub const UNCHANGED_NOTE: &str = "音量が変わりませんでした";
/// 上限・下限とみなす読み戻しの許容幅。
const LIMIT_EPSILON: f32 = 1e-4;
/// Android のおやすみモードで変更が拒否されたときの理由。
pub const PERMISSION_DENIED_REASON: &str =
    "おやすみモードのため音量を変えられません。設定でおやすみモードを確認してください";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// 期限切れの理由。
pub const TIMEOUT_REASON: &str = "時間内に完了しませんでした";
/// 前の操作が終わっていないときの理由。
pub const BUSY_REASON: &str = "前の操作が終わっていません";
/// 音量の操作の失敗理由。
pub const OPERATION_REASON: &str = "音量の操作";
/// 読み取りに失敗したときの案内。
pub const READ_FAILED_STATUS: &str = "音量を読み取れませんでした";
/// 「＋」ボタンのラベル。
pub const UP_LABEL: &str = "＋";
/// 「－」ボタンのラベル。
pub const DOWN_LABEL: &str = "－";
/// 「読み直す」ボタンのラベル。
pub const RELOAD_LABEL: &str = "読み直す";

/// 1 段階の向き（`fandhe_device::volume_control::StepDirection` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDir {
    /// 上げる。
    Up,
    /// 下げる。
    Down,
}

/// 音量の変え方（`VolumeControlMode` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMode {
    /// アプリから 1 段階ずつ変えられる。
    Steps,
    /// OS の UI でだけ変えられる（iOS）。
    OsSlider,
    /// 変えられない。
    NotChangeable,
}

/// OS の読み戻し（`VolumeReading` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// 現在の音量（0.0〜1.0）。読めない出力は `None`。
    pub level: Option<f32>,
    /// 消音中か。読めない OS・機器は `None`。
    pub muted: Option<bool>,
    /// 変え方。
    pub mode: ControlMode,
}

/// `step` の結果（`StepOutcome` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepResult {
    /// 値が変わった。
    Changed(Reading),
    /// 上限・下限で変わらなかった。
    Unchanged(Reading),
    /// 変えられない環境だった。
    NotChangeable(Reading),
}

/// 失敗の種別（`fandhe-egui-proxy` が `DeviceError` から写す。OS の詳細は載せない）。
#[derive(Debug, Clone, PartialEq)]
pub enum VolumeFailure {
    /// この環境では使えない。
    Unsupported,
    /// 権限拒否（Android のおやすみモード）。
    PermissionDenied,
    /// その他の失敗（固定文言のみ）。
    Failed(String),
}

/// OS への要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// 読み取り。
    Read,
    /// 1 段階の変更。
    Step(StepDir),
}

/// 読み取り状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ReadState {
    /// まだ読んでいない。
    #[default]
    Unread,
    /// 読めた。
    Read(Reading),
    /// この環境では使えない（失敗ではない）。
    Unsupported,
    /// 読み取りに失敗した（読み直しを止めている）。
    Failed,
}

/// 「システム音量」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VolumeControlShowState {
    read: ReadState,
    /// 押されたがまだ OS へ渡していない向き（最新 1 件）。
    pending_step: Option<StepDir>,
    /// OS へ渡して完了待ちの要求（ticket と種類）。
    outstanding: Option<(u64, OpKind)>,
    /// 最後に発行した ticket。
    ticket: u64,
    error: String,
    notice: String,
}

impl VolumeControlShowState {
    /// 読み取り状態。
    pub fn read_state(&self) -> &ReadState {
        &self.read
    }

    /// 最後に発行した ticket。次の要求は `ticket() + 1` を使う。
    pub fn ticket(&self) -> u64 {
        self.ticket
    }

    /// 保留中の向き。
    pub fn pending_step(&self) -> Option<StepDir> {
        self.pending_step
    }

    /// OS 呼び出しの完了待ちか。
    pub fn is_busy(&self) -> bool {
        self.outstanding.is_some()
    }

    fn can_step(&self) -> bool {
        matches!(&self.read, ReadState::Read(r)
            if r.mode == ControlMode::Steps && r.level.is_some())
    }
}

fn percent(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 100.0).round() as u32
}

fn press(state: &mut VolumeControlShowState, dir: StepDir) {
    if !state.can_step() {
        return;
    }
    // 連打は最新 1 件に畳む。OS 呼び出しの最中でも捨てずに保留する。
    state.pending_step = Some(dir);
    state.error.clear();
    state.notice.clear();
}

/// 「＋」。
pub fn press_up(state: &mut VolumeControlShowState) {
    press(state, StepDir::Up);
}

/// 「－」。
pub fn press_down(state: &mut VolumeControlShowState) {
    press(state, StepDir::Down);
}

/// 「読み直す」。失敗後に読み取りをやり直す。
pub fn reload(state: &mut VolumeControlShowState) {
    if state.read == ReadState::Failed {
        state.read = ReadState::Unread;
        state.error.clear();
    }
}

/// OS 呼び出し中でなければ、保留中の向きを状態を変えずに返す。
pub fn take_step(state: &VolumeControlShowState) -> Option<StepDir> {
    if state.outstanding.is_some() {
        return None;
    }
    state.pending_step
}

/// driver が OS を読み直すべきか。非対応・失敗では偽（失敗状態で再描画し続けない）。
pub fn wants_read(state: &VolumeControlShowState) -> bool {
    matches!(state.read, ReadState::Unread | ReadState::Read(_))
}

/// OS へ渡す直前に呼ぶ。ticket を記録し、取り出した保留を取り除く。
pub fn begin(state: &mut VolumeControlShowState, ticket: u64, kind: OpKind) {
    state.ticket = ticket;
    state.outstanding = Some((ticket, kind));
    if matches!(kind, OpKind::Step(_)) {
        state.pending_step = None;
    }
}

fn take_outstanding(
    state: &mut VolumeControlShowState,
    ticket: u64,
    is_read: bool,
) -> Option<OpKind> {
    match state.outstanding {
        // 種類の違う完了（読み取りの ticket で変更を閉じる等）は outstanding を壊さず捨てる。
        Some((t, kind)) if t == ticket && (matches!(kind, OpKind::Read) == is_read) => {
            state.outstanding = None;
            Some(kind)
        }
        // 古い世代・期限後に届いた完了は丸ごと捨てる。
        _ => None,
    }
}

fn failure_text(f: &VolumeFailure) -> String {
    match f {
        VolumeFailure::PermissionDenied => PERMISSION_DENIED_REASON.to_string(),
        VolumeFailure::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        VolumeFailure::Unsupported => String::new(),
    }
}

/// 変更の完了を反映する。ticket が合わない完了は捨てる。表示は読み戻しで決める。
pub fn finish_step(
    state: &mut VolumeControlShowState,
    ticket: u64,
    result: Result<StepResult, VolumeFailure>,
) {
    let Some(OpKind::Step(dir)) = take_outstanding(state, ticket, false) else {
        return;
    };
    match result {
        Ok(StepResult::Changed(r)) => {
            state.read = ReadState::Read(r);
            state.error.clear();
            state.notice.clear();
        }
        Ok(StepResult::Unchanged(r)) => {
            state.read = ReadState::Read(r);
            state.error.clear();
            // 理由は押した向きではなく読み戻した値だけから選ぶ（粗い刻みの機器が
            // 中間値へ丸め戻した場合に「上限」と偽らない）。値が無ければ理由を言わない。
            state.notice = match (dir, r.level) {
                (StepDir::Up, Some(l)) if l >= 1.0 - LIMIT_EPSILON => AT_MAX_NOTE,
                (StepDir::Down, Some(l)) if l <= LIMIT_EPSILON => AT_MIN_NOTE,
                _ => UNCHANGED_NOTE,
            }
            .to_string();
        }
        Ok(StepResult::NotChangeable(mut r)) => {
            if r.mode == ControlMode::Steps {
                r.mode = ControlMode::NotChangeable;
            }
            state.read = ReadState::Read(r);
            state.error.clear();
            state.notice.clear();
            state.pending_step = None;
        }
        Err(VolumeFailure::Unsupported) => {
            state.read = ReadState::Unsupported;
            state.pending_step = None;
        }
        // 失敗後も直前の読み戻しは残し、次の読み取りで OS の値へ揃える。
        Err(f) => {
            state.error = failure_text(&f);
            state.notice.clear();
        }
    }
}

/// 読み取りの完了を反映する。ticket が合わない完了は捨てる。
pub fn apply_read(
    state: &mut VolumeControlShowState,
    ticket: u64,
    result: Result<Reading, VolumeFailure>,
) {
    if !matches!(take_outstanding(state, ticket, true), Some(OpKind::Read)) {
        return;
    }
    match result {
        Ok(r) => {
            if r.mode != ControlMode::Steps {
                state.pending_step = None;
            }
            // 上限・下限などの案内は、その時の読み戻し（音量・変え方）に対してだけ有効。
            // OS 側で音量が変わっていたら古い案内を残さない。
            let same_reading = matches!(&state.read, ReadState::Read(old)
                if old.level == r.level && old.mode == r.mode);
            if !same_reading {
                state.notice.clear();
            }
            state.read = ReadState::Read(r);
        }
        Err(VolumeFailure::Unsupported) => {
            state.read = ReadState::Unsupported;
            state.pending_step = None;
            state.notice.clear();
        }
        Err(f) => {
            state.read = ReadState::Failed;
            state.pending_step = None;
            state.notice.clear();
            state.error = match f {
                VolumeFailure::PermissionDenied => PERMISSION_DENIED_REASON.to_string(),
                _ => READ_FAILED_STATUS.to_string(),
            };
        }
    }
}

/// 期限切れを確定する。変更は失敗として表示を残し、読み取りは読み直しを止める。
/// 期限後に届く完了は ticket が外れているため [`finish_step`] / [`apply_read`] が捨てる。
pub fn expire(state: &mut VolumeControlShowState, ticket: u64) {
    let kind = match state.outstanding {
        Some((t, kind)) if t == ticket => kind,
        _ => return,
    };
    state.outstanding = None;
    match kind {
        OpKind::Step(_) => {
            state.error = format!("{FAILED_PREFIX}{TIMEOUT_REASON}");
            state.notice.clear();
        }
        OpKind::Read => {
            state.read = ReadState::Failed;
            state.pending_step = None;
            state.notice.clear();
            state.error = format!("{FAILED_PREFIX}{TIMEOUT_REASON}");
        }
    }
}

/// 音量の案内。読み戻しの値だけから作る。
pub fn level_text(state: &VolumeControlShowState) -> String {
    match &state.read {
        ReadState::Unread => READING_STATUS.to_string(),
        ReadState::Unsupported => UNSUPPORTED_STATUS.to_string(),
        ReadState::Failed => READ_FAILED_STATUS.to_string(),
        ReadState::Read(r) => match r.level {
            Some(l) => format!("{LEVEL_PREFIX}{}%", percent(l)),
            None => NO_LEVEL_STATUS.to_string(),
        },
    }
}

/// 消音状態の案内。読めない OS・機器では出さない。
pub fn mute_text(state: &VolumeControlShowState) -> String {
    match &state.read {
        ReadState::Read(r) => match r.muted {
            Some(true) => MUTED_STATUS.to_string(),
            Some(false) => NOT_MUTED_STATUS.to_string(),
            None => String::new(),
        },
        _ => String::new(),
    }
}

/// 変え方の案内（iOS の OS スライダー、変えられない出力）。
pub fn mode_text(state: &VolumeControlShowState) -> String {
    match &state.read {
        ReadState::Read(r) => match r.mode {
            ControlMode::Steps => String::new(),
            ControlMode::OsSlider => OS_SLIDER_NOTE.to_string(),
            ControlMode::NotChangeable => NOT_CHANGEABLE_STATUS.to_string(),
        },
        _ => String::new(),
    }
}

/// 変わらなかったときの案内。
pub fn notice_text(state: &VolumeControlShowState) -> String {
    state.notice.clone()
}

/// エラーの案内。
pub fn error_text(state: &VolumeControlShowState) -> String {
    state.error.clone()
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
/// 変えられる環境で読み取り済みのときだけ「＋」「－」を出す。
pub fn current_screen(state: &VolumeControlShowState) -> Screen<VolumeControlShowState> {
    let mut screen = Screen::new("システム音量")
        .dynamic_text(level_text)
        .dynamic_text(mute_text)
        .dynamic_text(mode_text)
        .dynamic_text(notice_text)
        .error_text(error_text);
    if state.can_step() {
        screen = screen.button(UP_BUTTON_ID, UP_LABEL, press_up).button(
            DOWN_BUTTON_ID,
            DOWN_LABEL,
            press_down,
        );
    }
    if state.read == ReadState::Failed {
        screen = screen.button(RELOAD_BUTTON_ID, RELOAD_LABEL, reload);
    }
    screen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(level: f32) -> Reading {
        Reading {
            level: Some(level),
            muted: Some(false),
            mode: ControlMode::Steps,
        }
    }

    /// 読み取り済み（Steps）の状態にして返す。
    fn ready(level: f32) -> VolumeControlShowState {
        let mut s = VolumeControlShowState::default();
        let t = s.ticket() + 1;
        begin(&mut s, t, OpKind::Read);
        apply_read(&mut s, t, Ok(steps(level)));
        assert!(matches!(s.read_state(), ReadState::Read(_)));
        s
    }

    fn issue_step(s: &mut VolumeControlShowState, dir: StepDir) -> u64 {
        let t = s.ticket() + 1;
        begin(s, t, OpKind::Step(dir));
        t
    }

    #[test]
    fn stale_ticket_completion_is_dropped() {
        let mut s = ready(0.5);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        let before = s.clone();
        finish_step(&mut s, t + 7, Ok(StepResult::Changed(steps(0.9))));
        assert_eq!(s, before);
        assert!(s.is_busy());
    }

    #[test]
    fn completion_after_expire_is_dropped() {
        let mut s = ready(0.5);
        let t = issue_step(&mut s, StepDir::Up);
        expire(&mut s, t);
        assert!(!s.is_busy());
        assert!(error_text(&s).contains(TIMEOUT_REASON));
        let before = s.clone();
        finish_step(&mut s, t, Ok(StepResult::Changed(steps(0.9))));
        assert_eq!(s, before, "期限後の完了は成功にならない");
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}50%"));
    }

    #[test]
    fn read_expire_stops_polling() {
        let mut s = VolumeControlShowState::default();
        let t = s.ticket() + 1;
        begin(&mut s, t, OpKind::Read);
        expire(&mut s, t);
        assert_eq!(s.read_state(), &ReadState::Failed);
        assert!(!wants_read(&s));
        reload(&mut s);
        assert!(wants_read(&s));
        assert_eq!(s.read_state(), &ReadState::Unread);
    }

    #[test]
    fn unchanged_is_not_a_failure() {
        let mut s = ready(1.0);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        finish_step(&mut s, t, Ok(StepResult::Unchanged(steps(1.0))));
        assert_eq!(error_text(&s), "");
        assert_eq!(notice_text(&s), AT_MAX_NOTE);
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}100%"));
        press_down(&mut s);
        let t = issue_step(&mut s, StepDir::Down);
        finish_step(&mut s, t, Ok(StepResult::Unchanged(steps(0.0))));
        assert_eq!(notice_text(&s), AT_MIN_NOTE);
    }

    #[test]
    fn notice_is_dropped_when_readback_changes() {
        let mut s = ready(1.0);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        finish_step(&mut s, t, Ok(StepResult::Unchanged(steps(1.0))));
        assert_eq!(notice_text(&s), AT_MAX_NOTE);
        // 同じ読み戻しでは案内を残す。
        let t = s.ticket() + 1;
        begin(&mut s, t, OpKind::Read);
        apply_read(&mut s, t, Ok(steps(1.0)));
        assert_eq!(notice_text(&s), AT_MAX_NOTE);
        // OS 側で 50% に変わったら案内を消す。
        let t = s.ticket() + 1;
        begin(&mut s, t, OpKind::Read);
        apply_read(&mut s, t, Ok(steps(0.5)));
        assert_eq!(notice_text(&s), "");
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}50%"));
    }

    #[test]
    fn unchanged_at_midlevel_or_unknown_does_not_claim_limit() {
        let mut s = ready(0.5);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        finish_step(&mut s, t, Ok(StepResult::Unchanged(steps(0.5))));
        assert_eq!(notice_text(&s), UNCHANGED_NOTE);
        press_down(&mut s);
        let t = issue_step(&mut s, StepDir::Down);
        finish_step(&mut s, t, Ok(StepResult::Unchanged(steps(0.5))));
        assert_eq!(notice_text(&s), UNCHANGED_NOTE);
        // 上限で「－」、下限で「＋」の Unchanged も限界の理由にしない。
        press_down(&mut s);
        let t = issue_step(&mut s, StepDir::Down);
        finish_step(&mut s, t, Ok(StepResult::Unchanged(steps(1.0))));
        assert_eq!(notice_text(&s), UNCHANGED_NOTE);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        let none = Reading {
            level: None,
            muted: Some(false),
            mode: ControlMode::Steps,
        };
        finish_step(&mut s, t, Ok(StepResult::Unchanged(none)));
        assert_eq!(notice_text(&s), UNCHANGED_NOTE);
    }

    #[test]
    fn changed_shows_readback_and_clears_messages() {
        let mut s = ready(0.5);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        finish_step(&mut s, t, Ok(StepResult::Changed(steps(0.5625))));
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}56%"));
        assert_eq!(error_text(&s), "");
        assert_eq!(notice_text(&s), "");
    }

    #[test]
    fn buttons_hidden_unless_steps_mode() {
        let hidden = |r: Reading| {
            let mut s = VolumeControlShowState::default();
            begin(&mut s, 1, OpKind::Read);
            apply_read(&mut s, 1, Ok(r));
            press_up(&mut s);
            assert_eq!(s.pending_step(), None, "変えられない環境では要求を立てない");
            assert!(!s.can_step());
            s
        };
        let os = hidden(Reading {
            level: Some(0.4),
            muted: None,
            mode: ControlMode::OsSlider,
        });
        assert_eq!(mode_text(&os), OS_SLIDER_NOTE);
        assert_eq!(mute_text(&os), "");
        let fixed = hidden(Reading {
            level: None,
            muted: None,
            mode: ControlMode::NotChangeable,
        });
        assert_eq!(mode_text(&fixed), NOT_CHANGEABLE_STATUS);
        assert_eq!(level_text(&fixed), NO_LEVEL_STATUS);
    }

    #[test]
    fn not_changeable_result_hides_buttons() {
        let mut s = ready(0.5);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        finish_step(
            &mut s,
            t,
            Ok(StepResult::NotChangeable(Reading {
                level: Some(0.5),
                muted: Some(false),
                mode: ControlMode::Steps,
            })),
        );
        assert!(!s.can_step());
        assert_eq!(mode_text(&s), NOT_CHANGEABLE_STATUS);
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn permission_denied_has_its_own_reason() {
        let mut s = ready(0.5);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        finish_step(&mut s, t, Err(VolumeFailure::PermissionDenied));
        assert_eq!(error_text(&s), PERMISSION_DENIED_REASON);
        // 直前の読み戻しは残る。
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}50%"));
    }

    #[test]
    fn step_failure_is_not_success() {
        let mut s = ready(0.5);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        finish_step(
            &mut s,
            t,
            Err(VolumeFailure::Failed(OPERATION_REASON.to_string())),
        );
        assert!(error_text(&s).starts_with(FAILED_PREFIX));
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}50%"));
    }

    #[test]
    fn read_failure_stops_polling_and_reload_resumes() {
        let mut s = VolumeControlShowState::default();
        begin(&mut s, 1, OpKind::Read);
        apply_read(
            &mut s,
            1,
            Err(VolumeFailure::Failed(OPERATION_REASON.to_string())),
        );
        assert_eq!(s.read_state(), &ReadState::Failed);
        assert!(!wants_read(&s));
        assert_eq!(error_text(&s), READ_FAILED_STATUS);
        reload(&mut s);
        assert!(wants_read(&s));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn press_during_busy_is_kept_and_taken_after_completion() {
        let mut s = ready(0.5);
        press_up(&mut s);
        let t = issue_step(&mut s, StepDir::Up);
        assert_eq!(s.pending_step(), None, "begin が保留を取り除く");
        press_down(&mut s);
        press_up(&mut s);
        press_down(&mut s);
        assert_eq!(take_step(&s), None, "処理中は取り出さない");
        finish_step(&mut s, t, Ok(StepResult::Changed(steps(0.5625))));
        assert_eq!(take_step(&s), Some(StepDir::Down), "連打は最新 1 件");
    }

    #[test]
    fn unsupported_builds_no_request() {
        let mut s = VolumeControlShowState::default();
        begin(&mut s, 1, OpKind::Read);
        apply_read(&mut s, 1, Err(VolumeFailure::Unsupported));
        assert_eq!(s.read_state(), &ReadState::Unsupported);
        assert!(!wants_read(&s));
        press_up(&mut s);
        assert_eq!(take_step(&s), None);
        assert_eq!(level_text(&s), UNSUPPORTED_STATUS);
    }

    #[test]
    fn read_ticket_cannot_complete_a_step() {
        let mut s = ready(0.5);
        let t = issue_step(&mut s, StepDir::Up);
        // 種類の違う完了（読み取り）は step の outstanding を壊さない。
        apply_read(&mut s, t, Ok(steps(0.9)));
        assert!(s.is_busy());
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}50%"));
    }

    #[test]
    fn mute_text_follows_readback() {
        let mut s = ready(0.5);
        assert_eq!(mute_text(&s), NOT_MUTED_STATUS);
        begin(&mut s, 9, OpKind::Read);
        apply_read(
            &mut s,
            9,
            Ok(Reading {
                level: Some(0.5),
                muted: Some(true),
                mode: ControlMode::Steps,
            }),
        );
        assert_eq!(mute_text(&s), MUTED_STATUS);
    }
}
