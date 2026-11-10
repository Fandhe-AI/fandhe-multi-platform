//! Phase 11 の拡充ループ型サンプル「ディープリンク」（親 #153、Issue #1278）。
//! `FANDHE_PROXY_APP=deep_link_show` で選ばれ、
//! `crates/fandhe-gallery/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod deep_link_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS から届いた URL の検査・保持は
//! `fandhe_device::deep_link`、スナップショットを本モジュールへ注入するのは
//! `fandhe-egui-proxy` の effect ドライバ（`device/deep_link.rs`）の責務。
//!
//! OS 固有の操作 UI は置かない。リンクの検証も既定の設定も OS が持ち、アプリから登録・解除
//! する操作が無いため（url-scheme の Windows にある登録ボタンに当たるものが不要）。
//! 後続の OS 実装（Issue #1279 ほか）は本ファイルを触らない。
//!
//! 「使えない」と「まだ届いていない」は別の状態（[`DeepLinkView::Unsupported`] と
//! [`DeepLinkView::Waiting`]）で、文言も分ける。弾いた URL は中身を表示せず件数だけ出す。

use crate::Screen;
use crate::screens::background_task::format_utc;

/// 受信状態の表示区分。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DeepLinkView {
    /// まだ読み取っていない（effect ドライバが最初の結果を注入するまで）。
    #[default]
    Unread,
    /// この OS ではディープリンクを受信できない（失敗ではない）。
    Unsupported,
    /// 受信できる状態だが、まだ URL を受理していない。
    Waiting,
    /// 最後に受理した URL と受信時刻（UNIX ms）。
    Received {
        /// 検査済みの URL 文字列。
        url: String,
        /// 受信時刻。
        received_at_unix_ms: u64,
    },
}

/// 「ディープリンク」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeepLinkShowState {
    view: DeepLinkView,
    rejected: u64,
    seq: Option<u64>,
}

impl DeepLinkShowState {
    /// 最後に適用したスナップショットの世代番号（effect ドライバが変化検出に使う）。
    /// 未適用なら `None`。
    pub fn seq(&self) -> Option<u64> {
        self.seq
    }

    /// 非対応として確定済みか（effect ドライバが一度だけ送るために使う）。
    pub fn is_unsupported(&self) -> bool {
        self.view == DeepLinkView::Unsupported
    }
}

/// 受信状態のスナップショットを反映する。`last` は `(url, 受信時刻 UNIX ms)`。
/// 適用済みの世代以下のスナップショット（古い世代）は捨てる。
pub fn apply_snapshot(
    state: &mut DeepLinkShowState,
    seq: u64,
    last: Option<(String, u64)>,
    rejected: u64,
) {
    if state.seq.is_some_and(|applied| applied >= seq) {
        return;
    }
    state.view = match last {
        Some((url, received_at_unix_ms)) => DeepLinkView::Received {
            url,
            received_at_unix_ms,
        },
        None => DeepLinkView::Waiting,
    };
    state.rejected = rejected;
    state.seq = Some(seq);
}

/// この OS では受信できないことを反映する。
pub fn apply_unsupported(state: &mut DeepLinkShowState) {
    state.view = DeepLinkView::Unsupported;
}

/// 状態行の文言。
pub fn status_text(state: &DeepLinkShowState) -> String {
    match &state.view {
        DeepLinkView::Unread => "読み取り中".to_string(),
        DeepLinkView::Unsupported => "この OS では未対応です".to_string(),
        DeepLinkView::Waiting => {
            "まだ受け取っていません（検証済みドメインの https リンクを開くと届きます）".to_string()
        }
        DeepLinkView::Received { .. } => "受け取りました".to_string(),
    }
}

/// 最後に受け取った URL（未受信なら空）。
pub fn url_text(state: &DeepLinkShowState) -> String {
    match &state.view {
        DeepLinkView::Received { url, .. } => format!("URL: {url}"),
        _ => String::new(),
    }
}

/// 受信時刻（未受信なら空）。
pub fn time_text(state: &DeepLinkShowState) -> String {
    match &state.view {
        DeepLinkView::Received {
            received_at_unix_ms,
            ..
        } => format!("受信時刻: {}", format_utc(*received_at_unix_ms)),
        _ => String::new(),
    }
}

/// 検査で弾いた件数の文言。
pub fn rejected_text(state: &DeepLinkShowState) -> String {
    format!("検査で弾いた件数: {}", state.rejected)
}

/// 「ディープリンク」画面。
pub fn deep_link_show_screen() -> Screen<DeepLinkShowState> {
    Screen::new("ディープリンク")
        .dynamic_text(status_text)
        .dynamic_text(url_text)
        .dynamic_text(time_text)
        .dynamic_text(rejected_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &DeepLinkShowState) -> Screen<DeepLinkShowState> {
    deep_link_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn received() -> DeepLinkShowState {
        let mut s = DeepLinkShowState::default();
        apply_snapshot(&mut s, 1, Some(("https://example.com/a".into(), 0)), 0);
        s
    }

    #[test]
    fn status_texts_differ_for_each_view() {
        let mut unsupported = DeepLinkShowState::default();
        apply_unsupported(&mut unsupported);
        let mut waiting = DeepLinkShowState::default();
        apply_snapshot(&mut waiting, 0, None, 0);
        let all = [
            status_text(&DeepLinkShowState::default()),
            status_text(&unsupported),
            status_text(&waiting),
            status_text(&received()),
        ];
        for i in 0..all.len() {
            for j in 0..i {
                assert_ne!(all[i], all[j]);
            }
        }
    }

    #[test]
    fn unread_and_unsupported_show_no_url_or_time() {
        let mut unsupported = DeepLinkShowState::default();
        apply_unsupported(&mut unsupported);
        assert!(unsupported.is_unsupported());
        for s in [DeepLinkShowState::default(), unsupported.clone()] {
            assert_eq!(url_text(&s), "");
            assert_eq!(time_text(&s), "");
        }
        assert!(!status_text(&unsupported).contains("受け取りました"));
    }

    #[test]
    fn stale_or_same_seq_is_ignored() {
        let mut s = received();
        apply_snapshot(&mut s, 1, None, 9);
        apply_snapshot(&mut s, 0, None, 9);
        assert_eq!(s, received());
        apply_snapshot(&mut s, 2, None, 1);
        assert_eq!(s.seq(), Some(2));
        assert_eq!(
            status_text(&s),
            status_text(&{
                let mut w = DeepLinkShowState::default();
                apply_snapshot(&mut w, 0, None, 0);
                w
            })
        );
    }

    #[test]
    fn renders_headless() {
        let state = received();
        let joined = render_headless(&current_screen(&state), &state).join("\n");
        assert!(joined.contains("URL: https://example.com/a"));
    }
}
