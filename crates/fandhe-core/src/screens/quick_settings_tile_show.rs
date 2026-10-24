//! Phase 11 の拡充ループ（親 #139）型サンプル「クイック設定タイル・コントロール」（Issue #719）。
//! OS のシステム UI に置いた 1 操作のボタンが押された回数を表示する 1 画面。
//! `FANDHE_PROXY_APP=quick_settings_tile_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod quick_settings_tile_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。回数の読み出しは
//! `fandhe_device::quick_settings_tile`、結果を本モジュールへ注入するのは `fandhe-egui-proxy` の
//! effect ドライバ（`device/quick_settings_tile.rs`）の責務。ドライバは読み出しを高々 1 件に
//! 絞り、世代番号つきで [`finish_read`] へ届ける。
//!
//! 全対象 OS（macOS・Android）分の状態・定数・文言を本モジュールで持つ。本機能には操作ボタンが
//! 無く（追加はユーザーが OS の UI から行う）、OS ごとの差は [`TileGuide`] の案内文言だけなので、
//! 後続の OS 別の子 issue はこのファイルに触れない。「0 回」「読み出せない」「未対応」は別の
//! 状態で、文言も分ける。読み出せない理由（パス・OS の文言）は表示しない。

use crate::Screen;

/// macOS のコントロール名。`platforms/apple/FandheTextWidget/Sources/FandheTextWidget/
/// FandheCountControl.swift` の `ControlWidget` の表示名と一致させる（双方のコメントで相互参照する）。
pub const CONTROL_TITLE: &str = "Fandhe カウント";

/// Android のクイック設定タイル名。後続の b-android が `strings.xml` / Manifest のラベルに使う。
pub const TILE_LABEL: &str = "Fandhe カウント";

/// 追加方法の案内の種別。effect ドライバが一度だけ注入する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileGuide {
    /// 案内なし（未注入、または対応 OS でない）。
    #[default]
    None,
    /// macOS: コントロールセンター / メニューバーのコントロール。
    MacControlCenter,
    /// Android: クイック設定のタイル。
    AndroidQuickSettings,
}

/// 読み出し結果（端末依存の型を持ち込まないための写像先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    /// 回数が読めた。未押下は `count == 0`。
    Count {
        /// 押された回数。
        count: u64,
        /// このプロセス内で保存に失敗した押下の件数（取れる OS だけ `Some`）。
        failed_presses: Option<u64>,
    },
    /// 読み出しに失敗した（0 とは別）。
    Failed,
}

/// 表示区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    /// まだ読んでいない。
    #[default]
    Unread,
    /// この OS では使えない（失敗ではない）。
    Unsupported,
    /// 回数が読めた。
    Count(u64),
    /// 読み出しに失敗した。
    ReadFailed,
}

/// 「クイック設定タイル・コントロール」画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickSettingsTileShowState {
    view: View,
    failed_presses: Option<u64>,
    guide: TileGuide,
    /// 最後に適用した読み出しの世代。これより古い完了は捨てる。
    applied_gen: Option<u64>,
}

impl QuickSettingsTileShowState {
    /// 非対応として確定済みか（effect ドライバが読み出しをやめるために使う）。
    pub fn is_unsupported(&self) -> bool {
        self.view == View::Unsupported
    }

    /// 注入済みの案内種別（effect ドライバが一度だけ送るために使う）。
    pub fn guide(&self) -> TileGuide {
        self.guide
    }

    /// 最後に適用した読み出しの世代。
    pub fn applied_gen(&self) -> Option<u64> {
        self.applied_gen
    }
}

/// 読み出し結果を反映する。適用済みより古い世代、および非対応確定後の完了は捨てる
/// （遅れて届いた古い成功で新しい失敗表示を上書きしない、非対応を戻さない）。
pub fn finish_read(state: &mut QuickSettingsTileShowState, generation: u64, outcome: ReadOutcome) {
    if state.view == View::Unsupported {
        return;
    }
    if matches!(state.applied_gen, Some(prev) if generation < prev) {
        return;
    }
    state.applied_gen = Some(generation);
    match outcome {
        ReadOutcome::Count {
            count,
            failed_presses,
        } => {
            state.view = View::Count(count);
            state.failed_presses = failed_presses;
        }
        ReadOutcome::Failed => {
            state.view = View::ReadFailed;
            state.failed_presses = None;
        }
    }
}

/// この OS では使えないことを反映する。以後の読み出し結果は無視する。
pub fn apply_unsupported(state: &mut QuickSettingsTileShowState) {
    state.view = View::Unsupported;
    state.failed_presses = None;
}

/// 追加方法の案内種別を反映する。
pub fn set_guide(state: &mut QuickSettingsTileShowState, guide: TileGuide) {
    state.guide = guide;
}

/// 状態行の文言。
pub fn status_text(state: &QuickSettingsTileShowState) -> String {
    match state.view {
        View::Unread => "読み取り中".to_string(),
        View::Unsupported => "この OS では未対応です".to_string(),
        View::Count(n) => format!("押された回数: {n}"),
        View::ReadFailed => "回数を読み出せません".to_string(),
    }
}

