//! Phase 11 の拡充ループ（親 #103）型サンプル「システムのアクセント色」（Issue #421）。
//! `FANDHE_PROXY_APP=system_accent_color_show` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod system_accent_color_show;` と対）。Web は [`system_accent_color_show_web_screen`] を
//! `fandhe-web-dom` の mount が使う。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `fandhe-egui-proxy` の `device/system_accent_color.rs`（Web は `fandhe-web-dom`）が一定間隔で
//! 行い、[`apply_read`] で結果を注入する。`fandhe_device::system_accent_color` の型は
//! 依存方向の都合でここに写し取る。
//!
//! 色見本は読めたときだけ描く。取得中・非対応・失敗では見本も `#` 表記も出さない
//! （灰色などの代わりの色は嘘の成功になる）。失敗時は最後に読めた値を残さない。
//! アプリのテーマ色・トークンへの反映は扱わない。

use crate::token::ColorValue;
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 色見本の幅（論理ピクセル相当）。
pub const SWATCH_W: f32 = 160.0;
/// 色見本の高さ（論理ピクセル相当）。
pub const SWATCH_H: f32 = 80.0;

const CANVAS_ID: WidgetId = WidgetId::from_raw(1);

/// OS から読めた色（`fandhe_device::system_accent_color::Rgb` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// 赤。
    pub r: u8,
    /// 緑。
    pub g: u8,
    /// 青。
    pub b: u8,
}

/// 読めなかった理由。`DeviceError` からの写像は呼び出し元（proxy / web-dom）の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出すアクセント色の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AccentView {
    /// 最初の値が届く前。
    #[default]
    Reading,
    /// 読めた。
    Read(Rgb),
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「システムのアクセント色」画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemAccentColorShowState {
    /// 現在の表示状態。
    pub view: AccentView,
}

/// 読み取り結果を反映する。毎回 view を置き換えるため、失敗後に前の値は残らない。
pub fn apply_read(state: &mut SystemAccentColorShowState, result: Result<Rgb, ReadFailure>) {
    state.view = match result {
        Ok(c) => AccentView::Read(c),
        Err(ReadFailure::Unsupported) => AccentView::Unsupported,
        Err(ReadFailure::Failed(m)) => AccentView::Failed(m),
    };
}

/// `#RRGGBB`（大文字）。
pub fn hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

/// 色見本の描画コマンド。読めたときだけ塗り、それ以外は空（代わりの色を描かない）。
/// `CanvasWidget::bound` の `paint` 関数として渡す。
pub fn swatch_commands(state: &SystemAccentColorShowState) -> Vec<PaintCommand> {
    match &state.view {
        AccentView::Read(c) => vec![PaintCommand::StyledRect {
            fill: ColorValue::rgb(c.r, c.g, c.b),
        }],
        _ => Vec::new(),
    }
}

/// 状態の文言。`render_headless` は Canvas を出力しないため、ヘッドレス検証と a11y の
/// 両方を本関数が担う。
pub fn status_text(state: &SystemAccentColorShowState) -> String {
    match &state.view {
        AccentView::Reading => "アクセント色を読み取っています".to_string(),
        AccentView::Unsupported => "この環境ではアクセント色を取得できません".to_string(),
        AccentView::Failed(_) => "アクセント色: 不明".to_string(),
        AccentView::Read(c) => format!("アクセント色: {}", hex(*c)),
    }
}

/// 失敗時のみ非空。
pub fn error_text(state: &SystemAccentColorShowState) -> String {
    match &state.view {
        AccentView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 注記（反映の遅れ）。
pub const NOTE: &str = "OS の設定変更は最大 1 秒ほどで反映されます。";

/// Web 用の注記。ブラウザは OS の設定と無関係な固定色を返すことが多い。
pub const WEB_NOTE: &str =
    "ブラウザが返したアクセント色です。多くのブラウザは OS の設定に関係なく固定の青を返します。";

fn base_screen() -> Screen<SystemAccentColorShowState> {
    Screen::new("システムのアクセント色")
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            CANVAS_ID,
            "アクセント色の見本",
            Measurement {
                width: SWATCH_W,
                height: SWATCH_H,
            },
            swatch_commands,
        ))))
        .dynamic_text(status_text)
        .error_text(error_text)
        .text(NOTE)
}

/// 「システムのアクセント色」画面。
pub fn system_accent_color_show_screen() -> Screen<SystemAccentColorShowState> {
    base_screen()
}

/// Web 用の画面（[`WEB_NOTE`] を足した版）。`fandhe-web-dom` の mount が使う。
pub fn system_accent_color_show_web_screen() -> Screen<SystemAccentColorShowState> {
    base_screen().text(WEB_NOTE)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &SystemAccentColorShowState) -> Screen<SystemAccentColorShowState> {
    system_accent_color_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    const BLUE: Rgb = Rgb {
        r: 0x0A,
        g: 0x84,
        b: 0xFF,
    };

    fn state(view: AccentView) -> SystemAccentColorShowState {
        SystemAccentColorShowState { view }
    }

    #[test]
    fn non_read_states_draw_nothing_and_show_no_hex() {
        for view in [
            AccentView::Reading,
            AccentView::Unsupported,
            AccentView::Failed("アクセント色の取得".to_string()),
        ] {
            let s = state(view);
            assert!(swatch_commands(&s).is_empty(), "{:?}", s.view);
            assert!(!status_text(&s).contains('#'));
        }
    }

    #[test]
    fn read_draws_swatch_and_hex() {
        let s = state(AccentView::Read(BLUE));
        assert_eq!(swatch_commands(&s).len(), 1);
        assert_eq!(status_text(&s), "アクセント色: #0A84FF");
    }

    #[test]
    fn failure_drops_previous_read() {
        for r in [
            Err(ReadFailure::Unsupported),
            Err(ReadFailure::Failed("アクセント色の取得".to_string())),
        ] {
            let mut s = SystemAccentColorShowState::default();
            apply_read(&mut s, Ok(BLUE));
            apply_read(&mut s, r);
            assert!(!matches!(s.view, AccentView::Read(_)));
            assert!(swatch_commands(&s).is_empty());
        }
    }

    #[test]
    fn hex_is_zero_padded_uppercase() {
        assert_eq!(hex(BLUE), "#0A84FF");
        assert_eq!(hex(Rgb { r: 0, g: 0, b: 0 }), "#000000");
    }

    #[test]
    fn error_text_only_when_failed() {
        let mut s = SystemAccentColorShowState::default();
        assert_eq!(error_text(&s), "");
        apply_read(
            &mut s,
            Err(ReadFailure::Failed("アクセント色の取得".to_string())),
        );
        assert_eq!(error_text(&s), "失敗しました: アクセント色の取得");
        apply_read(&mut s, Ok(BLUE));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn render_headless_does_not_panic() {
        let s = SystemAccentColorShowState::default();
        let joined = render_headless(&system_accent_color_show_screen(), &s).join("\n");
        assert!(joined.contains("読み取っています"));
        assert!(!joined.contains(WEB_NOTE));
        let web = render_headless(&system_accent_color_show_web_screen(), &s).join("\n");
        assert!(web.contains(WEB_NOTE));
    }
}
