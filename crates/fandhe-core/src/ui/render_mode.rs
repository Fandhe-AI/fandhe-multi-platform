//! 画面単位の出力方式選択導線（REQ-4 / TASK-4.4-S3、Issue #81）。
//!
//! `docs/architecture/dom-output-architecture.md` §2.5 が設計した「`Screen<S>` を保持する側
//! （アプリ・プロトタイプ層）が画面登録時に指定する列挙型」を実装したもの。
//! 同書 §4 が「S3 着手時に確定する」とした `RenderMode` の配置先は、依存ゼロの純データ enum
//! であり `fandhe-egui-proxy` / `fandhe-dom-renderer` のどちらにも依存させたくないという
//! 制約から本クレート（`fandhe-core::ui`）に確定した。
//!
//! [`ModeSwitcher`] は同書 §2.5 の 3 規約（共存方式・排他起動・停止→起動の順序）のうち
//! レンダラ非依存な調停ロジック部分を切り出したものである。
//! **配置に関する設計逸脱**: 設計書 §3.3 の原案は DOM 出力レンダラ（Issue #80）内への
//! 配置を想定していたが、レンダラ非依存という性質を活かして本クレートへ配置した。
//! `wasm32` 結線部（DOM 側 activate/deactivate・canvas 側停止）は DOM 出力レンダラ側が
//! [`RendererLifecycle`] を実装する形で担う想定であり、本モジュールはそれまでの間、
//! host（非 wasm32）の `cargo test` で検証可能な純粋ロジックとして独立して成立する。

/// 画面の出力方式（REQ-4 / TASK-4.4-S3）。Web ターゲットで
/// 独自レンダラ（canvas）と DOM 出力のどちらで描画するかを画面単位に指定する。
/// Web 以外のプラットフォームレイヤはこの値を無視する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// 独自レンダラ（WASM＋WebGL2/canvas）。性能優先の画面向け。既定値（既存挙動を変えない）。
    #[default]
    Canvas,
    /// DOM 出力。アクセシビリティ・SEO 優先の画面向け。
    Dom,
}

/// 1 つのレンダラの起動・停止をレンダラ非依存に抽象化するトレイト。
///
/// `fandhe-dom-renderer`（DOM 側の activate = マウント/イベントリスナー登録、
/// deactivate = アンマウント/リスナー解除）・`fandhe-egui-proxy` 呼び出し側
/// （canvas 側の activate = `eframe::WebRunner` 起動、deactivate = 停止）が
/// それぞれ実装する想定（設計書 §3.3）。[`ModeSwitcher`] はこのトレイトを介して
/// 実際のレンダラ実装を知らないまま排他制御のみを担う。
pub trait RendererLifecycle {
    /// このレンダラを起動する。
    fn activate(&mut self);
    /// このレンダラを停止する。
    fn deactivate(&mut self);
}

/// 現在アクティブな出力方式を保持し、画面遷移時の切替を調停する。
///
/// 設計書 §2.5 の 3 規約を不変条件として保証する:
/// - 同一モードへの再適用は no-op（二重起動を防ぐ）
/// - モード変更時は必ず「旧モードの `deactivate` → 新モードの `activate`」の順で呼ぶ
/// - 上記の結果、いかなる遷移列でも active なレンダラは常に高々 1 つ（二重イベント配線・
///   状態不整合の防止、設計書 §2.5 の共存方式・排他起動の規約に対応）
///
/// `state` ディスパッチ後に `current_screen(state).render_mode()` を評価して
/// [`ModeSwitcher::apply`] を呼ぶのが導線の駆動点になる想定（設計書 §3.3）。
pub struct ModeSwitcher<C, D>
where
    C: RendererLifecycle,
    D: RendererLifecycle,
{
    canvas: C,
    dom: D,
    current: Option<RenderMode>,
}

