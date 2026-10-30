//! Phase 11 の拡充ループ（親 #141）型サンプル「折りたたみの姿勢」（Issue #723）。
//! `FANDHE_PROXY_APP=foldable_posture_show` で選ばれ、`fandhe-egui-proxy` の
//! `screens/registry.rs` の `effect` 区画から呼ばれる（`screens/mod.rs` の
//! `pub mod foldable_posture_show;` と対）。Web は [`foldable_posture_show_web_screen`] を
//! `fandhe-web-dom` の mount が使う。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`）。値の取得は
//! `fandhe-egui-proxy` の `device/foldable_posture.rs`（Web は `fandhe-web-dom`）が一定間隔で
//! 行い、[`apply_read`] で結果を注入する。`fandhe_device::foldable_posture` の型は
//! 依存方向の都合でここに写し取る。
//!
//! 枠を画面に見立て、折れていて折れ目が分かるときだけ折れ目に沿って 2 枚のパネルに分けて描く。
//! 取得中・非対応・失敗では描かず数値も出さない（「平ら」や折れ目なしへの置き換えは
//! 嘘の成功になる）。失敗時に最後に読めた値を残さない。

use crate::token::ColorValue;
use crate::ui::CanvasWidget;
use crate::{Measurement, Node, PaintCommand, Screen, WidgetId};

/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 枠の幅（論理ピクセル相当）。
pub const FRAME_W: f32 = 240.0;
/// 枠の高さ（論理ピクセル相当）。
pub const FRAME_H: f32 = 200.0;
/// 折れ目の帯の最小描画幅。太さ 0 の折れ目でも分割が見えるよう、中心をそろえて広げる。
pub const MIN_GAP: f32 = 4.0;

const CANVAS_ID: WidgetId = WidgetId::from_raw(1);

/// 端末の姿勢（`fandhe_device::foldable_posture::Posture` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// 平ら。
    Flat,
    /// 折れている。
    Folded,
}

/// 折れ目の向き（`FoldOrientation` の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldOrientation {
    /// 折れ目の線が縦（左右に分かれる）。
    Vertical,
    /// 折れ目の線が横（上下に分かれる）。
    Horizontal,
}

/// 折れ目の帯（`Fold` の写し）。位置と太さは軸方向の全長 `extent` に対する値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fold {
    /// 向き。
    pub orientation: FoldOrientation,
    /// 軸方向の始点から帯の始まりまで。
    pub start: f32,
    /// 帯の太さ。
    pub thickness: f32,
    /// 軸方向の全長。
    pub extent: f32,
}

/// 折れ目の情報（`FoldInfo` の写し）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FoldInfo {
    /// 折れ目が分かる。
    Known(Fold),
    /// 取れない（「折れ目なし」ではない）。
    Unknown,
}

/// 読めた姿勢と折れ目（`PostureReading` の写し）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PostureReading {
    /// 姿勢。
    pub posture: Posture,
    /// 折れ目。
    pub fold: FoldInfo,
}

/// 読めなかった理由。`DeviceError` からの写像は呼び出し元（proxy / web-dom）の責務。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// この環境では読めない。
    Unsupported,
    /// 内部エラー（固定文言のみ。OS 固有値を持たない）。
    Failed(String),
}

/// 画面に出す状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FoldablePostureView {
    /// 最初の値が届く前。
    #[default]
    Reading,
    /// 読めた。
    Read(PostureReading),
    /// 非対応。
    Unsupported,
    /// 失敗。
    Failed(String),
}

/// 「折りたたみの姿勢」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FoldablePostureShowState {
    /// 現在の表示状態。
    pub view: FoldablePostureView,
}

/// 読み取り結果を反映する。毎回 view を置き換えるため、失敗後に前の値は残らない。
pub fn apply_read(
    state: &mut FoldablePostureShowState,
    result: Result<PostureReading, ReadFailure>,
) {
    state.view = match result {
        Ok(r) => FoldablePostureView::Read(r),
        Err(ReadFailure::Unsupported) => FoldablePostureView::Unsupported,
        Err(ReadFailure::Failed(m)) => FoldablePostureView::Failed(m),
    };
}

