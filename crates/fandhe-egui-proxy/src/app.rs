//! プロトタイプ・TV フォーカスの検証用アプリ（[`PrototypeApp`]・[`TvFocusApp`]）。
//!
//! 汎用の [`ProxyApp`] と起床口・完了通知は `fandhe-render-egui` が持つ（Issue #1164）。
//! 本モジュールは、TV フォーカス（`tv_focus.rs`）など本クレートに残る部品を使うアプリだけを置き、
//! 移した項目を従来のパス（`crate::app::CompletionSender` 等）で再公開する。

use fandhe_core::{Screen, StateStore};
use fandhe_render_egui::app::central_panel_with_insets;
#[cfg(test)]
#[cfg_attr(not(feature = "device"), allow(unused_imports))]
pub(crate) use fandhe_render_egui::app::egui_sender;
#[cfg_attr(
    not(all(feature = "device", not(target_arch = "wasm32"))),
    allow(unused_imports)
)]
pub(crate) use fandhe_render_egui::app::EguiSenderExt;
pub use fandhe_render_egui::app::{
    insets_px_to_margin, Completion, CompletionSender, EffectFn, EguiWaker, InsetMarginSupplier,
    ProxyApp, ProxyAppDyn,
};
use fandhe_render_egui::{EguiApp, VirtualListRecyclers};
use fandhe_runtime::Renderer as _;

use crate::tv_focus::TvFocusController;
use crate::tv_sample::{tv_sample_screen, TvState};

// TASK-2.3（#54）計測マーカー: 以下はプロトタイプ描画対象切替（TASK-2.2-S5 / #52）の
// 専用型。共通/プラットフォーム別コード比率計測（分母）としてマーカー登録する
// （docs/architecture/code-ratio-classification.md・scripts/metrics/code-classification.env 参照）。
// fandhe-prototype:begin
/// プロトタイプアプリ（`fandhe-prototype`、TASK-2.2）の実描画対象。
///
/// `sample_screen()` を固定で保持する [`ProxyApp`] と異なり、`AppState::route` の
/// 変化に追従して表示画面を毎フレーム引き直す必要があるため、`Screen<AppState>` を
/// キャッシュせず `update` のたびに `fandhe_prototype::current_screen` へ問い合わせる
/// （設計書 4 章のシーケンス「dispatch → version 進行 → current_screen 引き直し →
/// 再描画」の実配線、S5 #52 のスコープ）。
pub struct PrototypeApp {
    store: fandhe_core::StateStore<fandhe_prototype::AppState>,
    /// `Node::VirtualList` の項目ノードをフレームをまたいで再利用するプール（#490）。
    /// `current_screen` が毎フレーム画面を引き直す設計（本構造体の doc 参照）でも、
    /// 「同一 version → 同一画面」の純関数契約さえ保たれれば `path` ベースの
    /// プール識別（`recyclers` モジュール doc 参照）はそのまま機能する
    /// （route 遷移は dispatch 経由でのみ起き、その際は version が進むため
    /// `ListRecycler::acquire` が全キャッシュを無効化し、stale ノードは混入しない）。
    recyclers: VirtualListRecyclers<fandhe_prototype::AppState>,
}

impl PrototypeApp {
    /// 既定の初期状態（一覧画面・空の項目一覧）でアプリを構築する。
    pub fn new(ctx: &egui::Context) -> Self {
        crate::font::install_cjk_font(ctx);
        Self {
            store: fandhe_core::StateStore::new(fandhe_prototype::AppState::default()),
            recyclers: VirtualListRecyclers::new(),
        }
    }
}

impl EguiApp for PrototypeApp {
    /// 現在の `AppState` から表示画面を引き直し、`render_screen_with_recyclers` へ
    /// 委譲する（フレーム跨ぎ再利用プールを保持する版、#490）。
    ///
    /// `render_screen_with_recyclers` は描画中に発火したイベントを走査完了後に
    /// まとめて `store.dispatch` するため、遷移ハンドラ（`nav::go_to_*` 等）による
    /// `route` の書き換えは次フレームの `current_screen` 呼び出しに反映される
    /// （即時の画面差し替えではなく、既存の version 駆動再描画の枠組みに従う）。
    fn update(&mut self, ctx: &egui::Context) {
        let screen = fandhe_prototype::current_screen(self.store.state());
        egui::CentralPanel::default().show(ctx, |ui| {
            fandhe_render_egui::EguiRenderer::new(ui, &mut self.recyclers)
                .render(&screen, &mut self.store);
        });
    }
}

// fandhe-prototype:end

/// TV フォーカスナビゲーション検証用アプリケーション本体（TASK-17.2 / REQ-17）。
///
/// `ProxyApp`/`PrototypeApp` と同じく `StateStore` 経由で状態を保持する（#536 で
/// [`TvFocusController::update`] の決定操作・クリックが `StateStore::dispatch`
/// （`Event::ButtonActivatedAt`）へ統一されたことに伴い、直接所有をやめた）。
/// `tv_sample_screen()`（`tv_sample.rs`）を固定で保持し、実描画・入力処理は
/// [`TvFocusController::update`] へ委譲する。
pub struct TvFocusApp {
    screen: Screen<TvState>,
    store: StateStore<TvState>,
    controller: TvFocusController<TvState>,
    /// window insets 供給関数（Issue #579、`ProxyApp::inset_margin` と同じ契約）。
    inset_margin: Option<InsetMarginSupplier>,
}

impl TvFocusApp {
    /// 既定の初期状態でアプリを構築する。
    pub fn new(ctx: &egui::Context) -> Self {
        crate::font::install_cjk_font(ctx);
        Self {
            screen: tv_sample_screen(),
            store: StateStore::new(TvState::default()),
            controller: TvFocusController::new(),
            inset_margin: None,
        }
    }

    /// window insets 供給関数を注入する（Issue #579、`ProxyApp::with_inset_margin_supplier`
    /// と同じ契約。`android.rs` からのみ呼ばれる）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn with_inset_margin_supplier(
        mut self,
        supplier: impl Fn(&egui::Context) -> egui::Margin + 'static,
    ) -> Self {
        self.inset_margin = Some(Box::new(supplier));
        self
    }
}

impl EguiApp for TvFocusApp {
    /// 矢印キー/D-pad・決定操作の受付から描画までを [`TvFocusController::update`] へ
    /// 一括委譲する（入力受付には `ui.ctx()` が必要なため `CentralPanel` の内側で
    /// 呼ぶ、`TvFocusController::update` の doc 参照）。
    fn update(&mut self, ctx: &egui::Context) {
        central_panel_with_insets(ctx, &self.inset_margin).show(ctx, |ui| {
            self.controller.update(ui, &self.screen, &mut self.store);
        });
    }
}
