//! Phase 11（端末機能の拡充ループ、親 #864）型サンプル「読み上げ」（Issue #865）。
//! 入力欄 + 「読み上げ」→ OS の音声合成で発話、「停止」→ 発話を止める 1 画面。
//! `FANDHE_PROXY_APP=tts_speak` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod tts_speak;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の発話は本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/tts.rs`）が
//! [`take_speak_request`] / [`take_stop_request`] で要求を観測し
//! [`finish_speak`] / [`finish_stop`] で結果を注入する。
//!
//! # 成功と失敗を別フィールドに分ける理由
//!
//! 親 Issue #864 が最重要視する再発防止点は「初期化・発話に失敗したときにダミー
//! 実装へ黙って落ち、OS 側では何も起きていないのに画面へ『成功』と出す」経路
//! （#859 の P1 指摘と同型）。本画面は成功文言を `status`、失敗文言を `error` の
//! 別フィールドへ書き分け、[`current_screen`] は両方を（`error_text` で意味的な
//! 役割 `TextRole::Error` を持たせて）常に描画する。色の解決はプラットフォーム
//! レイヤの `TokenResolver` に委ね、DSL 側には色を書かない（`screens/form.rs` 参照）。
//!
//! # 発話中でも停止を受け付ける理由（codex-review P1・Cursor Bugbot 指摘の是正）
//!
//! 発話（`speak_text`）と停止（`stop_requested`）を別々のフィールドで持ち、
//! 独立に発送・完了を追跡する。1 つの `pending: Option<enum>` に両方を収めていた
//! 旧版は、発話が完了するまで停止要求そのものを受理できず（停止ボタンが発話中
//! ずっと消える）、発話完了を待ってからでないと停止できない構造的欠陥だった。
//!
//! [`stop`] は発話が発送済み（OS 呼び出しが別スレッドで進行中）であっても常に
//! 受理し、`speak_text` を即座に畳んで「打ち切り」を表す。OS 呼び出しスレッド
//! （`device/tts.rs::run_speak`、`Tts::wait_outcome` でブロッキング待機中）は
//! 動き続けるが、後から届くその完了は [`finish_speak`] が `speak_text` が既に
//! `None`（＝停止で打ち切り済み）であることを見て無視する（stale 判定、
//! `stale_completion_is_ignored` テスト参照）。表示に使う結果は停止側の
//! [`finish_stop`] が届けるものに一本化される。
//!
//! ```text
//! Idle --読み上げ--> speak_text=Some, speak_dispatched=false
//!   --effect が take_speak_request で観測 → mark_speak_dispatched-->
//!   OS 呼び出し（別スレッド、wait_outcome でブロッキング）
//!     --停止ボタンはこの間も表示され続ける（stop_requested のみで別途ゲート）--
//!   --finish_speak（speak_text が None ならこの完了は無視）--> 結果表示
//!
//! （いつでも）--停止--> stop_requested=true, stop_dispatched=false, speak_text=None
//!   --effect が take_stop_request で観測 → mark_stop_dispatched-->
//!   OS 呼び出し（別スレッド、`Tts::stop` が進行中の子プロセスを kill）
//!   --finish_stop--> stop_requested=false + 結果表示
//! ```
//!
//! # 完了通知を要求 ID（世代）で識別する理由（codex-review P1・Cursor Bugbot 指摘の是正）
//!
//! `speak_text` が `Some` かどうかだけでは「どの発話要求の完了か」を識別できない。
//! 発話 A → 停止（`speak_text` は `None` に戻る）→ 発話 B（`speak_text` が再び
//! `Some` になる）→ 打ち切られたはずの A の遅延した完了、という順で
//! [`finish_speak`] が呼ばれると、旧版は `speak_text.is_some()` だけを見て
//! B の要求を A の結果で誤って完了させてしまう（B は実際には OS 側で発話中の
//! ままなのに画面は結果表示済みになり、後から届く B 本来の完了は無視される）。
//! [`speak`] は呼ぶたびに [`TtsSpeakState::speak_generation`] を進め、
//! [`take_speak_request`] はその世代を要求と共に返す。effect ドライバは
//! 世代を OS 呼び出し・`wait_outcome` の結果と共に持ち回り、[`finish_speak`] へ
//! 引き渡す。[`finish_speak`] は現在の世代と一致する完了だけを反映し、
//! 一致しない（＝停止や新しい発話で上書き済みの）完了は無視する。

