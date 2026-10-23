//! Phase 11 の拡充ループ型サンプル「画面の明るさ」（親 #126、Issue #633）。
//! アプリが前面にいる間だけ、スライダーで画面輝度を上書きし、「元に戻す」で解除する 1 画面。
//! `FANDHE_PROXY_APP=screen_brightness_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod screen_brightness_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS への要求・読み戻しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/screen_brightness.rs`）が [`take_request`] で要求を取り、[`mark_applying`] →
//! [`finish`] / [`apply_read`] で結果を注入する（`wake_lock_toggle` と同型）。
//!
//! # 嘘の成功を避ける
//!
//! 「上書き中」の表示は、OS の読み戻し（[`Reading::overriding`]）が真のときだけ出す。要求が
//! 成功を返しても読み戻しが食い違えば失敗として扱い、失敗したときはスライダーを直前の確定値へ
//! 戻す。非対応（macOS・Windows・Linux 等）では操作 UI を出さず、要求も立てない。
//!
//! # OS ごとの違い（全対象 OS 分をここで定義する。後続の OS 実装はこの画面を変えない）
//!
//! - iOS: 利用者が輝度を変えると OS 側の上書きが外れる。上書き中から非上書きへ変わり、自分が
//!   解除を出していないときは [`RELEASED_BY_USER_NOTE`] を出す
//! - Android: 上書きしていない間は現在値を出さない（[`Reading::level`] が `None`）ので
//!   [`LEVEL_HIDDEN_NOTE`] を出す

use crate::{Screen, WidgetId};

/// 明るさスライダーの `WidgetId`。
pub const SLIDER_ID: WidgetId = WidgetId::from_raw(1);
/// 「元に戻す」ボタンの `WidgetId`。
pub const CLEAR_BUTTON_ID: WidgetId = WidgetId::from_raw(2);
/// 「読み直す」ボタンの `WidgetId`。
pub const RELOAD_BUTTON_ID: WidgetId = WidgetId::from_raw(3);

/// スライダーの下限（真っ暗で操作不能になるのを避ける。親 #126 の決定）。
pub const SLIDER_MIN: f32 = 0.1;
/// スライダーの上限。
pub const SLIDER_MAX: f32 = 1.0;
/// スライダーの刻み。
pub const LEVEL_STEP: f32 = 0.01;

/// 上書き中の案内の接頭辞（後ろに `{n}%`）。
pub const OVERRIDING_PREFIX: &str = "明るさを上書きしています: ";
/// 上書きしていないときの案内。
pub const NOT_OVERRIDING_STATUS: &str = "明るさを上書きしていません";
/// 現在値の接頭辞。
pub const CURRENT_LEVEL_PREFIX: &str = "現在の明るさ: ";
/// 現在値を出さない OS（Android の非上書き中）向けの案内。
pub const LEVEL_HIDDEN_NOTE: &str = "上書きしていない間の明るさは、この端末では表示しません";
/// 端末側の操作で上書きが外れたとき（iOS）の案内。
pub const RELEASED_BY_USER_NOTE: &str = "端末側で明るさが変わったため、上書きをやめました";
/// 非対応環境の案内。
pub const UNSUPPORTED_STATUS: &str = "この端末では画面の明るさを変えられません";
/// 範囲外の値に対する案内。
pub const INVALID_LEVEL_REASON: &str = "明るさは 0 から 100% の範囲で指定してください";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// 要求が成功を返したのに上書きが効いていないときの理由。
pub const NOT_APPLIED_REASON: &str = "上書きが効いていません";
/// 解除が成功を返したのに上書きが残っているときの理由。
pub const NOT_CLEARED_REASON: &str = "上書きを解除できていません";
/// 操作は成功を返したが、読み戻しに失敗して結果を確認できないときの理由。
pub const READBACK_FAILED_REASON: &str = "結果を確認できませんでした";
/// 読み戻した明るさが要求と食い違うときの理由。
pub const LEVEL_MISMATCH_REASON: &str = "要求した明るさになっていません";
/// 要求値と読み戻し値の許容誤差（OS 側の量子化・丸めを吸収する）。
pub const LEVEL_TOLERANCE: f32 = 0.05;
/// 初回の読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// スライダーのラベル。
pub const SLIDER_LABEL: &str = "明るさ";
/// 「元に戻す」ボタンのラベル。
pub const CLEAR_LABEL: &str = "元に戻す";
/// 「読み直す」ボタンのラベル。
pub const RELOAD_LABEL: &str = "読み直す";

