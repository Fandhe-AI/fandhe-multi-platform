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

#[test]
fn unhandled_back_returns_to_list() {
    let ctx = egui::Context::default();
    let menu = Menu::new(&ctx, "例").screen("サンプル", |ctx| Box::new(ProxyApp::new(ctx)));
    let mut harness =
        egui_kittest::Harness::new_state(|ctx, app: &mut Menu| EguiApp::update(app, ctx), menu);
    harness.run();
    harness.get_by_label("サンプル").click();
    harness.run();
    harness.get_by_label("項目を追加");

    // sample 画面は戻る操作を処理しないため、Esc で一覧へ戻る。
    harness.key_press(egui::Key::Escape);
    harness.run();
    harness.get_by_label("サンプル");
    assert!(harness.query_by_label("項目を追加").is_none());
}

#[test]
fn on_exit_reaches_open_child() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct Child(Arc<AtomicBool>);
    impl EguiApp for Child {
        fn update(&mut self, _ctx: &egui::Context) {}
        fn on_exit(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl fandhe_render_egui::ProxyAppDyn for Child {
        fn set_inset_margin_supplier(&mut self, _: fandhe_render_egui::app::InsetMarginSupplier) {}
        fn set_autoshot(&mut self, _: std::path::PathBuf, _: Arc<AtomicBool>) {}
    }

    let exited = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&exited);
    let ctx = egui::Context::default();
    let menu = Menu::new(&ctx, "例").screen("子", move |_| Box::new(Child(Arc::clone(&flag))));
    let mut harness =
        egui_kittest::Harness::new_state(|ctx, app: &mut Menu| EguiApp::update(app, ctx), menu);
    harness.run();
    harness.get_by_label("子").click();
    harness.run();
    EguiApp::on_exit(harness.state_mut());
    assert!(exited.load(Ordering::SeqCst));
}