/// 未受信（購読前・購読解除後）に戻す。前回値を残さない。
pub fn apply_pending(state: &mut FoldablePostureShowState) {
    state.view = FoldablePostureView::Reading;
}

const BAND: ColorValue = ColorValue::rgb(0x9A, 0x9A, 0x9A);
const PANEL: ColorValue = ColorValue::rgb(0x2F, 0x80, 0xED);

/// 折れ目の帯を枠の軸方向 `frame` の座標へ縮めた `(帯の始まり, 帯の終わり)`。
/// 描画のためだけのクランプで、値域の検査ではない（検査は `fandhe-device` 側）。
fn gap_range(f: &Fold, frame: f32) -> (f32, f32) {
    let scale = if f.extent > 0.0 {
        frame / f.extent
    } else {
        0.0
    };
    let thick = (f.thickness * scale).max(0.0);
    let center = f.start * scale + thick / 2.0;
    let width = thick.max(MIN_GAP);
    let begin = (center - width / 2.0).clamp(0.0, frame);
    let end = (center + width / 2.0).clamp(begin, frame);
    (begin, end)
}

/// 枠とパネルの描画コマンド。読めたときだけ描き、それ以外は空。
/// 折れていて折れ目が分かるときはパネル 2 枚、それ以外の `Read` は全面 1 枚。
/// `CanvasWidget::bound` の `paint` 関数として渡す。
pub fn panel_commands(state: &FoldablePostureShowState) -> Vec<PaintCommand> {
    let FoldablePostureView::Read(r) = &state.view else {
        return Vec::new();
    };
    let mut cmds = vec![PaintCommand::StyledRect { fill: BAND }];
    match (r.posture, r.fold) {
        (Posture::Folded, FoldInfo::Known(f)) => match f.orientation {
            FoldOrientation::Vertical => {
                let (b, e) = gap_range(&f, FRAME_W);
                cmds.push(PaintCommand::InsetRect {
                    left: 0.0,
                    top: 0.0,
                    right: FRAME_W - b,
                    bottom: 0.0,
                    fill: PANEL,
                });
                cmds.push(PaintCommand::InsetRect {
                    left: e,
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    fill: PANEL,
                });
            }
            FoldOrientation::Horizontal => {
                let (b, e) = gap_range(&f, FRAME_H);
                cmds.push(PaintCommand::InsetRect {
                    left: 0.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: FRAME_H - b,
                    fill: PANEL,
                });
                cmds.push(PaintCommand::InsetRect {
                    left: 0.0,
                    top: e,
                    right: 0.0,
                    bottom: 0.0,
                    fill: PANEL,
                });
            }
        },
        _ => cmds.push(PaintCommand::InsetRect {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            fill: PANEL,
        }),
    }
    cmds
}

fn fold_text(f: &FoldInfo) -> String {
    match f {
        FoldInfo::Unknown => "折れ目: 不明".to_string(),
        FoldInfo::Known(f) => match f.orientation {
            FoldOrientation::Vertical => format!(
                "折れ目: 縦（左端から {} px・幅 {} px）",
                f.start.round(),
                f.thickness.round()
            ),
            FoldOrientation::Horizontal => format!(
                "折れ目: 横（上端から {} px・幅 {} px）",
                f.start.round(),
                f.thickness.round()
            ),
        },
    }
}

/// 状態の文言。`render_headless` は Canvas を出力しないため、ヘッドレス検証と a11y の
/// 両方を本関数が担う。失敗・非対応・読み取り中は数字を出さない。
pub fn status_text(state: &FoldablePostureShowState) -> String {
    match &state.view {
        FoldablePostureView::Reading => "折りたたみの姿勢を読み取っています".to_string(),
        FoldablePostureView::Unsupported => {
            "この環境では折りたたみの姿勢を取得できません".to_string()
        }
        FoldablePostureView::Failed(_) => "折りたたみの姿勢: 不明".to_string(),
        FoldablePostureView::Read(r) => {
            let posture = match r.posture {
                Posture::Flat => "平ら",
                Posture::Folded => "折れている",
            };
            format!("姿勢: {posture} / {}", fold_text(&r.fold))
        }
    }
}