use crate::{Screen, WidgetId};

/// 「読み上げ」ボタンの `WidgetId`。
const SPEAK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「停止」ボタンの `WidgetId`。
const STOP_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 入力が空のまま読み上げようとしたときの案内。
pub const EMPTY_INPUT_STATUS: &str = "読み上げる文字列を入力してください";
/// 発話中の案内。
pub const SPEAKING_STATUS: &str = "読み上げ中…";
/// 発話開始に成功したときの案内。
pub const SPOKE_STATUS: &str = "読み上げました";
/// 停止に成功したときの案内。
pub const STOPPED_STATUS: &str = "停止しました";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 読み上げ画面の状態。
///
/// 発話（`speak_*`）と停止（`stop_*`）を独立したフィールドで追跡する
/// （モジュール doc「発話中でも停止を受け付ける理由」参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtsSpeakState {
    /// 入力欄の現在値（発話対象）。
    pub input: String,
    /// 未処理・処理中の発話対象テキスト。effect ドライバが
    /// [`take_speak_request`] で観測する。
    pub speak_text: Option<String>,
    /// 直近に [`speak`] で立てた発話要求の世代番号。[`speak`] を呼ぶたびに進み、
    /// 完了通知（[`finish_speak`]）が「今の要求」を指しているかの照合に使う
    /// （モジュール doc「完了通知を要求 ID（世代）で識別する理由」参照）。
    pub speak_generation: u64,
    /// `speak_text` を effect ドライバが既に別スレッドへ渡し済みか
    /// （[`mark_speak_dispatched`] 参照）。`take_speak_request` が同じ要求を
    /// 毎フレーム再送してしまう二重発話を防ぐゲート。
    pub speak_dispatched: bool,
    /// 未処理・処理中の停止要求か。`speak_text` の状態に関わらずいつでも
    /// 立てられる（発話中でも停止を受け付ける、モジュール doc 参照）。
    pub stop_requested: bool,
    /// `stop_requested` を effect ドライバが既に別スレッドへ渡し済みか
    /// （`speak_dispatched` と同型のゲート）。
    pub stop_dispatched: bool,
    /// 直近の操作結果（成功時のみ）。失敗時は空文字列にする。
    pub status: String,
    /// 直近の失敗理由。成功時は空文字列にする（成功と失敗を取り違えないための分離、
    /// モジュール doc 参照）。
    pub error: String,
}

impl Default for TtsSpeakState {
    fn default() -> Self {
        Self {
            // Mac 目視・キャプチャで機能が一目で伝わるデモ文言（CJK フォントの確認も兼ねる）。
            input: "こんにちは、fandhe の読み上げ機能です".to_string(),
            speak_text: None,
            speak_generation: 0,
            speak_dispatched: false,
            stop_requested: false,
            stop_dispatched: false,
            status: String::new(),
            error: String::new(),
        }
    }
}

/// 入力欄の表示値（`Screen::text_field` の getter）。
pub fn get_input(state: &TtsSpeakState) -> String {
    state.input.clone()
}

/// 入力欄の入力を反映する（`Screen::text_field` の setter）。
pub fn set_input(state: &mut TtsSpeakState, value: String) {
    state.input = value;
}

/// 「読み上げ」ボタンのハンドラ。空白のみの入力は要求を立てない。
/// 既に発話が進行中（`speak_text` が立っている）は二重要求を防ぐため no-op
/// （panic を API 契約にしない、coding-rust.md）。
pub fn speak(state: &mut TtsSpeakState) {
    if state.speak_text.is_some() {
        return;
    }
    let text = state.input.trim();
    if text.is_empty() {
        state.status = EMPTY_INPUT_STATUS.to_string();
        state.error = String::new();
        return;
    }
    state.speak_text = Some(text.to_string());
    state.speak_generation = state.speak_generation.wrapping_add(1);
    state.speak_dispatched = false;
    state.status = SPEAKING_STATUS.to_string();
    state.error = String::new();
}

