//! 複数の画面を一覧から選んで切り替えるトップ画面（[`Menu`]、Issue #1199）。
//!
//! 利用者が `ProxyAppDyn` を自前で実装しなくても、複数画面のアプリを組めるようにする。
//! 別リポジトリの最小アプリ（#1177）が手で書いていたメニューを、入口 crate `fandhe` から使える部品にした。
//! 子画面は選んだときに作り、メニューへ戻ると捨てる（画面の状態は持ち越さない）。子の画面が処理しなかった
//! 戻る操作（Esc・Android の戻る）でも一覧へ戻る。終了処理（`on_exit`）は表示中の子へ渡し、一覧の表示中は
//! 終了処理の登録表（`fandhe_runtime::teardown`）を直接呼ぶ。

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::app::{InsetMarginSupplier, ProxyAppDyn};
use crate::EguiApp;

/// 子画面を作る関数。メニューで選ばれるたびに呼ばれる。
pub type BuildScreenFn = Box<dyn Fn(&egui::Context) -> Box<dyn ProxyAppDyn>>;

/// 子画面の表示中に右上へ重ねる、メニューへ戻るボタンの文言。
pub const BACK_LABEL: &str = "メニュー";

/// 画面の一覧を出し、選んだ画面を子として表示するトップ画面。
///
/// 子の表示中は右上に [`BACK_LABEL`] のボタンを重ね、押すとメニューへ戻る。
/// 入口が注入する insets（Android の window insets）はメニュー自身と子の両方へ配る。
pub struct Menu {
    ctx: egui::Context,
    title: String,
    screens: Vec<(String, BuildScreenFn)>,
    child: Option<Box<dyn ProxyAppDyn>>,
    inset: Option<Rc<InsetMarginSupplier>>,
}

impl Menu {
    /// 見出し `title` の空のメニューを作る。CJK フォントを登録する（[`crate::ProxyApp::new`] と同じ）。
    pub fn new(ctx: &egui::Context, title: impl Into<String>) -> Self {
        crate::font::install_cjk_font(ctx);
        Self {
            ctx: ctx.clone(),
            title: title.into(),
            screens: Vec::new(),
            child: None,
            inset: None,
        }
    }

    /// 一覧の末尾に、表示名 `label` の画面を足す。`build` は選ばれるたびに呼ばれる。
    pub fn screen(
        mut self,
        label: impl Into<String>,
        build: impl Fn(&egui::Context) -> Box<dyn ProxyAppDyn> + 'static,
    ) -> Self {
        self.screens.push((label.into(), Box::new(build)));
        self
    }

    fn inset_margin(&self, ctx: &egui::Context) -> egui::Margin {
        self.inset.as_ref().map_or(egui::Margin::ZERO, |f| f(ctx))
    }

    fn open(&mut self, index: usize) {
        let mut child = (self.screens[index].1)(&self.ctx);
        if let Some(f) = &self.inset {
            let f = Rc::clone(f);
            child.set_inset_margin_supplier(Box::new(move |c| f(c)));
        }
        self.child = Some(child);
    }
}

impl EguiApp for Menu {
    fn update(&mut self, ctx: &egui::Context) {
        if let Some(child) = self.child.as_mut() {
            child.update(ctx);
            // 子の画面が処理しなかった戻る操作（Esc・Android の戻る）でも一覧へ戻る。
            if child.take_unhandled_back() {
                self.child = None;
                return;
            }
            let top = self.inset_margin(ctx).top as f32;
            let mut back = false;
            egui::Area::new(egui::Id::new("fandhe_menu_back"))
                .anchor(egui::Align2::RIGHT_TOP, [-8.0, top + 8.0])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| back = ui.button(BACK_LABEL).clicked());
            if back {
                self.child = None;
            }
            return;
        }
        let base = egui::Frame::central_panel(&ctx.style());
        let frame = base.inner_margin(base.inner_margin + self.inset_margin(ctx));
        let mut picked = None;
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            ui.heading(&self.title);
            ui.add_space(12.0);
            // 画面数に上限が無いため、収まらない分はスクロールで選べるようにする。
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (index, (label, _)) in self.screens.iter().enumerate() {
                    let button = egui::Button::new(label.as_str());
                    if ui.add_sized([ui.available_width(), 48.0], button).clicked() {
                        picked = Some(index);
                    }
                }
            });
        });
        if let Some(index) = picked {
            self.open(index);
        }
    }

    /// アプリの終了処理。子を表示中なら子の `on_exit`（`ProxyApp` は登録表の終了処理を呼ぶ）へ渡し、一覧の表示中は
    /// 登録表の終了処理（[`fandhe_runtime::teardown::run_exit`]）を直接呼ぶ。終了処理はプロセス全体の端末資源を止めるため、
    /// 子を閉じて一覧へ戻るときには呼ばない（戻った後も他の画面で端末機能を使い続けるため）。
    fn on_exit(&mut self) {
        match self.child.as_mut() {
            Some(child) => child.on_exit(),
            None => fandhe_runtime::teardown::run_exit(),
        }
    }
}

impl ProxyAppDyn for Menu {
    fn set_inset_margin_supplier(&mut self, supplier: InsetMarginSupplier) {
        self.inset = Some(Rc::new(supplier));
    }

    /// 自動撮影はメニューでは配線しない（子画面は選ばれるまで存在しないため）。
    fn set_autoshot(&mut self, _output_path: PathBuf, _save_failed: Arc<AtomicBool>) {}
}