/// 失敗時のみ非空。
pub fn error_text(state: &FoldablePostureShowState) -> String {
    match &state.view {
        FoldablePostureView::Failed(m) => format!("{FAILED_PREFIX}{m}"),
        _ => String::new(),
    }
}

/// 失敗時に固定で使うラベル（呼び出し元が `ReadFailure::Failed` に入れる）。
pub const FAILED_LABEL: &str = "折りたたみの姿勢の取得";

/// 注記（ネイティブ用）。
pub const NOTE: &str = "枠を端末の画面に見立て、折れていて折れ目が分かるときは折れ目に沿って 2 枚に分けて描きます。閉じた状態とヒンジの角度は扱いません。折りたためない端末や、窓が折れ目をまたがないときは「平ら」・折れ目「不明」と表示されます。1 秒ごとに読み直します。";

/// Web 用の注記。
pub const WEB_NOTE: &str = "Chromium 系ブラウザの Device Posture API と Viewport Segments API を読んでいます。折りたためない端末でも「平ら」と表示されます。";

fn base_screen() -> Screen<FoldablePostureShowState> {
    Screen::new("折りたたみの姿勢")
        .with_node(Node::Widget(Box::new(CanvasWidget::bound(
            CANVAS_ID,
            "折りたたみの姿勢と折れ目",
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

/// 「折りたたみの姿勢」画面（ネイティブ用）。
pub fn foldable_posture_show_screen() -> Screen<FoldablePostureShowState> {
    base_screen()
}

/// Web 用の画面（[`WEB_NOTE`] を足した版）。`fandhe-web-dom` の mount が使う。
pub fn foldable_posture_show_web_screen() -> Screen<FoldablePostureShowState> {
    base_screen().text(WEB_NOTE)
}

/// `effect` 区画が要求する `fn(&S) -> Screen<S>` 形の入口。
pub fn current_screen(_state: &FoldablePostureShowState) -> Screen<FoldablePostureShowState> {
    foldable_posture_show_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    #[test]
    fn apply_pending_returns_to_reading() {
        let mut s = state(FoldablePostureView::Read(PostureReading {
            posture: Posture::Folded,
            fold: FoldInfo::Unknown,
        }));
        apply_pending(&mut s);
        assert_eq!(s.view, FoldablePostureView::Reading);
    }

    fn state(view: FoldablePostureView) -> FoldablePostureShowState {
        FoldablePostureShowState { view }
    }

    fn folded(o: FoldOrientation, start: f32, thickness: f32, extent: f32) -> PostureReading {
        PostureReading {
            posture: Posture::Folded,
            fold: FoldInfo::Known(Fold {
                orientation: o,
                start,
                thickness,
                extent,
            }),
        }
    }

    fn insets(c: &PaintCommand) -> (f32, f32, f32, f32) {
        match c {
            PaintCommand::InsetRect {
                left,
                top,
                right,
                bottom,
                ..
            } => (*left, *top, *right, *bottom),
            other => panic!("InsetRect を期待したが {other:?} だった"),
        }
    }

    #[test]
    fn non_read_states_draw_nothing_and_show_no_numbers() {
        for view in [
            FoldablePostureView::Reading,
            FoldablePostureView::Unsupported,
            FoldablePostureView::Failed(FAILED_LABEL.to_string()),
        ] {
            let s = state(view);
            assert!(panel_commands(&s).is_empty(), "{:?}", s.view);
            assert!(!status_text(&s).chars().any(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn failure_drops_previous_read() {
        for r in [
            Err(ReadFailure::Unsupported),
            Err(ReadFailure::Failed(FAILED_LABEL.to_string())),
        ] {
            let mut s = FoldablePostureShowState::default();
            apply_read(
                &mut s,
                Ok(folded(FoldOrientation::Vertical, 400.0, 20.0, 800.0)),
            );
            apply_read(&mut s, r);
            assert!(!matches!(s.view, FoldablePostureView::Read(_)));
            assert!(panel_commands(&s).is_empty());
        }
    }

    #[test]
    fn folded_vertical_draws_two_panels_to_scale() {
        let s = state(FoldablePostureView::Read(folded(
            FoldOrientation::Vertical,
            400.0,
            80.0,
            800.0,
        )));
        let cmds = panel_commands(&s);
        assert_eq!(cmds.len(), 3);
        // scale = 240/800 = 0.3。帯は 120..144。
        assert_eq!(insets(&cmds[1]), (0.0, 0.0, FRAME_W - 120.0, 0.0));
        assert_eq!(insets(&cmds[2]), (144.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn folded_horizontal_draws_two_panels_to_scale() {
        let s = state(FoldablePostureView::Read(folded(
            FoldOrientation::Horizontal,
            300.0,
            100.0,
            600.0,
        )));
        let cmds = panel_commands(&s);
        assert_eq!(cmds.len(), 3);
        // scale = 200/600。帯は 100..133.33。
        let a = insets(&cmds[1]);
        let b = insets(&cmds[2]);
        assert!((a.3 - (FRAME_H - 100.0)).abs() < 0.01);
        assert!((b.1 - 133.333).abs() < 0.01);
    }

    #[test]
    fn zero_thickness_still_leaves_min_gap() {
        let s = state(FoldablePostureView::Read(folded(
            FoldOrientation::Vertical,
            400.0,
            0.0,
            800.0,
        )));
        let cmds = panel_commands(&s);
        let left_right = insets(&cmds[1]).2;
        let right_left = insets(&cmds[2]).0;
        assert!(right_left - (FRAME_W - left_right) >= MIN_GAP - 0.001);
    }

    #[test]
    fn flat_or_unknown_fold_draws_single_panel_without_no_fold_claim() {
        for r in [
            PostureReading {
                posture: Posture::Flat,
                fold: FoldInfo::Unknown,
            },
            PostureReading {
                posture: Posture::Folded,
                fold: FoldInfo::Unknown,
            },
        ] {
            let s = state(FoldablePostureView::Read(r));
            assert_eq!(panel_commands(&s).len(), 2);
            let t = status_text(&s);
            assert!(t.contains("折れ目: 不明"), "{t}");
            assert!(!t.contains("なし"), "{t}");
        }
    }

    #[test]
    fn status_text_describes_fold() {
        let s = state(FoldablePostureView::Read(folded(
            FoldOrientation::Vertical,
            400.0,
            20.0,
            800.0,
        )));
        assert_eq!(
            status_text(&s),
            "姿勢: 折れている / 折れ目: 縦（左端から 400 px・幅 20 px）"
        );
    }

    #[test]
    fn huge_values_are_clamped_to_frame() {
        for o in [FoldOrientation::Vertical, FoldOrientation::Horizontal] {
            let s = state(FoldablePostureView::Read(folded(o, 1.0e9, 1.0e9, 100.0)));
            for c in &panel_commands(&s)[1..] {
                let (l, t, r, b) = insets(c);
                for v in [l, t, r, b] {
                    assert!(v >= 0.0 && v <= FRAME_W.max(FRAME_H), "{v}");
                }
            }
        }
    }

    #[test]
    fn error_text_only_when_failed() {
        let mut s = FoldablePostureShowState::default();
        assert_eq!(error_text(&s), "");
        apply_read(&mut s, Err(ReadFailure::Failed(FAILED_LABEL.to_string())));
        assert_eq!(error_text(&s), "失敗しました: 折りたたみの姿勢の取得");
        apply_read(&mut s, Err(ReadFailure::Unsupported));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn render_headless_does_not_panic() {
        let s = FoldablePostureShowState::default();
        let joined = render_headless(&foldable_posture_show_screen(), &s).join("\n");
        assert!(joined.contains("読み取っています"));
        assert!(!joined.contains(WEB_NOTE));
        let web = render_headless(&foldable_posture_show_web_screen(), &s).join("\n");
        assert!(web.contains(WEB_NOTE));
    }
}
