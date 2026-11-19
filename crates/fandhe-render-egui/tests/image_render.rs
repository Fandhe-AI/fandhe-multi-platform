//! `.image()` / `.image_bound()` の alt 表示と AX を検証する（Issue #1427）。
//!
//! 画素は `fandhe_runtime::image_store::put` で同じ識別子を置いたときだけ出る。置かれていないとき
//! （ここでは保管所を触らず、識別子を使わない）は alt を文字で描き、ソースが空なら何も描かない。
//! 代わりの文字は AX ノードを作らずに描くため、読み上げは合成の Image ノード 1 つだけになる
//! （`ui.label` だと Label・TextRun が増えて二重に読まれる）。文字そのものは AX に出ないので、
//! 描画された shape の文字列で確かめる。画素が出る経路は `widget_coverage` の `ImageWidget(store 一致)`。

use fandhe_core::{Screen, StateStore, WidgetId};

struct St;

fn src(_: &St) -> String {
    "no-such-image-1427".into()
}

fn id(n: u64) -> WidgetId {
    WidgetId::from_raw(n)
}

fn shape_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Vec(v) => v.iter().for_each(|s| shape_text(s, out)),
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('|');
        }
        _ => {}
    }
}

/// 数フレーム描画して、出た文字列（`|` 区切り）を返す。
fn painted_text(screen: &Screen<St>) -> String {
    let ctx = egui::Context::default();
    let mut store = StateStore::new(St);
    let mut text = String::new();
    for _ in 0..3 {
        let full = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    fandhe_render_egui::render_screen(ui, screen, &mut store);
                });
            },
        );
        text.clear();
        for c in &full.shapes {
            shape_text(&c.shape, &mut text);
        }
    }
    text
}

/// 画素が置かれていない識別子の `.image()` / `.image_bound()` は、alt を文字で出す。
#[test]
fn unmatched_source_shows_alt_text() {
    let text = painted_text(
        &Screen::new("t")
            .image(id(1), "unplaced-image", "固定の代替")
            .image_bound(id(2), src, "束縛の代替"),
    );
    for t in ["固定の代替", "束縛の代替"] {
        assert!(text.contains(t), "{t}: {text}");
    }
}

/// ソースが空（写真選択の選ぶ前など）のときは、従来どおり何も描かない。
#[test]
fn empty_source_draws_nothing() {
    let text = painted_text(&Screen::new("t").image(id(1), "", "出ない代替"));
    assert!(!text.contains("出ない代替"), "{text}");
}

#[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]
mod ax {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn harness(screen: Screen<St>) -> egui_kittest::Harness<'static, StateStore<St>> {
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(400.0, 300.0))
            .build_ui_state(
                move |ui, store: &mut StateStore<St>| {
                    fandhe_render_egui::render_screen(ui, &screen, store);
                },
                StateStore::new(St),
            );
        h.run();
        h
    }

    /// label か value が `t` と一致する AX ノードの総数。
    fn named(h: &egui_kittest::Harness<'_, StateStore<St>>, t: &str) -> usize {
        h.query_all_by_label(t).count() + h.query_all_by_value(t).count()
    }

    /// alt は AX で 1 ノード（合成の Image）だけに出る。代わりの文字が Label・TextRun を増やすと落ちる。
    #[test]
    fn alt_is_exposed_as_one_ax_node() {
        let h = harness(Screen::new("t").image(id(1), "unplaced-image", "AX の代替"));
        assert_eq!(named(&h, "AX の代替"), 1);
    }

    /// ソースが空のときは、alt の名前も AX に出ない（`.image_bound()` の sync 前の空も含む）。
    #[test]
    fn empty_source_has_no_ax_name() {
        let h = harness(Screen::new("t").image(id(1), "", "名前なし"));
        assert_eq!(named(&h, "名前なし"), 0);
        let h = harness(Screen::new("t").image_bound(id(1), |_| String::new(), "束縛の名前なし"));
        assert_eq!(named(&h, "束縛の名前なし"), 0);
    }
}
