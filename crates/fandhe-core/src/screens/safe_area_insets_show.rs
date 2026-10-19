//! Phase 11 の拡充ループ（親 #105）型サンプル「セーフエリア」（Issue #444）。
//! `FANDHE_PROXY_APP=safe_area_insets_show` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod safe_area_insets_show;` と対）。Web は [`safe_area_insets_show_web_screen`] を
//! `fandhe-web-dom` の mount が使う。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。OS 値の取得は
//! `fandhe-egui-proxy` の `device/safe_area_insets.rs`（Web は `fandhe-web-dom`）が一定間隔で
//! 行い、[`apply_read`] で結果を注入する。`fandhe_device::safe_area_insets` の型は
//! 依存方向の都合でここに写し取る。
//!
//! 枠を画面の端に見立て、内側のパネルを OS が報告した余白の分だけ寄せて描く。パネルは
//! 読めたときだけ描く。取得中・非対応・失敗では描かず数値も出さない（0 などの代わりの値は
//! 嘘の成功になる）。失敗時・未取得に戻ったときは最後に読めた値を残さない。

use crate::token::ColorValue;
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 枠の幅（論理ピクセル相当）。
pub const FRAME_W: f32 = 240.0;
/// 枠の高さ（論理ピクセル相当）。
pub const FRAME_H: f32 = 200.0;

const CANVAS_ID: WidgetId = WidgetId::from_raw(1);

/// OS から読めた余白（`fandhe_device::safe_area_insets::Insets` の写し）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Insets {
    /// 上。
    pub top: f32,
    /// 右。
    pub right: f32,
    /// 下。
    pub bottom: f32,
    /// 左。
    pub left: f32,
}

/// 読めなかった理由。`DeviceError` からの写像は呼び出し元（proxy / web-dom）の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出すセーフエリアの状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SafeAreaView {
    /// 最初の値が届く前（OS から未取得を含む）。
    #[default]
    Reading,
    /// 読めた。
    Read(Insets),
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「セーフエリア」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SafeAreaInsetsShowState {
    /// 現在の表示状態。
    pub view: SafeAreaView,
}

/// 読み取り結果を反映する。`Ok(None)`（OS から未取得）は読み取り中に戻す。毎回 view を
/// 置き換えるため、失敗後・未取得に戻ったときに前の値は残らない。
pub fn apply_read(
    state: &mut SafeAreaInsetsShowState,
    result: Result<Option<Insets>, ReadFailure>,
) {
    state.view = match result {
        Ok(Some(i)) => SafeAreaView::Read(i),
        Ok(None) => SafeAreaView::Reading,
        Err(ReadFailure::Unsupported) => SafeAreaView::Unsupported,
        Err(ReadFailure::Failed(m)) => SafeAreaView::Failed(m),
    };
}

/// 枠とパネルの描画コマンド。読めたときだけ描き、それ以外は空。余白が枠を超える辺は
/// 枠いっぱいに詰め、内側が負の大きさにならないようにする。
/// `CanvasWidget::bound` の `paint` 関数として渡す。
pub fn panel_commands(state: &SafeAreaInsetsShowState) -> Vec<PaintCommand> {
    let SafeAreaView::Read(i) = &state.view else {
        return Vec::new();
    };
    let left = i.left.min(FRAME_W);
    let right = i.right.min(FRAME_W - left);
    let top = i.top.min(FRAME_H);
    let bottom = i.bottom.min(FRAME_H - top);
    vec![
        PaintCommand::StyledRect {
            fill: ColorValue::rgb(0x9A, 0x9A, 0x9A),
        },
        PaintCommand::InsetRect {
            left,
            top,
            right,
            bottom,
            fill: ColorValue::rgb(0x2F, 0x80, 0xED),
        },
    ]
}

/// 状態の文言。`render_headless` は Canvas を出力しないため、ヘッドレス検証と a11y の
/// 両方を本関数が担う。
pub fn status_text(state: &SafeAreaInsetsShowState) -> String {
    match &state.view {
        SafeAreaView::Reading => "セーフエリアを読み取っています".to_string(),
        SafeAreaView::Unsupported => "この環境ではセーフエリアを取得できません".to_string(),
        SafeAreaView::Failed(_) => "セーフエリア: 不明".to_string(),
        SafeAreaView::Read(i) => format!(
            "セーフエリア: 上 {} / 右 {} / 下 {} / 左 {}",
            i.top.round(),
            i.right.round(),
            i.bottom.round(),
            i.left.round()
        ),
    }
}

