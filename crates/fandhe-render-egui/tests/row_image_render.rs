//! 一覧の行ごとに別の画像が出ること、容量を超えた画像は捨てられて alt に戻ることを検証する（Issue #1291）。
//!
//! 画素は `fandhe_runtime::image_store::put` で行ごとの識別子に置く。出た画像は、描画された
//! テクスチャ塗りの `Shape::Rect`（`egui::Image`）（行ごとに別の `TextureId::Managed`）で数える（文字のフォントは id 0）。
//! store は process-global のため、1 ファイル 1 テストで直列に確かめる。

use fandhe_core::{ImageWidget, Node, RgbaPixels, Screen, StateStore, WidgetId};
use std::collections::BTreeSet;

struct St;

fn rows(_: &St) -> Vec<Node<St>> {
    (0..3)
        .map(|i| Node::Row {
            children: vec![Node::Widget(Box::new(ImageWidget::fixed(
                WidgetId::from_raw(10 + i),
                format!("rowimg-test:{i}"),
                format!("代替{i}"),
            )))],
            tap: None,
        })
        .collect()
}

fn solid(side: u32, c: u8) -> RgbaPixels {
    let rgba: Vec<u8> = (0..side * side).flat_map(|_| [c, 0, 0, 255]).collect();
    RgbaPixels::new(side, side, rgba).unwrap()
}

fn collect(shape: &egui::Shape, tex: &mut BTreeSet<u64>, text: &mut String) {
    match shape {
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, tex, text)),
        // `egui::Image` は `RectShape` のテクスチャ塗り（brush）として描かれる。
        egui::Shape::Rect(r) => {
            if let Some(egui::TextureId::Managed(n)) = r.brush.as_ref().map(|b| b.fill_texture_id) {
                tex.insert(n);
            }
        }
        egui::Shape::Mesh(m) => {
            if let egui::TextureId::Managed(n) = m.texture_id {
                if n != 0 {
                    tex.insert(n);
                }
            }
        }
        egui::Shape::Text(t) => {
            text.push_str(t.galley.text());
            text.push('|');
        }
        _ => {}
    }
}

/// 数フレーム描画して、出た画像テクスチャの数と文字列を返す。
fn paint(ctx: &egui::Context, screen: &Screen<St>) -> (usize, String) {
    let mut store = StateStore::new(St);
    let (mut tex, mut text) = (BTreeSet::new(), String::new());
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
        (tex, text) = (BTreeSet::new(), String::new());
        for c in &full.shapes {
            collect(&c.shape, &mut tex, &mut text);
        }
    }
    (tex.len(), text)
}

#[test]
fn rows_show_their_own_images_and_evicted_ones_fall_back_to_alt() {
    let ctx = egui::Context::default();
    let screen = Screen::new("t").list(rows);

    // 置く前: 画像は 0 枚で、どの行も alt が文字で出る。
    let (n, text) = paint(&ctx, &screen);
    assert_eq!(n, 0);
    for i in 0..3 {
        assert!(text.contains(&format!("代替{i}")), "{text}");
    }

    // 行ごとに別の画像を置くと、行ごとに別のテクスチャが出る（旧版は最後の 1 枚だけ）。
    for i in 0..3 {
        fandhe_runtime::image_store::put(format!("rowimg-test:{i}"), solid(8, 40 * (i as u8 + 1)));
    }
    let (n, text) = paint(&ctx, &screen);
    assert_eq!(n, 3, "3 行に 3 枚: {text}");
    assert!(
        !text.contains("代替"),
        "画像が出た行は alt を出さない: {text}"
    );

    // 1 枚だけ消すと、その行だけ alt に戻る（他の行は残る）。
    assert!(fandhe_runtime::image_store::remove("rowimg-test:1"));
    let (n, text) = paint(&ctx, &screen);
    assert_eq!(n, 2);
    assert!(
        text.contains("代替1") && !text.contains("代替0") && !text.contains("代替2"),
        "{text}"
    );

    // 容量を超える大きな画像を置くと、最後に使われたのが古い行から捨てられ、その行は alt に戻る。
    let big = |c| solid(2048, c); // 約 16 MiB。上限 64 MiB を超えるまで置く
    for k in 0..5 {
        fandhe_runtime::image_store::put(format!("rowimg-big:{k}"), big(k as u8));
    }
    let (_, text) = paint(&ctx, &screen);
    assert!(
        text.contains("代替0"),
        "古い行は追い出されて alt に戻る: {text}"
    );
    fandhe_runtime::image_store::remove_prefix("rowimg-");
}
