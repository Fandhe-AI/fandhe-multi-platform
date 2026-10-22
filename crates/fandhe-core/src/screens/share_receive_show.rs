//! Phase 11 の拡充ループ（親 #122）型サンプル「共有の受け取り」（Issue #606）。
//! `FANDHE_PROXY_APP=share_receive_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod share_receive_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS から届いたテキストの検査・保持は
//! `fandhe_device::share_receive`、スナップショットを本モジュールへ注入するのは
//! `fandhe-egui-proxy` の effect ドライバ（`device/share_receive.rs`）の責務。
//!
//! 全対象 OS（macOS・Android・Web）分の状態・定数・文言を本モジュールで持つ。本機能には
//! OS 固有の登録・解除ボタンが無く（macOS は plist、Android・Web は manifest の宣言で済む）、
//! OS ごとの差は [`ShareGuide`] の案内文言だけなので、後続の OS 別の子 issue はこのファイルに
//! 触れない。「使えない」と「まだ届いていない」は別の状態で、文言も分ける。
//! 弾いた入力は中身を表示せず件数だけ出す。

use crate::Screen;
use crate::screens::background_task::format_utc;

/// 画面に出す本文の最大文字数。表示は文字単位で切る（バイト単位で切らない）。
pub const DISPLAY_MAX_CHARS: usize = 2000;

/// macOS のサービスメニュー項目名。`scripts/apple/bundle-macos-url-scheme.sh` の
/// `NSMenuItem.default` と一致させる（双方のコメントで相互参照する）。
pub const SERVICE_MENU_TITLE: &str = "Fandhe で受け取る";

/// 受け取り方の案内の種別。effect ドライバ（macOS・Android）または Web のエントリが
/// 一度だけ注入する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShareGuide {
    /// 案内なし（未注入、または対応 OS でない）。
    #[default]
    None,
    /// macOS: サービスメニュー。
    MacServices,
    /// Android: 共有シート（`ACTION_SEND`）。
    AndroidShare,
    /// Web: インストールした PWA を共有先に選ぶ（`share_target`）。
    WebPwa,
}

/// 受信状態の表示区分。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ShareReceiveView {
    /// まだ読み取っていない（effect ドライバが最初の結果を注入するまで）。
    #[default]
    Unread,
    /// この OS では共有を受け取れない（失敗ではない）。
    Unsupported,
    /// 受け取れる状態だが、まだ何も受理していない。
    Waiting,
    /// 最後に受理したテキストと受信時刻（UNIX ms）。
    Received {
        /// 検査済みのテキスト（全文。表示時に [`DISPLAY_MAX_CHARS`] で切る）。
        text: String,
        /// 受信時刻。
        received_at_unix_ms: u64,
    },
}

/// 「共有の受け取り」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShareReceiveShowState {
    view: ShareReceiveView,
    rejected: u64,
    seq: Option<u64>,
    guide: ShareGuide,
}

impl ShareReceiveShowState {
    /// 最後に適用したスナップショットの世代番号（effect ドライバが変化検出に使う）。
    pub fn seq(&self) -> Option<u64> {
        self.seq
    }

    /// 非対応として確定済みか（effect ドライバが一度だけ送るために使う）。
    pub fn is_unsupported(&self) -> bool {
        self.view == ShareReceiveView::Unsupported
    }

    /// 注入済みの案内種別（effect ドライバが一度だけ送るために使う）。
    pub fn guide(&self) -> ShareGuide {
        self.guide
    }
}

/// 受信状態のスナップショットを反映する。`last` は `(テキスト, 受信時刻 UNIX ms)`。
/// 既に反映した世代より古い `seq` は捨てる（遅れて届いた完了で表示を戻さない）。
pub fn apply_snapshot(
    state: &mut ShareReceiveShowState,
    seq: u64,
    last: Option<(String, u64)>,
    rejected: u64,
) {
    if matches!(state.seq, Some(prev) if seq < prev) {
        return;
    }
    state.view = match last {
        Some((text, received_at_unix_ms)) => ShareReceiveView::Received {
            text,
            received_at_unix_ms,
        },
        None => ShareReceiveView::Waiting,
    };
    state.rejected = rejected;
    state.seq = Some(seq);
}

/// この OS では受け取れないことを反映する。
pub fn apply_unsupported(state: &mut ShareReceiveShowState) {
    state.view = ShareReceiveView::Unsupported;
}

/// 受け取り方の案内種別を反映する。
pub fn set_guide(state: &mut ShareReceiveShowState, guide: ShareGuide) {
    state.guide = guide;
}

/// 状態行の文言。Web は共有先になれるかを判定できないため「使えます」とは書かない。
pub fn status_text(state: &ShareReceiveShowState) -> String {
    match &state.view {
        ShareReceiveView::Unread => "読み取り中".to_string(),
        ShareReceiveView::Unsupported => "この OS では未対応です".to_string(),
        ShareReceiveView::Waiting if state.guide == ShareGuide::WebPwa => {
            "まだ受け取っていません（このブラウザが共有先になれるかは判定できません）".to_string()
        }
        ShareReceiveView::Waiting => "まだ受け取っていません".to_string(),
        ShareReceiveView::Received { .. } => "受け取りました".to_string(),
    }
}

/// 受け取り方の案内文言。未対応・読み取り中・案内なしでは空。
pub fn hint_text(state: &ShareReceiveShowState) -> String {
    if !matches!(
        state.view,
        ShareReceiveView::Waiting | ShareReceiveView::Received { .. }
    ) {
        return String::new();
    }
    match state.guide {
        ShareGuide::None => String::new(),
        ShareGuide::MacServices => format!(
            "テキストを選び、サービスメニューの『{SERVICE_MENU_TITLE}』から送ると届きます（.app 化して起動したときだけ）"
        ),
        ShareGuide::AndroidShare => {
            "他のアプリで「共有」を選び、このアプリを選ぶと届きます".to_string()
        }
        ShareGuide::WebPwa => {
            "このアプリをインストールして共有先に選んだときだけ届きます（Chromium 系）".to_string()
        }
    }
}