/// 追加方法の案内文言。読み取り中・案内なしでは空。未対応では macOS の前提だけを出す。
pub fn hint_text(state: &QuickSettingsTileShowState) -> String {
    match (state.view, state.guide) {
        (View::Unread, _) | (_, TileGuide::None) => String::new(),
        (View::Unsupported, TileGuide::MacControlCenter) => {
            "macOS 26 以降で、環境変数 FANDHE_DEVELOPMENT_TEAM を設定して FandheWidgetHost を導入した場合だけ使えます"
                .to_string()
        }
        (View::Unsupported, TileGuide::AndroidQuickSettings) => String::new(),
        (_, TileGuide::MacControlCenter) => format!(
            "コントロールセンターまたはメニューバーの編集から『{CONTROL_TITLE}』を追加して押すと増えます（macOS 26 以降・FandheWidgetHost の導入が必要）"
        ),
        (_, TileGuide::AndroidQuickSettings) => format!(
            "クイック設定を開き、編集から『{TILE_LABEL}』タイルを追加して押すと増えます"
        ),
    }
}

/// 保存に失敗した押下の件数の文言。件数が取れる OS（`Some`）だけ出し、他は空。
pub fn failed_presses_text(state: &QuickSettingsTileShowState) -> String {
    match (state.view, state.failed_presses) {
        (View::Count(_), Some(n)) => format!("保存に失敗した押下: {n} 回（このプロセス内）"),
        _ => String::new(),
    }
}

/// 「クイック設定タイル・コントロール」画面。操作ボタンは無い。
pub fn quick_settings_tile_show_screen() -> Screen<QuickSettingsTileShowState> {
    Screen::new("クイック設定タイル・コントロール")
        .dynamic_text(status_text)
        .dynamic_text(hint_text)
        .dynamic_text(failed_presses_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &QuickSettingsTileShowState) -> Screen<QuickSettingsTileShowState> {
    quick_settings_tile_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn count(n: u64, failed: Option<u64>) -> ReadOutcome {
        ReadOutcome::Count {
            count: n,
            failed_presses: failed,
        }
    }

    #[test]
    fn states_have_distinct_text() {
        let unread = QuickSettingsTileShowState::default();
        let mut zero = QuickSettingsTileShowState::default();
        finish_read(&mut zero, 1, count(0, None));
        let mut failed = QuickSettingsTileShowState::default();
        finish_read(&mut failed, 1, ReadOutcome::Failed);
        let mut unsupported = QuickSettingsTileShowState::default();
        apply_unsupported(&mut unsupported);

        assert_eq!(status_text(&unread), "読み取り中");
        assert_eq!(status_text(&zero), "押された回数: 0");
        assert_eq!(status_text(&failed), "回数を読み出せません");
        assert_eq!(status_text(&unsupported), "この OS では未対応です");
        let texts = [
            status_text(&unread),
            status_text(&zero),
            status_text(&failed),
            status_text(&unsupported),
        ];
        for (i, a) in texts.iter().enumerate() {
            for b in &texts[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(!status_text(&failed).contains('0'));
    }

    #[test]
    fn stale_ok_does_not_overwrite_newer_failure() {
        let mut s = QuickSettingsTileShowState::default();
        finish_read(&mut s, 2, ReadOutcome::Failed);
        finish_read(&mut s, 1, count(5, None));
        assert_eq!(status_text(&s), "回数を読み出せません");
        assert_eq!(s.applied_gen(), Some(2));
    }

    #[test]
    fn failure_recovers_on_newer_success() {
        let mut s = QuickSettingsTileShowState::default();
        finish_read(&mut s, 1, ReadOutcome::Failed);
        finish_read(&mut s, 2, count(3, None));
        assert_eq!(status_text(&s), "押された回数: 3");
    }

    #[test]
    fn unsupported_is_terminal() {
        let mut s = QuickSettingsTileShowState::default();
        apply_unsupported(&mut s);
        finish_read(&mut s, 9, count(7, Some(1)));
        assert!(s.is_unsupported());
        assert_eq!(status_text(&s), "この OS では未対応です");
        assert_eq!(failed_presses_text(&s), "");
    }

    #[test]
    fn failed_presses_only_when_some() {
        let mut s = QuickSettingsTileShowState::default();
        finish_read(&mut s, 1, count(2, None));
        assert_eq!(failed_presses_text(&s), "");
        finish_read(&mut s, 2, count(2, Some(0)));
        assert_eq!(
            failed_presses_text(&s),
            "保存に失敗した押下: 0 回（このプロセス内）"
        );
        finish_read(&mut s, 3, ReadOutcome::Failed);
        assert_eq!(failed_presses_text(&s), "");
    }

    #[test]
    fn guides_are_distinct_per_os() {
        let hint = |g| {
            let mut s = QuickSettingsTileShowState::default();
            set_guide(&mut s, g);
            finish_read(&mut s, 1, count(0, None));
            hint_text(&s)
        };
        let mac = hint(TileGuide::MacControlCenter);
        let android = hint(TileGuide::AndroidQuickSettings);
        assert!(mac.contains(CONTROL_TITLE));
        assert!(android.contains(TILE_LABEL));
        assert_ne!(mac, android);
        assert_eq!(hint(TileGuide::None), "");
        assert_eq!(hint_text(&QuickSettingsTileShowState::default()), "");
    }

    #[test]
    fn unsupported_hint_names_mac_prerequisites_only() {
        let mut s = QuickSettingsTileShowState::default();
        set_guide(&mut s, TileGuide::MacControlCenter);
        apply_unsupported(&mut s);
        assert!(hint_text(&s).contains("macOS 26"));
        assert!(hint_text(&s).contains("FANDHE_DEVELOPMENT_TEAM"));
        set_guide(&mut s, TileGuide::None);
        assert_eq!(hint_text(&s), "");
    }

    #[test]
    fn screen_renders_count() {
        let mut s = QuickSettingsTileShowState::default();
        set_guide(&mut s, TileGuide::MacControlCenter);
        finish_read(&mut s, 1, count(4, None));
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(joined.contains("押された回数: 4"));
        assert!(joined.contains(CONTROL_TITLE));
    }
}