impl<C, D> ModeSwitcher<C, D>
where
    C: RendererLifecycle,
    D: RendererLifecycle,
{
    /// 両レンダラを未起動の状態で `ModeSwitcher` を作る。
    ///
    /// 呼び出し側が構築前に一方のレンダラを独自に起動済みだった場合でも
    /// 「いかなる遷移列でも active なレンダラは常に高々 1 つ」という不変条件を型レベルで
    /// 保証するため、構築時に両レンダラへ `deactivate` を呼んでから `current` を `None` にする
    /// （Issue #81 レビュー指摘対応）。`deactivate` は「起動していないなら何もしない」冪等な
    /// 実装であることを [`RendererLifecycle`] の契約として前提にする。将来
    /// `fandhe-dom-renderer` 側で wasm32 結線（`RendererLifecycle` 実装）を追加する際も、
    /// この冪等性を満たすように実装すること。
    pub fn new(mut canvas: C, mut dom: D) -> Self {
        canvas.deactivate();
        dom.deactivate();
        Self {
            canvas,
            dom,
            current: None,
        }
    }

    /// 現在アクティブなモードを返す（初回 `apply` 前は `None`）。
    pub fn current(&self) -> Option<RenderMode> {
        self.current
    }

    /// canvas 側レンダラへの可変参照を返す。
    ///
    /// 設計書 §3.3 が想定する「状態ディスパッチ後の再描画」を canvas レンダラ側で
    /// 駆動する呼び出し元（`fandhe-egui-proxy` 呼び出し側）向けの脱出口。
    /// [`ModeSwitcher`] 自体は起動・停止の排他制御のみを担い、起動後の再描画契機までは
    /// 関知しないため、この参照を介して個々のレンダラへ直接アクセスする経路を用意する。
    pub fn canvas_mut(&mut self) -> &mut C {
        &mut self.canvas
    }

    /// DOM 側レンダラへの可変参照を返す。
    ///
    /// `canvas_mut` と対になる脱出口。`fandhe-dom-renderer` 側が
    /// 状態ディスパッチ後の DOM 再描画をこの参照経由で駆動する想定（設計書 §3.3）。
    pub fn dom_mut(&mut self) -> &mut D {
        &mut self.dom
    }

    /// 両レンダラと最終モードを取り出し、`ModeSwitcher` を消費する。
    ///
    /// `ModeSwitcher` が担う排他制御が不要になった場面（テスト終盤の検証、
    /// 呼び出し側でのライフサイクル引き継ぎ等）で個々のレンダラ実体を回収するための API。
    pub fn into_inner(self) -> (C, D, Option<RenderMode>) {
        (self.canvas, self.dom, self.current)
    }

    /// `next` へ切り替える。同一モードなら no-op、異なるモードなら
    /// 旧モードの `deactivate` を呼んだ後に新モードの `activate` を呼ぶ
    /// （初回適用時は `deactivate` を呼ばず `activate` のみ）。
    pub fn apply(&mut self, next: RenderMode) {
        if self.current == Some(next) {
            return;
        }

        if let Some(prev) = self.current {
            self.deactivate_mode(prev);
        }
        self.activate_mode(next);
        self.current = Some(next);
    }

    fn activate_mode(&mut self, mode: RenderMode) {
        match mode {
            RenderMode::Canvas => self.canvas.activate(),
            RenderMode::Dom => self.dom.activate(),
        }
    }

    fn deactivate_mode(&mut self, mode: RenderMode) {
        match mode {
            RenderMode::Canvas => self.canvas.deactivate(),
            RenderMode::Dom => self.dom.deactivate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のモックレンダラ。activate/deactivate の呼び出し回数を記録する。
    #[derive(Default)]
    struct MockRenderer {
        active: bool,
        activate_count: u32,
        deactivate_count: u32,
    }

    impl RendererLifecycle for MockRenderer {
        fn activate(&mut self) {
            self.active = true;
            self.activate_count += 1;
        }

        fn deactivate(&mut self) {
            self.active = false;
            self.deactivate_count += 1;
        }
    }

    /// 既定値が `Canvas` であることを確認する（既存挙動を変えない安全側の既定）。
    #[test]
    fn render_mode_default_is_canvas() {
        assert_eq!(RenderMode::default(), RenderMode::Canvas);
    }

    /// 初回適用は `activate` のみを呼び、`deactivate` は構築時の 1 回のみであることを検証する
    /// （`deactivate_count` の基準値は `new` が両レンダラを未起動状態に揃えるため 1 になる）。
    #[test]
    fn first_apply_only_activates() {
        let mut switcher = ModeSwitcher::new(MockRenderer::default(), MockRenderer::default());

        switcher.apply(RenderMode::Canvas);

        assert_eq!(switcher.canvas.activate_count, 1);
        assert_eq!(switcher.canvas.deactivate_count, 1);
        assert_eq!(switcher.dom.activate_count, 0);
        assert_eq!(switcher.current(), Some(RenderMode::Canvas));
    }

    /// 同一モードへの再適用が no-op になる（二重起動しない）ことを検証する。
    #[test]
    fn reapplying_same_mode_is_noop() {
        let mut switcher = ModeSwitcher::new(MockRenderer::default(), MockRenderer::default());

        switcher.apply(RenderMode::Dom);
        switcher.apply(RenderMode::Dom);
        switcher.apply(RenderMode::Dom);

        assert_eq!(switcher.dom.activate_count, 1);
        assert_eq!(switcher.dom.deactivate_count, 1);
    }

    /// 切替時は必ず旧モードの `deactivate` → 新モードの `activate` の順で呼ばれることを検証する
    /// （`deactivate_count` の基準値は構築時の 1 回を含む）。
    #[test]
    fn switching_deactivates_previous_then_activates_next() {
        let mut switcher = ModeSwitcher::new(MockRenderer::default(), MockRenderer::default());

        switcher.apply(RenderMode::Canvas);
        switcher.apply(RenderMode::Dom);

        assert_eq!(switcher.canvas.activate_count, 1);
        assert_eq!(switcher.canvas.deactivate_count, 2);
        assert_eq!(switcher.dom.activate_count, 1);
        assert_eq!(switcher.dom.deactivate_count, 1);
        assert!(!switcher.canvas.active);
        assert!(switcher.dom.active);
    }

    /// 構築前に一方のレンダラが独自に起動済みであっても、`new` が両レンダラへ
    /// `deactivate` を呼ぶため構築直後は両方非 active になることを検証する
    /// （Issue #81 レビュー指摘: 呼び出し側の前提条件違反を型で吸収する）。
    #[test]
    fn new_deactivates_pre_activated_renderers() {
        let mut pre_activated_canvas = MockRenderer::default();
        pre_activated_canvas.activate();
        assert!(pre_activated_canvas.active);

        let switcher = ModeSwitcher::new(pre_activated_canvas, MockRenderer::default());

        assert!(!switcher.canvas.active);
        assert!(!switcher.dom.active);
        assert_eq!(switcher.current(), None);
    }

    /// `canvas_mut` / `dom_mut` / `into_inner` が個々のレンダラへの直接アクセス経路として
    /// 機能することを検証する（Issue #81 レビュー指摘: 駆動経路の脱出口）。
    #[test]
    fn accessors_expose_individual_renderers() {
        let mut switcher = ModeSwitcher::new(MockRenderer::default(), MockRenderer::default());
        switcher.apply(RenderMode::Dom);

        switcher.canvas_mut().activate_count += 100;
        switcher.dom_mut().activate_count += 200;

        let (canvas, dom, current) = switcher.into_inner();
        assert_eq!(canvas.activate_count, 100);
        assert_eq!(dom.activate_count, 201);
        assert_eq!(current, Some(RenderMode::Dom));
    }

    /// いかなる遷移列でも active なレンダラは常に高々 1 つであることを検証する
    /// （二重イベント配線・状態不整合の防止、設計書 §2.5）。
    #[test]
    fn at_most_one_renderer_is_active_across_any_transition_sequence() {
        let mut switcher = ModeSwitcher::new(MockRenderer::default(), MockRenderer::default());

        for mode in [
            RenderMode::Canvas,
            RenderMode::Dom,
            RenderMode::Dom,
            RenderMode::Canvas,
            RenderMode::Canvas,
            RenderMode::Dom,
        ] {
            switcher.apply(mode);
            let active_count = u32::from(switcher.canvas.active) + u32::from(switcher.dom.active);
            assert_eq!(
                active_count, 1,
                "mode={mode:?} 適用後も active は高々 1 つであるべき"
            );
        }
    }
}