/// OS への要求の種類。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrightnessCommand {
    /// 輝度を上書きする（0.0〜1.0）。
    Set(f32),
    /// 上書きを解く。
    Clear,
}

/// effect ドライバが取り出す要求 1 件。`generation` は完了通知の照合に使う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrightnessRequest {
    /// 要求の世代。
    pub generation: u64,
    /// 要求の内容。
    pub command: BrightnessCommand,
}

/// 失敗の種別（`fandhe-egui-proxy` が `DeviceError` から写す。OS の詳細は載せない）。
#[derive(Debug, Clone, PartialEq)]
pub enum BrightnessFailure {
    /// この環境では使えない。
    Unsupported,
    /// 範囲外・非有限値。
    InvalidInput,
    /// その他の失敗（固定文言のみ）。
    Failed(String),
}

/// OS の読み戻し（`fandhe_device::screen_brightness::BrightnessReading` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// 現在の輝度。出さない OS では `None`。
    pub level: Option<f32>,
    /// OS 側で上書きが効いているか。
    pub overriding: bool,
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
    /// 読み取りに失敗した（理由は `error`）。
    Failed,
}

/// 「画面の明るさ」画面の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenBrightnessShowState {
    slider_value: f32,
    /// 最後に OS で確認できたスライダー値（失敗時の戻し先）。
    confirmed: f32,
    /// 上書きを要求している（または効いている）か。
    desired: bool,
    pending: Option<BrightnessCommand>,
    applying: bool,
    in_flight: Option<BrightnessCommand>,
    generation: u64,
    read: ReadState,
    error: String,
    notice: String,
}

impl Default for ScreenBrightnessShowState {
    fn default() -> Self {
        Self {
            slider_value: 0.5,
            confirmed: 0.5,
            desired: false,
            pending: None,
            applying: false,
            in_flight: None,
            generation: 0,
            read: ReadState::Unread,
            error: String::new(),
            notice: String::new(),
        }
    }
}

impl ScreenBrightnessShowState {
    /// 読み取り状態。
    pub fn read_state(&self) -> &ReadState {
        &self.read
    }

    /// スライダーの現在値。
    pub fn slider_value(&self) -> f32 {
        self.slider_value
    }

    /// 現在の世代。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn overriding(&self) -> bool {
        matches!(&self.read, ReadState::Read(r) if r.overriding)
    }
}

fn round_level(v: f32) -> f32 {
    (v.clamp(SLIDER_MIN, SLIDER_MAX) * 100.0).round() / 100.0
}

fn percent(v: f32) -> u32 {
    (v * 100.0).round() as u32
}

/// `Screen::slider` の getter。
pub fn get_slider(state: &ScreenBrightnessShowState) -> f32 {
    state.slider_value
}

/// `Screen::slider` の setter。非有限値は無視し、範囲へ丸めて刻みにそろえる。変化があったときだけ
/// 要求を立てる。連続操作は最新値 1 件に畳まれる。非対応・未読の間は要求を立てない。
pub fn set_slider(state: &mut ScreenBrightnessShowState, value: f32) {
    if !value.is_finite() || !matches!(state.read, ReadState::Read(_)) {
        return;
    }
    let v = round_level(value);
    if v == state.slider_value && state.desired {
        return;
    }
    state.slider_value = v;
    state.desired = true;
    state.pending = Some(BrightnessCommand::Set(v));
    state.generation += 1;
    state.applying = false;
    state.error.clear();
    state.notice.clear();
}

/// 「元に戻す」。
pub fn clear(state: &mut ScreenBrightnessShowState) {
    if !matches!(state.read, ReadState::Read(_)) {
        return;
    }
    state.desired = false;
    state.pending = Some(BrightnessCommand::Clear);
    state.generation += 1;
    state.applying = false;
    state.error.clear();
    state.notice.clear();
}

/// 「読み直す」。失敗後に読み取りをやり直す。
pub fn reload(state: &mut ScreenBrightnessShowState) {
    state.read = ReadState::Unread;
    state.error.clear();
}

/// 保留中の要求を状態を変えずに返す。
pub fn take_request(state: &ScreenBrightnessShowState) -> Option<BrightnessRequest> {
    state.pending.map(|command| BrightnessRequest {
        generation: state.generation,
        command,
    })
}

/// 要求を OS へ渡す直前に呼ぶ。世代が一致するときだけ保留を取り除く
/// （新しい要求が来ていれば何もしない）。
pub fn mark_applying(state: &mut ScreenBrightnessShowState, generation: u64) {
    if state.generation != generation {
        return;
    }
    state.in_flight = state.pending.take();
    state.applying = true;
}