/// 受け取った本文（先頭 [`DISPLAY_MAX_CHARS`] 文字。未受信なら空）。
pub fn body_text(state: &ShareReceiveShowState) -> String {
    match &state.view {
        ShareReceiveView::Received { text, .. } => text.chars().take(DISPLAY_MAX_CHARS).collect(),
        _ => String::new(),
    }
}

/// 文字数の文言。上限を超えたら先頭だけを表示していることを併記する。
pub fn length_text(state: &ShareReceiveShowState) -> String {
    match &state.view {
        ShareReceiveView::Received { text, .. } => {
            let n = text.chars().count();
            if n > DISPLAY_MAX_CHARS {
                format!("文字数: {n}（先頭 2,000 文字を表示）")
            } else {
                format!("文字数: {n}")
            }
        }
        _ => String::new(),
    }
}

/// 受信時刻（未受信なら空）。
pub fn time_text(state: &ShareReceiveShowState) -> String {
    match &state.view {
        ShareReceiveView::Received {
            received_at_unix_ms,
            ..
        } => format!("受信時刻: {}", format_utc(*received_at_unix_ms)),
        _ => String::new(),
    }
}

/// 検査で弾いた件数の文言（弾いた中身は出さない）。
pub fn rejected_text(state: &ShareReceiveShowState) -> String {
    format!("検査で弾いた件数: {}", state.rejected)
}

/// 「共有の受け取り」画面。ボタンは無い。
pub fn share_receive_show_screen() -> Screen<ShareReceiveShowState> {
    Screen::new("共有の受け取り")
        .dynamic_text(status_text)
        .dynamic_text(hint_text)
        .dynamic_text(body_text)
        .dynamic_text(length_text)
        .dynamic_text(time_text)
        .dynamic_text(rejected_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &ShareReceiveShowState) -> Screen<ShareReceiveShowState> {
    share_receive_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn received(text: &str, guide: ShareGuide) -> ShareReceiveShowState {
        let mut s = ShareReceiveShowState::default();
        set_guide(&mut s, guide);
        apply_snapshot(&mut s, 1, Some((text.to_string(), 0)), 0);
        s
    }

    #[test]
    fn unsupported_and_waiting_have_different_text() {
        let mut unsupported = ShareReceiveShowState::default();
        apply_unsupported(&mut unsupported);
        let mut waiting = ShareReceiveShowState::default();
        apply_snapshot(&mut waiting, 0, None, 0);
        assert_ne!(status_text(&unsupported), status_text(&waiting));
        assert_eq!(hint_text(&unsupported), "");
        assert_eq!(hint_text(&ShareReceiveShowState::default()), "");
    }

    #[test]
    fn web_waiting_does_not_claim_availability() {
        let mut s = ShareReceiveShowState::default();
        set_guide(&mut s, ShareGuide::WebPwa);
        apply_snapshot(&mut s, 0, None, 0);
        assert!(!status_text(&s).contains("使えます"));
        assert!(status_text(&s).contains("判定できません"));
    }

    #[test]
    fn body_is_cut_by_chars_not_bytes() {
        let exact = "あ".repeat(DISPLAY_MAX_CHARS);
        let s = received(&exact, ShareGuide::None);
        assert_eq!(body_text(&s).chars().count(), DISPLAY_MAX_CHARS);
        assert_eq!(length_text(&s), "文字数: 2000");

        let over = format!("{exact}い");
        let s = received(&over, ShareGuide::None);
        assert_eq!(body_text(&s), exact);
        assert!(length_text(&s).contains("文字数: 2001"));
        assert!(length_text(&s).contains("先頭 2,000 文字を表示"));
    }

    #[test]
    fn stale_snapshot_does_not_revert_state() {
        let mut s = ShareReceiveShowState::default();
        apply_snapshot(&mut s, 5, Some(("new".to_string(), 1)), 0);
        apply_snapshot(&mut s, 3, None, 0);
        assert_eq!(s.seq(), Some(5));
        assert_eq!(status_text(&s), "受け取りました");
    }

    #[test]
    fn rejected_line_never_shows_content() {
        let mut s = ShareReceiveShowState::default();
        apply_snapshot(&mut s, 2, None, 2);
        let joined = render_headless(&share_receive_show_screen(), &s).join("\n");
        assert!(joined.contains("検査で弾いた件数: 2"));
        assert_eq!(rejected_text(&s), "検査で弾いた件数: 2");
    }

    #[test]
    fn guides_have_distinct_hints() {
        let hints: Vec<String> = [
            ShareGuide::MacServices,
            ShareGuide::AndroidShare,
            ShareGuide::WebPwa,
        ]
        .into_iter()
        .map(|g| {
            let mut s = ShareReceiveShowState::default();
            set_guide(&mut s, g);
            apply_snapshot(&mut s, 0, None, 0);
            hint_text(&s)
        })
        .collect();
        assert!(hints.iter().all(|h| !h.is_empty()));
        assert_ne!(hints[0], hints[1]);
        assert_ne!(hints[1], hints[2]);
        assert_ne!(hints[0], hints[2]);
        assert!(hints[0].contains(SERVICE_MENU_TITLE));
    }

    #[test]
    fn received_renders() {
        let s = received("hello", ShareGuide::MacServices);
        let joined = render_headless(&share_receive_show_screen(), &s).join("\n");
        assert!(joined.contains("受け取りました"));
        assert!(joined.contains("hello"));
    }
}
