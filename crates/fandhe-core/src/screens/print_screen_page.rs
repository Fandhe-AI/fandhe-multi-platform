//! Phase 11（端末機能の拡充ループ、親 #341）型サンプル「画面の印刷」（Issue #1010）。
//! 押した直後の描画領域を 1 枚の画像として取り込み、1 ページに収めて OS の印刷ダイアログへ渡す
//! 1 ボタンの 1 画面。`FANDHE_PROXY_APP=print_screen_page` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod print_screen_page;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。状態・遷移
//! （`print` / `take_request` / `mark_printing` / `finish_print`）・結果の三値・文言は
//! [`print_page`](super::print_page) をそのまま共用し、本画面が持つのは見た目（色見本の図形）と
//! 画面固有の文言だけ。全 OS 分の状態・定数・文言はここで揃えてあり、後続の OS 実装は core を触らない。
//!
//! 処理中（`phase != Idle`）はボタンを外す。取り込まれる画像にボタンが写らないことの担保でもあり、
//! 二重押しの防止でもある。

use crate::token::ColorValue;
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

pub use super::print_page::{
    CoreOutcome, PrintFailure, PrintPageState, PrintPhase, error_text, finish_print, mark_printing,
    print, status_text, take_request,
};

/// 画面に出す補足（プリンタが無い環境での確かめ方）。
pub const NOTE_TEXT: &str =
    "押した直後の画面を 1 ページ印刷します。プリンタが無ければ OS の PDF 出力で確かめてください";
/// 見出し（日本語と ASCII の混在）。
pub const HEADING_TEXT: &str = "fandhe 画面印刷テスト / Screen print test";
/// 見本の文字（印刷結果と見比べられるよう数行置く）。
pub const SAMPLE_LINES: [&str; 3] = [
    "これは画面の内容を印刷する見本です。",
    "This screen is captured and printed.",
    "Line 3: 0123456789 ABCDEFG",
];
/// 「この画面を印刷する」ボタンの `WidgetId`。
pub const PRINT_SCREEN_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// ボタンの文言。
pub const PRINT_SCREEN_BUTTON_LABEL: &str = "この画面を印刷する";
/// 色見本キャンバスの `WidgetId`。
pub const SWATCH_ID: WidgetId = WidgetId::from_raw(2);
/// 画面を取り込めなかったとき（期限内に読み戻しが届かない・寸法不正・Web の複製の失敗）の文言。
/// `FAILED_PREFIX` の後ろに付く。proxy の結果写像と Web 実装が使う。
pub const CAPTURE_FAILED_TEXT: &str = "画面を取り込めませんでした";
/// 色見本キャンバスの一辺（論理ピクセル相当）。
pub const SWATCH_SIZE: f32 = 160.0;

/// 色見本（背景 + 円）の描画コマンド列。
pub fn paint(_s: &PrintPageState) -> Vec<PaintCommand> {
    vec![
        PaintCommand::StyledRect {
            fill: ColorValue::rgb(230, 240, 250),
        },
        PaintCommand::Circle {
            center: (SWATCH_SIZE / 2.0, SWATCH_SIZE / 2.0),
            radius: SWATCH_SIZE / 3.0,
            fill: ColorValue::rgb(200, 60, 60),
        },
    ]
}

/// 画面を組み立てる。処理中はボタンを外す。
pub fn current_screen(state: &PrintPageState) -> Screen<PrintPageState> {
    let mut screen = Screen::new("画面の印刷").text(NOTE_TEXT).text(HEADING_TEXT);
    for line in SAMPLE_LINES {
        screen = screen.text(line);
    }
    let screen = screen
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            SWATCH_ID,
            "色見本",
            Measurement {
                width: SWATCH_SIZE,
                height: SWATCH_SIZE,
            },
            paint,
        ))))
        .dynamic_text(status_text)
        .error_text(error_text);
    if state.phase != PrintPhase::Idle {
        return screen;
    }
    screen.button(PRINT_SCREEN_BUTTON_ID, PRINT_SCREEN_BUTTON_LABEL, print)
}

#[cfg(test)]
mod tests {
    use super::super::print_page as base;
    use super::*;

    // 印刷に権限は要らないため、権限拒否の失敗系は該当なし。

    #[test]
    fn button_hidden_while_busy() {
        let idle = current_screen(&PrintPageState::default());
        let mut s = PrintPageState::default();
        print(&mut s);
        mark_printing(&mut s);
        let busy = current_screen(&s);
        assert!(format!("{idle:?}").contains(PRINT_SCREEN_BUTTON_LABEL));
        assert!(!format!("{busy:?}").contains(PRINT_SCREEN_BUTTON_LABEL));
    }

    #[test]
    fn paint_has_rect_and_circle() {
        let cmds = paint(&PrintPageState::default());
        assert!(
            cmds.iter()
                .any(|c| matches!(c, PaintCommand::StyledRect { .. }))
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, PaintCommand::Circle { .. }))
        );
    }

    #[test]
    fn capture_failure_text_is_distinct_and_never_claims_printed() {
        let mut s = PrintPageState::default();
        print(&mut s);
        mark_printing(&mut s);
        let g = s.generation;
        finish_print(
            &mut s,
            g,
            Err(PrintFailure::Failed(CAPTURE_FAILED_TEXT.to_string())),
        );
        assert!(s.error.contains(CAPTURE_FAILED_TEXT));
        assert!(s.status.is_empty());
        for other in [
            base::SUBMITTED_STATUS,
            base::CANCELLED_STATUS,
            base::UNCONFIRMED_STATUS,
            base::UNSUPPORTED_STATUS,
        ] {
            assert_ne!(other, s.error);
            assert!(!other.contains(CAPTURE_FAILED_TEXT));
        }
        assert!(!s.error.contains("印刷しました"));
    }
}