/// 「停止」ボタンのハンドラ。既に停止要求済みなら二重発送を避け no-op。
/// 発話が発送済み（OS 呼び出しが進行中）でも常に受理し、発話要求を打ち切る
/// （モジュール doc「発話中でも停止を受け付ける理由」参照。打ち切られた発話の
/// 完了は [`finish_speak`] が stale として無視する）。
pub fn stop(state: &mut TtsSpeakState) {
    if state.stop_requested {
        return;
    }
    state.stop_requested = true;
    state.stop_dispatched = false;
    state.speak_text = None;
    state.speak_dispatched = false;
}

/// effect ドライバが未処理・未発送の発話要求を観測するための getter（状態は変えない。
/// 発送済みマークは呼び出し元が [`mark_speak_dispatched`] を `CompletionSender`
/// 経由で送る契約）。戻り値の世代番号は [`finish_speak`] へそのまま引き渡し、
/// 完了通知の照合に使う（モジュール doc参照）。
pub fn take_speak_request(state: &TtsSpeakState) -> Option<(u64, String)> {
    if state.speak_dispatched {
        return None;
    }
    state
        .speak_text
        .clone()
        .map(|text| (state.speak_generation, text))
}

/// `speak_text` の要求を発送済みにする（`CompletionSender::send` 経由でのみ呼ばれる契約、
/// `fandhe-egui-proxy` 側の effect ドライバ参照）。
pub fn mark_speak_dispatched(state: &mut TtsSpeakState) {
    state.speak_dispatched = true;
}

/// effect ドライバが未処理・未発送の停止要求を観測するための getter（状態は変えない）。
pub fn take_stop_request(state: &TtsSpeakState) -> bool {
    state.stop_requested && !state.stop_dispatched
}

/// 停止要求を発送済みにする（`take_speak_request` の停止版）。
pub fn mark_stop_dispatched(state: &mut TtsSpeakState) {
    state.stop_dispatched = true;
}

