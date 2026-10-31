//! Phase 11 の拡充ループ（親 #151、Issue #1128）型サンプル「NFC カードエミュレーション」。
//! 端末が NFC カードとして読まれる側（HCE）の状態を表示する 1 画面。
//! `FANDHE_PROXY_APP=nfc_card_emulation_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod nfc_card_emulation_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。状態の読み出しは
//! `fandhe_device::nfc_card_emulation`、結果を本モジュールへ注入するのは `fandhe-egui-proxy` の
//! effect ドライバ（`device/nfc_card_emulation.rs`）の責務。ドライバは読み出しを高々 1 件に絞り、
//! 世代番号つきで [`finish_read`] へ届ける。
//!
//! 全対象 OS 分の状態・文言を本モジュールで持つ。操作ボタンは無い（AID は manifest で静的に登録）ため、
//! 後続の OS 別の子 issue はこのファイルに触れない。「0 回」「読み出せない」「未対応」は別の状態で、
//! 文言も分ける。応答の中身・失敗の理由は表示しない。

use crate::Screen;

/// 読み出し結果（端末依存の型を持ち込まないための写像先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    /// 状態が読めた。
    Status {
        /// NFC が有効か。
        nfc_enabled: bool,
        /// この AID の既定の振り先が本アプリか。
        default_for_aid: bool,
        /// 応答した回数。
        responses: u64,
        /// 最後の応答からの経過ミリ秒。応答 0 回なら `None`。
        since_last_ms: Option<u64>,
    },
    /// 読み出しに失敗した（0 回とは別）。
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Unread,
    Unsupported,
    Status {
        nfc_enabled: bool,
        default_for_aid: bool,
        responses: u64,
        since_last_ms: Option<u64>,
    },
    ReadFailed,
}

/// 「NFC カードエミュレーション」画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NfcCardEmulationShowState {
    view: View,
    aid: Option<&'static str>,
    /// 最後に適用した読み出しの世代。これより古い完了は捨てる。
    applied_gen: Option<u64>,
}

impl NfcCardEmulationShowState {
    /// 非対応として確定済みか（effect ドライバが読み出しをやめるために使う）。
    pub fn is_unsupported(&self) -> bool {
        self.view == View::Unsupported
    }

    /// 注入済みの AID（effect ドライバが一度だけ送るために使う）。
    pub fn aid(&self) -> Option<&'static str> {
        self.aid
    }

    /// 最後に適用した読み出しの世代。
    pub fn applied_gen(&self) -> Option<u64> {
        self.applied_gen
    }
}

/// 読み出し結果を反映する。適用済みより古い世代、および非対応確定後の完了は捨てる。
pub fn finish_read(state: &mut NfcCardEmulationShowState, generation: u64, outcome: ReadOutcome) {
    if state.view == View::Unsupported {
        return;
    }
    if matches!(state.applied_gen, Some(prev) if generation < prev) {
        return;
    }
    state.applied_gen = Some(generation);
    state.view = match outcome {
        ReadOutcome::Status {
            nfc_enabled,
            default_for_aid,
            responses,
            since_last_ms,
        } => View::Status {
            nfc_enabled,
            default_for_aid,
            responses,
            since_last_ms,
        },
        ReadOutcome::Failed => View::ReadFailed,
    };
}

/// この OS では使えないことを反映する。以後の読み出し結果は無視する。
pub fn apply_unsupported(state: &mut NfcCardEmulationShowState) {
    state.view = View::Unsupported;
}

/// AID を反映する。
pub fn set_aid(state: &mut NfcCardEmulationShowState, aid: &'static str) {
    state.aid = Some(aid);
}

/// 状態行の文言。
pub fn status_text(state: &NfcCardEmulationShowState) -> String {
    match state.view {
        View::Unread => "読み取り中",
        View::Unsupported => "この OS では未対応です",
        View::Status { .. } => "対応しています",
        View::ReadFailed => "状態を読み出せません",
    }
    .to_string()
}

/// AID 行の文言。未注入なら空。
pub fn aid_text(state: &NfcCardEmulationShowState) -> String {
    state.aid.map(|a| format!("AID: {a}")).unwrap_or_default()
}

/// NFC の有効状態の文言。状態が読めたときだけ出す。
pub fn nfc_text(state: &NfcCardEmulationShowState) -> String {
    match state.view {
        View::Status {
            nfc_enabled: true, ..
        } => "NFC: オン".to_string(),
        View::Status { .. } => "NFC: オフ（設定でオンにすると読み取られます）".to_string(),
        _ => String::new(),
    }
}

