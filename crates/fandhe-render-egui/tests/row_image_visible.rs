//! 見えている行の画像が、フレーム末尾の effect の `put` で追い出されてちらつかないことを検証する（Issue #1291）。
//!
//! 見えている 4 行（各 約 16 MiB。保管所の予算 64 MiB ちょうど）を描いたあと、effect が別の画像を `put` する。
//! 描画側が `begin_frame` でフレームの区切りを知らせていれば、そのフレームで描かれた行は追い出されず、
//! 今置いた画像のほうが捨てられる。旧版（純粋な LRU）は、見えている最古の行を追い出して alt に戻していた。
//! store は process-global のため、1 ファイル 1 テストで確かめる。

use fandhe_core::{ImageWidget, Node, RgbaPixels, Screen, StateStore, WidgetId};
use fandhe_runtime::image_store::PutResult;
use std::collections::BTreeSet;

struct St;

const ROWS: u64 = 4;

fn rows(_: &St) -> Vec<Node<St>> {
    (0..ROWS)
        .map(|i| Node::Row {
            children: vec![Node::Widget(Box::new(ImageWidget::fixed(
                WidgetId::from_raw(10 + i),
                format!("rowvis:{i}"),
                format!("代替{i}"),
            )))],
            tap: None,
        })
        .collect()
}

fn big(c: u8) -> RgbaPixels {
    RgbaPixels::new(2048, 2048, vec![c; 2048 * 2048 * 4]).unwrap()
}

fn collect(shape: &egui::Shape, tex: &mut BTreeSet<u64>, text: &mut String) {
    match shape {
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, tex, text)),
        egui::Shape::Rect(r) => {
            if let Some(egui::TextureId::Managed(n)) = r.brush.as_ref().map(|b| b.fill_texture_id) {
                tex.insert(n);
            }
        }
        egui::Shape::Text(t) => text.push_str(t.galley.text()),
        _ => {}
    }
}

/// 1 フレーム描いて、(画像テクスチャの数, 文字列) を返す。フレームの冒頭で `begin_frame` を呼ぶ（`ProxyApp::update` と同じ）。
fn frame(ctx: &egui::Context, screen: &Screen<St>) -> (usize, String) {
    fandhe_runtime::image_store::begin_frame();
    let mut store = StateStore::new(St);
    let full = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 1200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                fandhe_render_egui::render_screen(ui, screen, &mut store);
            });
        },
    );
    let (mut tex, mut text) = (BTreeSet::new(), String::new());
    for c in &full.shapes {
        collect(&c.shape, &mut tex, &mut text);
    }
    (tex.len(), text)
}

#[test]
fn visible_rows_do_not_flicker_when_an_effect_puts_after_the_frame() {
    let ctx = egui::Context::default();
    let screen = Screen::new("t").list(rows);
    for i in 0..ROWS {
        assert_eq!(
            fandhe_runtime::image_store::put(format!("rowvis:{i}"), big(i as u8 + 1)),
            PutResult::Stored
        );
    }

    for round in 0..3 {
        let (n, text) = frame(&ctx, &screen);
        assert_eq!(
            n as u64, ROWS,
            "round {round}: 見えている行はすべて画像: {text}"
        );
        assert!(
            !text.contains("代替"),
            "round {round}: alt に戻らない: {text}"
        );
        // フレーム末尾の effect が、まだ無いキーを置く（見えている行に足すと予算を超える）。
        assert_eq!(
            fandhe_runtime::image_store::put("rowvis:extra".into(), big(99)),
            PutResult::RejectedOverBudget,
            "置けなかったと戻り値で分かる"
        );
        assert!(
            !fandhe_runtime::image_store::contains("rowvis:extra"),
            "今置いた画像のほうが捨てられる"
        );
        for i in 0..ROWS {
            assert!(fandhe_runtime::image_store::contains(&format!(
                "rowvis:{i}"
            )));
        }
    }

    // `ProxyApp` の配線: `update` の冒頭が `begin_frame` を呼ぶので、フレーム末尾の effect の `put` でも見えている行は残る。
    static HIDE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn screen_fn(_: &St) -> Screen<St> {
        if HIDE.load(std::sync::atomic::Ordering::SeqCst) {
            Screen::new("t")
        } else {
            Screen::new("t").list(rows)
        }
    }
    // `image_store` の doc の書き方: 置けなかった（`RejectedOverBudget`）識別子は failed に入れ、読み直さない。
    static DECODES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn effect(_: &St, _: &fandhe_render_egui::CompletionSender<St>) {
        use std::sync::atomic::Ordering::SeqCst;
        let key = "rowvis:extra";
        if fandhe_runtime::image_store::contains(key) || FAILED.load(SeqCst) {
            return;
        }
        DECODES.fetch_add(1, SeqCst);
        if fandhe_runtime::image_store::put(key.into(), big(99)) == PutResult::RejectedOverBudget {
            FAILED.store(true, SeqCst);
        }
    }
    let app = fandhe_render_egui::ProxyApp::with_screen_fn(&ctx, screen_fn, St).with_effect(effect);
    let mut h = egui_kittest::Harness::new_state(
        |ctx, app: &mut fandhe_render_egui::ProxyApp<St>| {
            fandhe_render_egui::EguiApp::update(app, ctx)
        },
        app,
    );
    for _ in 0..4 {
        h.step();
        for i in 0..ROWS {
            assert!(
                fandhe_runtime::image_store::contains(&format!("rowvis:{i}")),
                "ProxyApp 経由でも見えている行は残る"
            );
        }
        assert!(!fandhe_runtime::image_store::contains("rowvis:extra"));
    }

    // 書き方どおりなら、置けなかった識別子の復号は繰り返されない（復号と再描画が止まらなくならない）。
    assert_eq!(
        DECODES.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "置けなかった識別子を毎フレーム復号し直さない"
    );

    // 行が見えなくなったら（描かれなくなったら）保護も外れ、次のフレームの `put` は古い行から追い出せる。
    // `ProxyApp` がフレームごとに `begin_frame` を呼ばないと、最後に描いた印が残り続けて、ここが成り立たない。
    HIDE.store(true, std::sync::atomic::Ordering::SeqCst);
    // 行が見えなくなると、前のフレームだけで描かれた行は追い出し候補（2 段目）になり、`put` が置けるようになる。
    for _ in 0..2 {
        h.step();
    }
    // 条件が変わったので、やり直す（failed から外す）。
    FAILED.store(false, std::sync::atomic::Ordering::SeqCst);
    h.step();
    assert!(
        fandhe_runtime::image_store::contains("rowvis:extra"),
        "描かれなくなった行は追い出せる（保護が次のフレームへ持ち越されない）"
    );
    fandhe_runtime::image_store::remove_prefix("rowvis:");
}
