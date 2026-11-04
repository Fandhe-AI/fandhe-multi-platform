//! `Renderer` trait 経由の egui 描画の結合テスト（Issue #1163）。
//!
//! `EguiRenderer` が `fandhe_runtime::Renderer` として画面を描き、クリックが
//! `store.dispatch` へ届くことを検証する。GPU を使わない既定の `Harness` で動く。

use egui_kittest::kittest::Queryable;
use fandhe_core::{Screen, StateStore, WidgetId};
use fandhe_egui_proxy::{EguiRenderer, VirtualListRecyclers};
use fandhe_runtime::Renderer;

#[derive(Default)]
struct S {
    clicks: u32,
}

/// 具体型を知らない呼び出し側と同じく、trait 境界越しに描く。
fn draw<R: Renderer<S>>(r: &mut R, screen: &Screen<S>, store: &mut StateStore<S>) {
    r.render(screen, store);
}

#[test]
fn egui_renderer_draws_and_dispatches_through_trait() {
    let screen =
        Screen::new("題").button(WidgetId::from_raw(1), "押す", |s: &mut S| s.clicks += 1);
    let mut harness = egui_kittest::Harness::builder().build_ui_state(
        move |ui, (store, recyclers): &mut (StateStore<S>, VirtualListRecyclers<S>)| {
            draw(&mut EguiRenderer::new(ui, recyclers), &screen, store);
        },
        (StateStore::new(S::default()), VirtualListRecyclers::new()),
    );
    harness.run();
    harness.get_by_label("押す").click();
    harness.run();
    assert_eq!(harness.state().0.state().clicks, 1);
}