/// 既定の振り先の文言。状態が読めたときだけ出す。
pub fn default_text(state: &NfcCardEmulationShowState) -> String {
    match state.view {
        View::Status {
            default_for_aid: true,
            ..
        } => "既定の振り先: このアプリ".to_string(),
        View::Status { .. } => {
            "既定の振り先: 他のアプリ（読み取り時に OS が選ばせます）".to_string()
        }
        _ => String::new(),
    }
}

/// 応答回数の文言。状態が読めたときだけ出す。
pub fn responses_text(state: &NfcCardEmulationShowState) -> String {
    match state.view {
        View::Status { responses, .. } => format!("応答した回数: {responses}（このプロセス内）"),
        _ => String::new(),
    }
}

/// 最後の応答からの経過の文言。状態が読めたときだけ出す。
pub fn since_text(state: &NfcCardEmulationShowState) -> String {
    match state.view {
        View::Status {
            since_last_ms: Some(ms),
            ..
        } => format!("最後の応答から: {} 秒", ms / 1000),
        View::Status { .. } => "まだ応答していません".to_string(),
        _ => String::new(),
    }
}

/// 「NFC カードエミュレーション」画面。操作ボタンは無い。
pub fn nfc_card_emulation_show_screen() -> Screen<NfcCardEmulationShowState> {
    Screen::new("NFC カードエミュレーション")
        .dynamic_text(status_text)
        .dynamic_text(aid_text)
        .dynamic_text(nfc_text)
        .dynamic_text(default_text)
        .dynamic_text(responses_text)
        .dynamic_text(since_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &NfcCardEmulationShowState) -> Screen<NfcCardEmulationShowState> {
    nfc_card_emulation_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    type St = NfcCardEmulationShowState;

    fn ok(nfc: bool, def: bool, n: u64, since: Option<u64>) -> ReadOutcome {
        ReadOutcome::Status {
            nfc_enabled: nfc,
            default_for_aid: def,
            responses: n,
            since_last_ms: since,
        }
    }

    #[test]
    fn states_have_distinct_text() {
        let unread = St::default();
        let mut st = St::default();
        finish_read(&mut st, 1, ok(true, true, 0, None));
        let mut failed = St::default();
        finish_read(&mut failed, 1, ReadOutcome::Failed);
        let mut unsup = St::default();
        apply_unsupported(&mut unsup);
        let t = [
            status_text(&unread),
            status_text(&st),
            status_text(&failed),
            status_text(&unsup),
        ];
        for (i, a) in t.iter().enumerate() {
            for b in &t[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(!status_text(&failed).contains('0'));
        assert_eq!(responses_text(&failed), "");
    }

    #[test]
    fn stale_ok_does_not_overwrite_newer_failure() {
        let mut s = St::default();
        finish_read(&mut s, 2, ReadOutcome::Failed);
        finish_read(&mut s, 1, ok(true, true, 5, None));
        assert_eq!(status_text(&s), "状態を読み出せません");
        assert_eq!(s.applied_gen(), Some(2));
    }

    #[test]
    fn failure_recovers_on_newer_success() {
        let mut s = St::default();
        finish_read(&mut s, 1, ReadOutcome::Failed);
        finish_read(&mut s, 2, ok(true, true, 3, Some(1500)));
        assert_eq!(responses_text(&s), "応答した回数: 3（このプロセス内）");
        assert_eq!(since_text(&s), "最後の応答から: 1 秒");
    }

    #[test]
    fn unsupported_is_terminal() {
        let mut s = St::default();
        apply_unsupported(&mut s);
        finish_read(&mut s, 9, ok(true, true, 7, None));
        assert!(s.is_unsupported());
        assert_eq!(status_text(&s), "この OS では未対応です");
        assert_eq!(responses_text(&s), "");
        assert_eq!(nfc_text(&s), "");
    }

    #[test]
    fn nfc_off_and_other_default_are_status_not_failure() {
        let mut s = St::default();
        finish_read(&mut s, 1, ok(false, false, 0, None));
        assert_eq!(status_text(&s), "対応しています");
        assert!(nfc_text(&s).contains("オフ"));
        assert!(default_text(&s).contains("他のアプリ"));
    }

    #[test]
    fn never_responded_differs_from_zero_seconds() {
        let mut s = St::default();
        finish_read(&mut s, 1, ok(true, true, 1, None));
        let none = since_text(&s);
        finish_read(&mut s, 2, ok(true, true, 1, Some(0)));
        assert_ne!(none, since_text(&s));
    }

    #[test]
    fn screen_renders_aid_and_count() {
        let mut s = St::default();
        set_aid(&mut s, "F046414E44484501");
        finish_read(&mut s, 1, ok(true, true, 4, None));
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(joined.contains("AID: F046414E44484501"));
        assert!(joined.contains("応答した回数: 4"));
    }
}