/// 失敗時のみ非空。
pub fn error_text(state: &SafeAreaInsetsShowState) -> String {
    match &state.view {
        SafeAreaView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 注記。OS ごとに見ている値が違う（iOS は key window、Android はシステムバーと切り欠きの
/// 合計、macOS は窓の中身で普段は 0）。
pub const NOTE: &str = "枠を画面の端に見立て、内側のパネルを OS が報告した余白の分だけ寄せて描きます。回転すると 1 秒ほどで反映されます。macOS は窓の中身の余白を見るため、普段は 0 です。";

/// Web 用の注記。viewport の `env()` を読むため、iPhone Safari 以外では 0 になることが多い。
pub const WEB_NOTE: &str = "ブラウザの env(safe-area-inset-*) を読んでいます。iPhone の Safari 以外では多くの場合 0 になります。";

fn base_screen() -> Screen<SafeAreaInsetsShowState> {
    Screen::new("セーフエリア")
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            CANVAS_ID,
            "セーフエリアの内側のパネル",
            Measurement {
                width: FRAME_W,
                height: FRAME_H,
            },
            panel_commands,
        ))))
        .dynamic_text(status_text)
        .error_text(error_text)
        .text(NOTE)
}

/// 「セーフエリア」画面。
pub fn safe_area_insets_show_screen() -> Screen<SafeAreaInsetsShowState> {
    base_screen()
}

/// Web 用の画面（[`WEB_NOTE`] を足した版）。`fandhe-web-dom` の mount が使う。
pub fn safe_area_insets_show_web_screen() -> Screen<SafeAreaInsetsShowState> {
    base_screen().text(WEB_NOTE)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &SafeAreaInsetsShowState) -> Screen<SafeAreaInsetsShowState> {
    safe_area_insets_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    const SAMPLE: Insets = Insets {
        top: 38.6,
        right: 0.0,
        bottom: 21.0,
        left: 0.0,
    };

    fn state(view: SafeAreaView) -> SafeAreaInsetsShowState {
        SafeAreaInsetsShowState { view }
    }

    #[test]
    fn non_read_states_draw_nothing_and_show_no_numbers() {
        for view in [
            SafeAreaView::Reading,
            SafeAreaView::Unsupported,
            SafeAreaView::Failed("セーフエリアの取得".to_string()),
        ] {
            let s = state(view);
            assert!(panel_commands(&s).is_empty(), "{:?}", s.view);
            assert!(!status_text(&s).chars().any(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn read_draws_frame_and_panel_with_text() {
        let s = state(SafeAreaView::Read(SAMPLE));
        let cmds = panel_commands(&s);
        assert_eq!(cmds.len(), 2);
        assert!(matches!(cmds[1], PaintCommand::InsetRect { top, .. } if top == 38.6));
        assert_eq!(status_text(&s), "セーフエリア: 上 39 / 右 0 / 下 21 / 左 0");
    }

    #[test]
    fn failure_drops_previous_read() {
        for r in [
            Err(ReadFailure::Unsupported),
            Err(ReadFailure::Failed("セーフエリアの取得".to_string())),
        ] {
            let mut s = SafeAreaInsetsShowState::default();
            apply_read(&mut s, Ok(Some(SAMPLE)));
            apply_read(&mut s, r);
            assert!(!matches!(s.view, SafeAreaView::Read(_)));
            assert!(panel_commands(&s).is_empty());
        }
    }

    #[test]
    fn not_yet_received_returns_to_reading() {
        let mut s = SafeAreaInsetsShowState::default();
        apply_read(&mut s, Ok(Some(SAMPLE)));
        apply_read(&mut s, Ok(None));
        assert_eq!(s.view, SafeAreaView::Reading);
        assert!(panel_commands(&s).is_empty());
    }

    #[test]
    fn oversized_insets_are_clamped_to_frame() {
        let s = state(SafeAreaView::Read(Insets {
            top: 500.0,
            right: 500.0,
            bottom: 500.0,
            left: 500.0,
        }));
        match &panel_commands(&s)[1] {
            PaintCommand::InsetRect {
                left,
                top,
                right,
                bottom,
                ..
            } => {
                assert_eq!((*left, *right), (FRAME_W, 0.0));
                assert_eq!((*top, *bottom), (FRAME_H, 0.0));
            }
            other => panic!("InsetRect を期待したが {other:?} だった"),
        }
    }

    #[test]
    fn error_text_only_when_failed() {
        let mut s = SafeAreaInsetsShowState::default();
        assert_eq!(error_text(&s), "");
        apply_read(
            &mut s,
            Err(ReadFailure::Failed("セーフエリアの取得".to_string())),
        );
        assert_eq!(error_text(&s), "失敗しました: セーフエリアの取得");
        apply_read(&mut s, Ok(Some(SAMPLE)));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn render_headless_does_not_panic() {
        let s = SafeAreaInsetsShowState::default();
        let joined = render_headless(&safe_area_insets_show_screen(), &s).join("\n");
        assert!(joined.contains("読み取っています"));
        assert!(!joined.contains(WEB_NOTE));
        let web = render_headless(&safe_area_insets_show_web_screen(), &s).join("\n");
        assert!(web.contains(WEB_NOTE));
    }
}
