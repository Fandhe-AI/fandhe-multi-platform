//! eframe 無しで `ProxyApp` が [`EguiApp::update`] 経由で描画でき、操作が状態に届くことの回帰テスト
//! （Issue #1164）。起動口（`launch`）が eframe の `App` を包む前の、描画と状態更新だけを確かめる。
//!
//! GPU を使わない（kittest の既定レンダラ）ため、アダプタの無い CI でもスキップされない。

use egui_kittest::kittest::Queryable;
use fandhe_render_egui::{EguiApp, ProxyApp};

#[test]
fn click_reaches_state_through_egui_app_update() {
    let app = ProxyApp::new(&egui::Context::default());
    let mut harness =
        egui_kittest::Harness::new_state(|ctx, app: &mut ProxyApp| EguiApp::update(app, ctx), app);
    harness.run();

    harness.get_by_label("項目を追加").click();
    harness.run();

    // 追加した項目が一覧に描画される = クリックが dispatch され、状態が更新されて再描画された。
    harness.get_by_label("item-1");
}
