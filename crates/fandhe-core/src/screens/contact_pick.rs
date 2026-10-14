//! Phase 11（端末機能の拡充ループ、親 #1117）型サンプル「連絡先選択」（Issue #1220）。
//! 「連絡先を選ぶ」ボタン → OS の連絡先ピッカー → 選んだ 1 件の表示名を出す 1 画面。
//! `FANDHE_PROXY_APP=contact_pick` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod contact_pick;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。画面状態に持つのは表示名だけで、
//! 電話・メール・識別子は持たない。橋渡し層が `DeviceError` を [`ContactPickOutcome`] へ
//! 畳んで渡す（iOS / Android / Web の橋渡しも同じ型を使う）。
//!
//! # 成功・キャンセル・失敗を混同しない
//!
//! [`PickedContact`](ContactPickOutcome::Picked) だけが [`ContactPickState::picked`] を埋める。
//! それ以外は必ず `None` に戻す（失敗表示の横に前回の名前を残さない）。新しい要求を出した
//! 時点でも名前を消す。キャンセルは `status`（通常色）、拒否・非対応・失敗は `error`
//! （`TextRole::Error`）へ書き分ける。進行状態・世代照合は `file_pick` と同じ
//! （[`PickPhase`] を再利用する）。

use crate::{Screen, WidgetId};

pub use super::file_pick::PickPhase;

/// 「連絡先を選ぶ」ボタンの `WidgetId`。
const PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "「連絡先を選ぶ」で連絡先を 1 件選びます";
/// キャンセルされたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "選択しませんでした";
/// 権限拒否時の案内。
pub const PERMISSION_DENIED_TEXT: &str = "連絡先へのアクセスが許可されていません。システム設定 > プライバシーとセキュリティ > 連絡先 で許可してください";
/// 非対応環境の案内。
pub const UNSUPPORTED_TEXT: &str = "この環境では連絡先を選べません";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 橋渡し層が画面へ渡す結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContactPickOutcome {
    /// 選ばれた連絡先の表示名（空でないことは橋渡し層が保証する）。
    Picked(String),
    /// 利用者がキャンセルした。
    Cancelled,
    /// 権限が拒否・制限された。
    PermissionDenied,
    /// この環境では選べない。
    Unsupported,
    /// 失敗（理由は内部識別子のみで、名前は含まない）。
    Failed(String),
}

/// 連絡先選択画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactPickState {
    /// 選択操作の進行状態。
    pub phase: PickPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 選ばれた表示名。成功の唯一の根拠。
    pub picked: Option<String>,
    /// キャンセルの案内文言（通常色）。
    pub status: String,
    /// 拒否・非対応・失敗の理由（`TextRole::Error`）。
    pub error: String,
}

/// 「連絡先を選ぶ」ボタンのハンドラ。`Idle` 以外は no-op。前の名前は新しい要求で消す。
pub fn request_pick(state: &mut ContactPickState) {
    if state.phase != PickPhase::Idle {
        return;
    }
    state.phase = PickPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.picked = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &ContactPickState) -> Option<u64> {
    (state.phase == PickPhase::Requested).then_some(state.generation)
}

/// 要求を処理中（`Picking`）へ進める。`Requested` 以外では何もしない。
pub fn mark_picking(state: &mut ContactPickState) {
    if state.phase == PickPhase::Requested {
        state.phase = PickPhase::Picking;
    }
}

/// 選択結果を反映する。`Idle` のとき・世代が一致しないときは無視する。
pub fn finish_pick(state: &mut ContactPickState, generation: u64, outcome: ContactPickOutcome) {
    if state.phase == PickPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PickPhase::Idle;
    state.picked = None;
    state.status = String::new();
    state.error = String::new();
    match outcome {
        ContactPickOutcome::Picked(name) => state.picked = Some(name),
        ContactPickOutcome::Cancelled => state.status = CANCELLED_STATUS.to_string(),
        ContactPickOutcome::PermissionDenied => state.error = PERMISSION_DENIED_TEXT.to_string(),
        ContactPickOutcome::Unsupported => state.error = UNSUPPORTED_TEXT.to_string(),
        ContactPickOutcome::Failed(message) => {
            state.error = format!("{FAILED_PREFIX}{message}");
        }
    }
}

/// 選んだ連絡先の表示文言（`DynamicText` の `get`）。未選択なら空文字列。
pub fn picked_text(state: &ContactPickState) -> String {
    state
        .picked
        .as_ref()
        .map(|n| format!("選んだ連絡先: {n}"))
        .unwrap_or_default()
}

/// キャンセルの案内文言。
pub fn status_text(state: &ContactPickState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言。
pub fn error_text(state: &ContactPickState) -> String {
    state.error.clone()
}

/// 連絡先選択画面を組み立てる。ボタンは `Idle` のときだけ出す。
pub fn current_screen(state: &ContactPickState) -> Screen<ContactPickState> {
    let screen = Screen::new("連絡先選択")
        .text(IDLE_HINT)
        .dynamic_text(picked_text)
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != PickPhase::Idle {
        return screen;
    }
    screen.button(PICK_BUTTON_ID, "連絡先を選ぶ", request_pick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picking() -> ContactPickState {
        let mut s = ContactPickState::default();
        request_pick(&mut s);
        mark_picking(&mut s);
        s
    }

    #[test]
    fn success_sets_picked_only() {
        let mut s = picking();
        finish_pick(&mut s, 1, ContactPickOutcome::Picked("山田 太郎".into()));
        assert_eq!(s.phase, PickPhase::Idle);
        assert_eq!(picked_text(&s), "選んだ連絡先: 山田 太郎");
        assert!(s.status.is_empty() && s.error.is_empty());
    }

    #[test]
    fn non_success_outcomes_never_leave_a_name() {
        let cases = [
            (ContactPickOutcome::Cancelled, false),
            (ContactPickOutcome::PermissionDenied, true),
            (ContactPickOutcome::Unsupported, true),
            (ContactPickOutcome::Failed("x".into()), true),
        ];
        let mut texts = Vec::new();
        for (outcome, is_error) in cases {
            let mut s = picking();
            s.picked = Some("前回".into());
            finish_pick(&mut s, 1, outcome);
            assert!(s.picked.is_none());
            assert_eq!(s.error.is_empty(), !is_error);
            assert_eq!(s.status.is_empty(), is_error);
            texts.push(format!("{}{}", s.status, s.error));
        }
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), 4, "文言が区別される");
    }

    #[test]
    fn stale_generation_and_idle_completion_are_ignored() {
        let mut s = picking();
        finish_pick(&mut s, 99, ContactPickOutcome::Picked("A".into()));
        assert_eq!(s.phase, PickPhase::Picking);
        assert!(s.picked.is_none());
        let mut idle = ContactPickState::default();
        finish_pick(&mut idle, 0, ContactPickOutcome::Picked("A".into()));
        assert!(idle.picked.is_none());
    }

    #[test]
    fn new_request_clears_previous_name() {
        let mut s = ContactPickState {
            picked: Some("A".into()),
            ..Default::default()
        };
        request_pick(&mut s);
        assert!(s.picked.is_none());
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = picking();
        request_pick(&mut s);
        assert_eq!(s.phase, PickPhase::Picking);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn button_only_when_idle() {
        let idle = ContactPickState::default();
        let busy = picking();
        assert_eq!(current_screen(&idle).len(), current_screen(&busy).len() + 1);
    }
}
