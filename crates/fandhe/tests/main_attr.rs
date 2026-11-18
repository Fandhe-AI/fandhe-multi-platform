//! `#[fandhe::main]`（Issue #1200）が、付けた関数から各 OS の入口を生成することの確認。
//!
//! テストの実行体はハーネスが `main` を持つため、生成先をモジュールの中に置く。起動（ウィンドウを開く）はしない。
#![cfg(feature = "egui")]

mod entry {
    #[fandhe::main]
    fn app(ctx: &fandhe::egui::Context) -> Box<dyn fandhe::app::ProxyAppDyn> {
        Box::new(fandhe::app::Menu::new(ctx, "テスト"))
    }

    /// 生成された入口が呼ぶ関数と同じものを、起動せずに呼べるようにする。
    pub(super) fn build(ctx: &fandhe::egui::Context) -> Box<dyn fandhe::app::ProxyAppDyn> {
        app(ctx)
    }
}

#[test]
fn generates_main_with_the_entry_signature() {
    let _: fn() -> Result<(), fandhe::app::LaunchError> = entry::main;
}

#[test]
fn annotated_function_matches_the_entry_build_type() {
    let _: fandhe::__private::BuildFn = entry::build;
    let ctx = fandhe::egui::Context::default();
    let _app = entry::build(&ctx);
}