/// 発話結果を反映する。`generation` は [`take_speak_request`] が返した世代番号
/// （呼び出し元が OS 呼び出し・`wait_outcome` を経ても保持し続ける契約）。
/// `stop` で打ち切り済み（`speak_text` が既に `None`）、または現在の世代と
/// 一致しない（＝停止や新しい発話要求で既に上書きされた古い完了）場合は
/// この完了を無視する（モジュール doc「完了通知を要求 ID（世代）で識別する理由」参照）。
pub fn finish_speak(state: &mut TtsSpeakState, generation: u64, result: Result<(), String>) {
    if state.speak_text.is_none() || state.speak_generation != generation {
        return;
    }
    state.speak_text = None;
    state.speak_dispatched = false;
    match result {
        Ok(()) => {
            state.status = SPOKE_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 停止結果を反映する。停止要求が立っていない状態で届いた完了は古い結果とみなして無視する。
pub fn finish_stop(state: &mut TtsSpeakState, result: Result<(), String>) {
    if !state.stop_requested {
        return;
    }
    state.stop_requested = false;
    state.stop_dispatched = false;
    match result {
        Ok(()) => {
            state.status = STOPPED_STATUS.to_string();
            state.error = String::new();
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 成功時の案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &TtsSpeakState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &TtsSpeakState) -> String {
    state.error.clone()
}

/// 読み上げ画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。
///
/// - 停止要求が発送待ち・処理中（`stop_requested`）の間は二重発送を防ぐため
///   両方のボタンを外す。
/// - 発話中（`speak_text` が立っている）は「読み上げ」ボタンだけ外し、
///   「停止」ボタンは表示し続ける（codex-review P1・Cursor Bugbot 指摘の是正:
///   発話完了を待たずに停止できる）。
pub fn current_screen(state: &TtsSpeakState) -> Screen<TtsSpeakState> {
    let screen = Screen::new("読み上げ")
        .text_field("読み上げる文字列", get_input, set_input)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.stop_requested {
        return screen;
    }
    if state.speak_text.is_some() {
        return screen.button(STOP_BUTTON_ID, "停止", stop);
    }
    screen
        .button(SPEAK_BUTTON_ID, "読み上げ", speak)
        .button(STOP_BUTTON_ID, "停止", stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speak_sets_pending_with_trimmed_input() {
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "  読み上げ対象  ".to_string());
        speak(&mut state);
        assert_eq!(state.speak_text, Some("読み上げ対象".to_string()));
    }

    #[test]
    fn speak_rejects_blank_input() {
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "   ".to_string());
        speak(&mut state);
        assert_eq!(state.speak_text, None);
        assert_eq!(state.status, EMPTY_INPUT_STATUS);
    }

    #[test]
    fn speak_is_noop_while_already_speaking() {
        let mut state = TtsSpeakState {
            speak_text: Some("先発".to_string()),
            ..Default::default()
        };
        set_input(&mut state, "あとから".to_string());
        speak(&mut state);
        assert_eq!(state.speak_text, Some("先発".to_string()));
    }

    #[test]
    fn finish_speak_reports_success_and_clears_error() {
        let mut state = TtsSpeakState {
            error: "前回の失敗".to_string(),
            ..Default::default()
        };
        speak(&mut state);
        let generation = state.speak_generation;
        finish_speak(&mut state, generation, Ok(()));
        assert_eq!(state.status, SPOKE_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn finish_speak_error_sets_error_text_and_no_success_status() {
        // #859 の P1 指摘（ダミーへ黙って落ちて偽の成功を出す）と同型の経路を
        // 型サンプル側でも塞ぐことの固定。
        let mut state = TtsSpeakState::default();
        speak(&mut state);
        let generation = state.speak_generation;
        finish_speak(
            &mut state,
            generation,
            Err("この環境では読み上げを使えません".to_string()),
        );
        assert!(
            !state.status.contains(SPOKE_STATUS),
            "got: {}",
            state.status
        );
        assert_eq!(state.status, "");
        assert!(
            state.error.starts_with(FAILED_PREFIX),
            "got: {}",
            state.error
        );
        assert!(!state.error.is_empty());
    }

    #[test]
    fn stop_sets_pending() {
        let mut state = TtsSpeakState::default();
        stop(&mut state);
        assert!(state.stop_requested);
    }

    #[test]
    fn stop_is_noop_while_already_requested() {
        let mut state = TtsSpeakState {
            stop_requested: true,
            stop_dispatched: true,
            ..Default::default()
        };
        stop(&mut state);
        // 二重発送を避けるため dispatched フラグは変わらない。
        assert!(state.stop_dispatched);
    }

    #[test]
    fn finish_stop_reports_stopped() {
        let mut state = TtsSpeakState::default();
        stop(&mut state);
        finish_stop(&mut state, Ok(()));
        assert_eq!(state.status, STOPPED_STATUS);
        assert!(!state.stop_requested);
    }

    #[test]
    fn stale_completion_is_ignored() {
        let mut state = TtsSpeakState {
            status: "変わらない".to_string(),
            ..Default::default()
        };
        let generation = state.speak_generation;
        finish_speak(&mut state, generation, Ok(()));
        finish_stop(&mut state, Ok(()));
        assert_eq!(state.status, "変わらない");
    }

    #[test]
    fn finish_stop_without_request_does_not_clear_unrelated_speak() {
        // stop() を経ずに finish_stop が届いても、進行中の発話要求を勝手に
        // 完了させない（無関係な完了の取り違え防止）。
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "x".to_string());
        speak(&mut state);
        finish_stop(&mut state, Ok(()));
        assert!(state.speak_text.is_some());
    }

    #[test]
    fn stop_while_speaking_cancels_speak_and_keeps_stop_button() {
        // codex-review P1 / Cursor Bugbot 指摘の是正: 発話中でも停止ボタンが
        // 押せて、押すと発話要求は即座に打ち切られる。
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "長い文章".to_string());
        speak(&mut state);
        assert!(state.speak_text.is_some());

        stop(&mut state);
        assert!(state.speak_text.is_none(), "発話要求は打ち切られる");
        assert!(state.stop_requested);
    }

    #[test]
    fn late_speak_completion_after_stop_is_ignored() {
        // 発話中に停止 → 別スレッドの OS 呼び出しがその後に完了しても
        // （wait_outcome がブロッキング中だったケース）、停止で表示した結果を
        // 上書きしない（stale 判定、モジュール doc 参照）。
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "長い文章".to_string());
        speak(&mut state);
        let stale_generation = state.speak_generation;
        stop(&mut state);
        finish_stop(&mut state, Ok(()));
        assert_eq!(state.status, STOPPED_STATUS);

        // 打ち切られた発話スレッドから遅れて届く完了。
        finish_speak(&mut state, stale_generation, Ok(()));
        assert_eq!(
            state.status, STOPPED_STATUS,
            "打ち切り後に届いた発話完了で上書きされてはならない"
        );
    }

    #[test]
    fn late_speak_completion_after_newer_speak_is_ignored() {
        // codex-review P1・Cursor Bugbot 指摘の是正: 発話 A → 停止 → 発話 B の
        // 順で、A の遅延した完了が B の要求を誤って完了させないことの固定
        // （`speak_text.is_some()` だけを見ていた旧版は世代を持たず区別できなかった）。
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "発話 A".to_string());
        speak(&mut state);
        let stale_generation = state.speak_generation;
        stop(&mut state);
        finish_stop(&mut state, Ok(()));

        set_input(&mut state, "発話 B".to_string());
        speak(&mut state);
        assert_ne!(state.speak_generation, stale_generation);

        // A の遅延した完了。B の要求はまだ進行中のまま保たれる。
        finish_speak(&mut state, stale_generation, Ok(()));
        assert!(
            state.speak_text.is_some(),
            "古い世代の完了で B の要求が消えてはならない"
        );
        assert_ne!(
            state.status, SPOKE_STATUS,
            "A の完了が B の成功として誤表示されてはならない"
        );

        // B 本来の完了は正しく反映される。
        let generation = state.speak_generation;
        finish_speak(&mut state, generation, Ok(()));
        assert_eq!(state.status, SPOKE_STATUS);
        assert!(state.speak_text.is_none());
    }

    #[test]
    fn buttons_are_hidden_while_stop_is_in_flight() {
        use crate::click_button;
        let mut state = TtsSpeakState {
            stop_requested: true,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(!click_button(&screen, "読み上げ", &mut state));
        assert!(!click_button(&screen, "停止", &mut state));
    }

    #[test]
    fn stop_button_stays_visible_while_speaking() {
        use crate::click_button;
        let mut state = TtsSpeakState {
            speak_text: Some("発話中".to_string()),
            speak_dispatched: true,
            ..Default::default()
        };
        let screen = current_screen(&state);
        assert!(
            !click_button(&screen, "読み上げ", &mut state),
            "発話中は読み上げボタンを外す"
        );
        assert!(
            click_button(&screen, "停止", &mut state),
            "発話中でも停止ボタンは押せる"
        );
    }

    #[test]
    fn buttons_are_clickable_when_idle() {
        use crate::click_button;
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "押せる".to_string());
        let screen = current_screen(&state);
        assert!(click_button(&screen, "読み上げ", &mut state));
        assert!(state.speak_text.is_some());
    }

    #[test]
    fn take_speak_request_returns_none_once_dispatched() {
        // OS 呼び出しが別スレッドへ移った（egui-proxy 側 UI スレッド解放）ことで
        // 完了まで複数フレームにまたがるようになったため、`speak_dispatched` が
        // 立った後は同じ要求を再度 `take_speak_request` で取り出せない
        // （＝二重発話しない）ことの固定（codex-review P1 指摘）。
        let mut state = TtsSpeakState::default();
        set_input(&mut state, "1 回だけ".to_string());
        speak(&mut state);
        let (generation, _text) = take_speak_request(&state).expect("要求がある");
        assert_eq!(generation, state.speak_generation);

        mark_speak_dispatched(&mut state);
        assert_eq!(take_speak_request(&state), None);

        // 完了が届いて finish すれば dispatched は下り、次の要求はまた取り出せる。
        finish_speak(&mut state, generation, Ok(()));
        assert!(!state.speak_dispatched);
    }

    #[test]
    fn take_stop_request_returns_false_once_dispatched() {
        let mut state = TtsSpeakState::default();
        stop(&mut state);
        assert!(take_stop_request(&state));

        mark_stop_dispatched(&mut state);
        assert!(!take_stop_request(&state));

        finish_stop(&mut state, Ok(()));
        assert!(!state.stop_dispatched);
    }
}
