//! [`Menu`] が一覧から選んだ画面を開き、戻るボタンで一覧へ戻ることの回帰テスト（Issue #1199）。

use egui_kittest::kittest::Queryable;
use fandhe_render_egui::{menu::BACK_LABEL, EguiApp, Menu, ProxyApp};

#[test]
fn menu_opens_screen_and_returns() {
    let ctx = egui::Context::default();
    let menu = Menu::new(&ctx, "例").screen("サンプル", |ctx| Box::new(ProxyApp::new(ctx)));
    let mut harness =
        egui_kittest::Harness::new_state(|ctx, app: &mut Menu| EguiApp::update(app, ctx), menu);
    harness.run();

    harness.get_by_label("サンプル").click();
    harness.run();
    // 子画面（sample 画面）が開いた。
    harness.get_by_label("項目を追加");

    harness.get_by_label(BACK_LABEL).click();
    harness.run();
    harness.get_by_label("サンプル");
    assert!(harness.query_by_label("項目を追加").is_none());
}