/// 要求の完了を反映する。`reading` は成否に関わらず OS から読み戻した値。
/// 世代が合わない（古い）完了は丸ごと捨てる。
pub fn finish(
    state: &mut ScreenBrightnessShowState,
    generation: u64,
    result: Result<(), BrightnessFailure>,
    reading: Result<Reading, BrightnessFailure>,
) {
    if state.generation != generation {
        return;
    }
    let command = state.in_flight.take();
    state.applying = false;

    let failure = match (&result, &reading, command) {
        (Err(f), _, _) => Some(f.clone()),
        (Ok(()), Ok(r), Some(BrightnessCommand::Set(_))) if !r.overriding => {
            Some(BrightnessFailure::Failed(NOT_APPLIED_REASON.to_string()))
        }
        (Ok(()), Ok(r), Some(BrightnessCommand::Set(v)))
            if r.level
                .is_some_and(|l| !l.is_finite() || (l - v).abs() > LEVEL_TOLERANCE) =>
        {
            Some(BrightnessFailure::Failed(LEVEL_MISMATCH_REASON.to_string()))
        }
        (Ok(()), Err(e), _) if *e != BrightnessFailure::Unsupported => Some(
            BrightnessFailure::Failed(READBACK_FAILED_REASON.to_string()),
        ),
        (Ok(()), Ok(r), Some(BrightnessCommand::Clear)) if r.overriding => {
            Some(BrightnessFailure::Failed(NOT_CLEARED_REASON.to_string()))
        }
        _ => None,
    };

    // 読み戻しは成否に関わらず真実として採用する。
    match reading {
        Ok(r) => state.read = ReadState::Read(r),
        Err(BrightnessFailure::Unsupported) => state.read = ReadState::Unsupported,
        // 読み戻せないときは古い「上書き中」を残さず、読み取り失敗として扱う。
        Err(_) => state.read = ReadState::Failed,
    }

    match failure {
        None => {
            if let Some(BrightnessCommand::Set(v)) = command {
                state.confirmed = v;
            }
            state.desired = state.overriding();
        }
        Some(BrightnessFailure::Unsupported) => {
            state.read = ReadState::Unsupported;
            state.desired = false;
        }
        Some(f) => {
            // 失敗した操作の値を表示に残さない。
            state.slider_value = state.confirmed;
            state.desired = state.overriding();
            state.error = match f {
                BrightnessFailure::InvalidInput => INVALID_LEVEL_REASON.to_string(),
                BrightnessFailure::Failed(m) => format!("{FAILED_PREFIX}{m}"),
                BrightnessFailure::Unsupported => String::new(),
            };
        }
    }
}

/// 定期読み取り（pull）の結果を反映する。
pub fn apply_read(
    state: &mut ScreenBrightnessShowState,
    result: Result<Reading, BrightnessFailure>,
) {
    match result {
        Ok(r) => {
            // 自分が解除を出していないのに上書きが外れた = 端末側の操作。
            if state.overriding() && !r.overriding && state.desired {
                state.notice = RELEASED_BY_USER_NOTE.to_string();
                state.desired = false;
            }
            state.read = ReadState::Read(r);
            // 上書き中でも、保留・処理中の操作が無ければ OS の値へスライダーを揃える。
            let idle = state.pending.is_none() && !state.applying;
            // 非有限値は信頼しない。
            if let Some(l) = r.level.filter(|l| idle && l.is_finite()) {
                state.slider_value = round_level(l);
                state.confirmed = state.slider_value;
            }
        }
        Err(BrightnessFailure::Unsupported) => {
            state.read = ReadState::Unsupported;
            state.desired = false;
        }
        Err(f) => {
            state.read = ReadState::Failed;
            state.desired = false;
            state.error = match f {
                BrightnessFailure::Failed(m) => format!("{FAILED_PREFIX}{m}"),
                _ => FAILED_PREFIX.to_string(),
            };
        }
    }
}

/// driver が OS を読み直すべきか。非対応・失敗では偽（失敗状態で再描画し続けない）。
pub fn wants_read(state: &ScreenBrightnessShowState) -> bool {
    matches!(state.read, ReadState::Unread | ReadState::Read(_))
}

/// 状態の案内。「上書き中」は読み戻しが上書きを示すときだけ出す。
pub fn status_text(state: &ScreenBrightnessShowState) -> String {
    match &state.read {
        ReadState::Unread => READING_STATUS.to_string(),
        ReadState::Unsupported => UNSUPPORTED_STATUS.to_string(),
        ReadState::Failed => String::new(),
        ReadState::Read(r) if r.overriding => {
            let v = r.level.unwrap_or(state.slider_value);
            format!("{OVERRIDING_PREFIX}{}%", percent(v))
        }
        ReadState::Read(_) => NOT_OVERRIDING_STATUS.to_string(),
    }
}

/// 現在値の案内。出さない OS では [`LEVEL_HIDDEN_NOTE`]。
pub fn level_text(state: &ScreenBrightnessShowState) -> String {
    match &state.read {
        ReadState::Read(r) if !r.overriding => match r.level {
            Some(l) => format!("{CURRENT_LEVEL_PREFIX}{}%", percent(l)),
            None => LEVEL_HIDDEN_NOTE.to_string(),
        },
        _ => String::new(),
    }
}

/// エラーの案内。
pub fn error_text(state: &ScreenBrightnessShowState) -> String {
    state.error.clone()
}

/// 端末側で上書きが外れたときの案内。
pub fn notice_text(state: &ScreenBrightnessShowState) -> String {
    state.notice.clone()
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
/// 読み取り済みのときだけスライダーを出し、非対応では理由だけを出す。
pub fn current_screen(state: &ScreenBrightnessShowState) -> Screen<ScreenBrightnessShowState> {
    let mut screen = Screen::new("画面の明るさ");
    if matches!(state.read, ReadState::Read(_)) {
        screen = screen.slider(
            SLIDER_ID,
            SLIDER_LABEL,
            SLIDER_MIN,
            SLIDER_MAX,
            get_slider,
            set_slider,
        );
    }
    screen = screen
        .dynamic_text(status_text)
        .dynamic_text(level_text)
        .dynamic_text(notice_text)
        .error_text(error_text);
    match state.read {
        ReadState::Read(r) if r.overriding => screen.button(CLEAR_BUTTON_ID, CLEAR_LABEL, clear),
        ReadState::Failed => screen.button(RELOAD_BUTTON_ID, RELOAD_LABEL, reload),
        _ => screen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn read(level: Option<f32>, overriding: bool) -> Reading {
        Reading { level, overriding }
    }

    fn ready() -> ScreenBrightnessShowState {
        let mut s = ScreenBrightnessShowState::default();
        apply_read(&mut s, Ok(read(Some(0.6), false)));
        s
    }

    fn failed(m: &str) -> BrightnessFailure {
        BrightnessFailure::Failed(m.to_string())
    }

    #[test]
    fn rapid_sets_collapse_to_latest() {
        let mut s = ready();
        let g0 = s.generation();
        set_slider(&mut s, 0.3);
        set_slider(&mut s, 0.4);
        set_slider(&mut s, 0.5);
        let req = take_request(&s).unwrap();
        assert_eq!(req.command, BrightnessCommand::Set(0.5));
        assert_eq!(req.generation, g0 + 3);
    }

    #[test]
    fn same_rounded_value_makes_no_request() {
        let mut s = ready();
        set_slider(&mut s, 0.3);
        let g = s.generation();
        mark_applying(&mut s, g);
        assert!(take_request(&s).is_none());
        set_slider(&mut s, 0.301);
        assert!(take_request(&s).is_none());
    }

    #[test]
    fn non_finite_ignored_and_low_clamped() {
        let mut s = ready();
        let before = s.clone();
        set_slider(&mut s, f32::NAN);
        set_slider(&mut s, f32::INFINITY);
        assert_eq!(s, before);
        set_slider(&mut s, 0.0);
        assert_eq!(
            take_request(&s).unwrap().command,
            BrightnessCommand::Set(SLIDER_MIN)
        );
    }

    #[test]
    fn stale_finish_is_dropped() {
        let mut s = ready();
        set_slider(&mut s, 0.3);
        let g = s.generation();
        mark_applying(&mut s, g);
        set_slider(&mut s, 0.8);
        let before = s.clone();
        finish(&mut s, g, Ok(()), Ok(read(Some(0.3), true)));
        assert_eq!(s, before);
        assert_eq!(
            take_request(&s).unwrap().command,
            BrightnessCommand::Set(0.8)
        );
    }

    #[test]
    fn set_failure_does_not_show_new_value() {
        let mut s = ready();
        let confirmed = s.slider_value();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Err(failed("x")), Ok(read(Some(0.6), false)));
        assert_eq!(s.slider_value(), confirmed);
        assert!(error_text(&s).starts_with(FAILED_PREFIX));
        assert_eq!(status_text(&s), NOT_OVERRIDING_STATUS);
    }

    #[test]
    fn clear_failure_keeps_overriding() {
        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.9), true)));
        clear(&mut s);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Err(failed("x")), Ok(read(Some(0.9), true)));
        assert!(status_text(&s).starts_with(OVERRIDING_PREFIX));
        assert!(!error_text(&s).is_empty());
    }

    #[test]
    fn ok_with_mismatched_readback_is_failure() {
        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.6), false)));
        assert!(error_text(&s).contains(NOT_APPLIED_REASON));
        assert!(!status_text(&s).starts_with(OVERRIDING_PREFIX));

        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.9), true)));
        clear(&mut s);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.9), true)));
        assert!(error_text(&s).contains(NOT_CLEARED_REASON));
        assert!(status_text(&s).starts_with(OVERRIDING_PREFIX));
    }

    #[test]
    fn ok_with_readback_error_is_failure() {
        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.9), true)));
        clear(&mut s);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Err(failed("x")));
        assert!(error_text(&s).contains(READBACK_FAILED_REASON));
        assert!(!status_text(&s).starts_with(OVERRIDING_PREFIX));
        assert_eq!(s.slider_value(), 0.9);

        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Err(failed("x")));
        assert_eq!(s.slider_value(), 0.6);
        assert!(error_text(&s).contains(READBACK_FAILED_REASON));
        assert!(!wants_read(&s));
    }

    #[test]
    fn ok_with_mismatched_level_is_failure_and_pull_syncs() {
        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.3), true)));
        assert!(error_text(&s).contains(LEVEL_MISMATCH_REASON));
        assert_eq!(s.slider_value(), 0.6);
        apply_read(&mut s, Ok(read(Some(0.3), true)));
        assert_eq!(s.slider_value(), 0.3);
    }

    #[test]
    fn overriding_text_only_when_readback_says_so() {
        let s = ready();
        assert!(!status_text(&s).starts_with(OVERRIDING_PREFIX));
        let mut s = s;
        set_slider(&mut s, 0.9);
        // 要求しただけでは「上書き中」と出さない。
        assert!(!status_text(&s).starts_with(OVERRIDING_PREFIX));
    }

    #[test]
    fn hidden_level_shows_note() {
        let mut s = ScreenBrightnessShowState::default();
        apply_read(&mut s, Ok(read(None, false)));
        assert_eq!(level_text(&s), LEVEL_HIDDEN_NOTE);
    }

    #[test]
    fn released_by_user_shows_notice() {
        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.9), true)));
        apply_read(&mut s, Ok(read(Some(0.4), false)));
        assert_eq!(notice_text(&s), RELEASED_BY_USER_NOTE);
        assert!(!status_text(&s).starts_with(OVERRIDING_PREFIX));
    }

    #[test]
    fn own_clear_does_not_show_notice() {
        let mut s = ready();
        set_slider(&mut s, 0.9);
        let g = s.generation();
        mark_applying(&mut s, g);
        finish(&mut s, g, Ok(()), Ok(read(Some(0.9), true)));
        clear(&mut s);
        apply_read(&mut s, Ok(read(Some(0.6), false)));
        assert_eq!(notice_text(&s), "");
    }

    #[test]
    fn unsupported_has_no_controls_and_no_requests() {
        let mut s = ScreenBrightnessShowState::default();
        apply_read(&mut s, Err(BrightnessFailure::Unsupported));
        set_slider(&mut s, 0.5);
        clear(&mut s);
        assert!(take_request(&s).is_none());
        assert!(!wants_read(&s));
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(joined.contains(UNSUPPORTED_STATUS));
        let count = |st: &ScreenBrightnessShowState| {
            let mut n = 0;
            current_screen(st).visit_expanded(st, |_| n += 1);
            n
        };
        // スライダーが無いぶん、読み取り済みの画面よりノードが少ない。
        assert!(count(&s) < count(&ready()));
        assert!(!joined.contains(CLEAR_LABEL));
    }

    #[test]
    fn failed_read_stops_polling_until_reload() {
        let mut s = ScreenBrightnessShowState::default();
        apply_read(&mut s, Err(failed("x")));
        assert!(!wants_read(&s));
        assert!(error_text(&s).starts_with(FAILED_PREFIX));
        reload(&mut s);
        assert!(wants_read(&s));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn non_overriding_read_syncs_slider() {
        let mut s = ScreenBrightnessShowState::default();
        apply_read(&mut s, Ok(read(Some(0.734), false)));
        assert_eq!(s.slider_value(), 0.73);
    }
}
