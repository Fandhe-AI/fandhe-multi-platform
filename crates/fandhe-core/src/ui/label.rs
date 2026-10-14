//! ラベル・テキストウィジェット（TASK-8.3-S10 / S11、REQ-8 / REQ-13、Issue #105 / #106）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」）の分解 sub-issue（10/60・11/60）。
//! [`super::widget::Widget`] トレイト（TASK-8.3-S2 / #97）の契約を成立させる最初期の
//! 具体ウィジェット型として、本モジュールは 2 つの独立した型を提供する。
//!
//! - [`Label`]（S10 / #105）: API 設計・基本描画を担当する型。既存の
//!   [`super::node::Node::Text`]（状態 `S` に依存しない静的文字列）とは異なり、
//!   状態 `S` に束縛されたテキスト表示を [`LabelSource::Bound`]（関数ポインタ限定、
//!   REQ-13）で提供する。
//! - [`LabelWidget`]（S11 / #106）: 状態・イベント処理（hover/press/focus 等の
//!   インタラクション状態の実配線）を担当する型。
//!
//! S10 と S11 はそれぞれ既存 main の公開 API のみに依存する自己完結の型として
//! 独立に実装している（`ui::hit_test` / `ui::catalog` 等が採ったのと同じ判断）。
//! 両者は名前が異なり実体も独立しているため統合を行わず両立させる。
//! 型統合（重複する描画・計測ロジックの一本化）は別 issue のスコープとし、
//! `.claude/rules/out-of-scope-tracking.md` のフロー対象として追跡する。
//!
//! **未配線契約（widget.rs 参照）**: `render_headless`（本クレート）・`convert_node`
//! （`fandhe-web-dom`）・`to_snapshot_node`（`fandhe-swiftui-bridge`）は `&Node<S>`
//! 共有参照のみを持ち `sync_from_state` を呼べないため、`Node::Widget` 経由で
//! 組み込んだ本モジュールのウィジェットは現状これらのバックエンドでは無出力
//! （プレースホルダ）のままとなる。S3（#98）が reconcile/sync パスを確立するまで
//! この契約は変更しない。本モジュールが提供するのは `Widget::paint` 自体の実装と、
//! `WidgetHarness` 等を通じた直接呼び出し経路での検証までである。

use crate::state::ReadFn;

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 1 文字あたりの近似幅（論理ピクセル相当のプレースホルダ定数、[`Label`] 用）。
///
/// `cosmic-text`（REQ-8）未導入のため、実フォントメトリクスの代わりに単純な
/// 文字数比例の近似計測を行う。`cosmic-text` 導入時にこの近似は置き換えられる想定
/// （`docs/architecture/widget-architecture.md` §8 でスコープ外と明記済み）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の近似行高（論理ピクセル相当のプレースホルダ定数、[`Label`] 用）。折り返し・
/// 複数行はスコープ外（`APPROX_CHAR_WIDTH` と同じ置換点）。
const APPROX_LINE_HEIGHT: f32 = 16.0;

/// [`Label`] のテキスト供給源。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn<S, String>`]、実体は `fn(&S) -> String`）に限定する。
pub enum LabelSource<S> {
    /// 状態 `S` に依存しない固定文言。
    Static(String),
    /// 状態 `S` から都度読み取る束縛テキスト（[`Widget::sync_from_state`] で
    /// キャッシュを更新する。reconcile フェーズより前は空文字列のまま）。
    Bound(ReadFn<S, String>),
}

// `LabelSource<S>` は `String` と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の
// `S: Debug` / `S: Clone` 境界を回避する（`node.rs` / `screen.rs` と同じ設計判断）。
impl<S> std::fmt::Debug for LabelSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LabelSource::Static(text) => f.debug_tuple("Static").field(text).finish(),
            LabelSource::Bound(_) => f.debug_tuple("Bound").field(&"<fn ptr>").finish(),
        }
    }
}

impl<S> Clone for LabelSource<S> {
    fn clone(&self) -> Self {
        match self {
            LabelSource::Static(text) => LabelSource::Static(text.clone()),
            LabelSource::Bound(get) => LabelSource::Bound(*get),
        }
    }
}

/// ラベル・テキストウィジェット（TASK-8.3-S10 / Issue #105）。
///
/// [`Widget<S>`] を実装する最初期の具体型。`sync_from_state` で更新される表示
/// キャッシュ（`cached_text`）を保持し、`paint` は常にこのキャッシュ値のみを
/// 参照する（`Bound` の場合、構築直後は空文字列であり初回 `sync_from_state` まで
/// 表示に反映されない reconcile 意味論）。
///
/// `Debug` / `Clone` は `Screen<S>` / `Node<S>` と同じ理由（状態 `S` を所有しない
/// フィールドのみで構成されるため、`derive` が付与する不要な `S: Debug` /
/// `S: Clone` 境界を避ける）で手動実装する。
pub struct Label<S> {
    id: WidgetId,
    source: LabelSource<S>,
    cached_text: String,
    /// hover/press/disabled/focus の一時的状態。[`LabelWidget`]（S11）が更新経路を
    /// 配線するのに対し、`Label` 自体は `Default`（すべて false）を保持するのみ。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Label<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Label")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("cached_text", &self.cached_text)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Label<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            cached_text: self.cached_text.clone(),
            interaction: self.interaction,
        }
    }
}

impl<S> Label<S> {
    /// 状態 `S` に依存しない固定文言のラベルを構築する。
    ///
    /// `cached_text` は構築直後から `text` で初期化されるため、`sync_from_state`
    /// 呼び出し前でも `paint` が期待どおりの文言を返す。
    pub fn fixed(id: WidgetId, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            id,
            cached_text: text.clone(),
            source: LabelSource::Static(text),
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に束縛されたラベルを構築する。
    ///
    /// `cached_text` は構築直後は空文字列であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、[`Widget::sync_from_state`]
    /// 参照）。
    pub fn bound(id: WidgetId, get: ReadFn<S, String>) -> Self {
        Self {
            id,
            source: LabelSource::Bound(get),
            cached_text: String::new(),
            interaction: InteractionState::default(),
        }
    }

    /// 現在の表示キャッシュ値を返す（テスト・観測用）。
    pub fn text(&self) -> &str {
        &self.cached_text
    }
}

impl<S: 'static> Widget<S> for Label<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn sync_from_state(&mut self, state: &S) {
        match &self.source {
            LabelSource::Static(text) => {
                // 固定文言は状態に依存しないため不変。呼び出し自体は許容し、
                // reconcile ループの呼び出し規約（全ウィジェット一律呼び出し）を崩さない。
                self.cached_text = text.clone();
            }
            LabelSource::Bound(get) => {
                self.cached_text = get(state);
            }
        }
    }

    /// 単一行モノスペース近似で必要寸法を返す（`cosmic-text` 未導入時のプレースホルダ、
    /// モジュール doc の `APPROX_CHAR_WIDTH` / `APPROX_LINE_HEIGHT` 参照）。
    ///
    /// マルチバイト文字（日本語等）はバイト長ではなく `chars().count()`（Unicode
    /// スカラ値数）を基準にする。`available` の幅でクランプし、折り返し・複数行は
    /// 行わない（スコープ外）。
    fn measure(&self, available: Measurement) -> Measurement {
        let natural_width = self.cached_text.chars().count() as f32 * APPROX_CHAR_WIDTH;
        Measurement {
            width: natural_width.min(available.width),
            // 高さのクランプは仕様上要求されていない（モジュール doc は幅のクランプのみ
            // 明言）ため、常に単一行の近似行高をそのまま返す。
            height: APPROX_LINE_HEIGHT,
        }
    }

    /// 現在の表示キャッシュを [`PaintCommand::Text`] へ変換する。
    ///
    /// `origin` / `size` は将来の描画バックエンド（矩形背景・下線等）向けの拡張点
    /// として受け取るのみで、現時点ではテキスト内容の生成に使わない。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Text(self.cached_text.clone())]
    }

    /// ラベルは表示専用要素であり、常に `false` を返す（クリック透過）。
    ///
    /// イベントディスパッチ対象へ不用意に載せない安全側の判断。リンク的挙動が
    /// 必要になった場合の再検討は [`LabelWidget`]（S11）側へ委ねる（トレイト自体の
    /// シグネチャは変更しない）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Label
    }

    /// 表示中のテキストそのものをアクセシブルネームとして公開する
    /// （TASK-9.1-S2、REQ-9）。空文字列の場合はアクセシブルネームなし（`None`）。
    fn accessibility_label(&self) -> Option<String> {
        if self.cached_text.is_empty() {
            None
        } else {
            Some(self.cached_text.clone())
        }
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。Display カテゴリの既定マッピング
    /// （`style_tokens::display_style_refs`）を用い、`paint()` と同じ構造を保った
    /// まま文字色・タイポグラフィのみを解決済み具体値へ差し替える。解決失敗時は
    /// `paint()` の無スタイル列へフォールバックする。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        resolver: &crate::token::TokenResolver,
    ) -> Vec<PaintCommand> {
        let Ok(refs) = super::style_tokens::display_style_refs() else {
            return self.paint(origin, size);
        };
        let Ok(resolved) = super::style_tokens::resolve_widget_style(
            resolver,
            &refs,
            self.interaction_state().visual_state(),
        ) else {
            return self.paint(origin, size);
        };
        match super::style_tokens::style_paint_commands(self.paint(origin, size), &resolved) {
            Some(styled) => styled,
            None => self.paint(origin, size),
        }
    }
}

/// ラベル・テキストウィジェット（TASK-8.3-S11 で新設する自己完結の最小実装）。
///
/// `measure` / `paint` は元々 S10（#105）の基本描画実装がスコープであったため、
/// 本型では状態・イベント処理を単体で検証可能にするための暫定実装（文字数に応じた
/// 素朴な計測、`PaintCommand::Text` への変換のみ）を提供する。正式なテキスト
/// レイアウト（`cosmic-text` 等、TASK-8.4 以降）への置き換え、および [`Label`]
/// との型統合は別作業のスコープとする。
pub struct LabelWidget<S> {
    id: WidgetId,
    /// 状態 `S` から動的テキストを読み取る関数ポインタ。`None` の場合は `cached_text` を
    /// 構築時の値のまま保持する静的テキストとして扱う（`sync_from_state` が no-op になる）。
    text_source: Option<ReadFn<S, String>>,
    /// 直近の `sync_from_state`（または構築時の初期値）で確定した表示用キャッシュ文字列。
    cached_text: String,
    interaction: InteractionState,
}

impl<S> LabelWidget<S> {
    /// 静的テキストのみを持つラベルを構築する（`sync_from_state` は no-op）。
    pub fn new_static(id: WidgetId, text: impl Into<String>) -> Self {
        Self {
            id,
            text_source: None,
            cached_text: text.into(),
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` から動的にテキストを読み取るラベルを構築する。
    ///
    /// `initial` は最初の `sync_from_state` 呼び出し前に `text()` / `paint()` が参照する
    /// 初期表示値（未同期状態でも空文字を返さないための保険）。
    pub fn new_dynamic(
        id: WidgetId,
        initial: impl Into<String>,
        text_source: ReadFn<S, String>,
    ) -> Self {
        Self {
            id,
            text_source: Some(text_source),
            cached_text: initial.into(),
            interaction: InteractionState::default(),
        }
    }

    /// 現在の表示用キャッシュ文字列（`sync_from_state` 反映後の最新値）。
    pub fn text(&self) -> &str {
        &self.cached_text
    }

    /// disabled フラグを切り替える（[`InteractionState::set_disabled`] への委譲）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// ポインタ・フォーカス入力を適用する（[`InteractionState::apply`] への委譲）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }
}

impl<S> std::fmt::Debug for LabelWidget<S> {
    /// `read.rs` の `ReadAccessor` と同じ理由（derive が `S: Debug` の不要な境界を課すのを
    /// 避けるため）で手動実装する。関数ポインタの有無のみ記録し、アドレスは出力しない。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelWidget")
            .field("id", &self.id)
            .field("cached_text", &self.cached_text)
            .field("interaction", &self.interaction)
            .field("has_text_source", &self.text_source.is_some())
            .finish()
    }
}

impl<S> Clone for LabelWidget<S> {
    /// `read.rs` の `ReadAccessor` と同じ理由で手動実装する（derive が `S: Clone` の
    /// 不要な境界を課すのを避ける。関数ポインタは `Copy` であり複製にコストがかからない）。
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            text_source: self.text_source,
            cached_text: self.cached_text.clone(),
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Widget<S> for LabelWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn sync_from_state(&mut self, state: &S) {
        if let Some(read) = self.text_source {
            self.cached_text = read(state);
        }
    }

    fn measure(&self, available: Measurement) -> Measurement {
        // 暫定計測（正式なテキストレイアウトへの置き換えは別作業）。
        // 1 文字あたり幅 8.0・高さ 16.0 固定という単純な近似値を用い、
        // available を上回らないようクランプする。
        let approx_width = (self.cached_text.chars().count() as f32) * 8.0;
        Measurement {
            width: approx_width.min(available.width),
            height: available.height.min(16.0),
        }
    }

    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Text(self.cached_text.clone())]
    }

    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        // 設計判断: ラベル・テキストは発火イベントを持たない非インタラクティブ要素であり、
        // 常にポインタ透過とする（下層ウィジェットへのイベント配送を遮らない）。
        false
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Label
    }

    /// 表示中のテキストそのものをアクセシブルネームとして公開する
    /// （TASK-9.1-S2、REQ-9）。空文字列の場合はアクセシブルネームなし（`None`）。
    fn accessibility_label(&self) -> Option<String> {
        if self.cached_text.is_empty() {
            None
        } else {
            Some(self.cached_text.clone())
        }
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。[`Label::paint_styled`] と同型
    /// （Display カテゴリの既定マッピングを用い、構造を保ったまま文字色・
    /// タイポグラフィのみ差し替える）。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        resolver: &crate::token::TokenResolver,
    ) -> Vec<PaintCommand> {
        let Ok(refs) = super::style_tokens::display_style_refs() else {
            return self.paint(origin, size);
        };
        let Ok(resolved) = super::style_tokens::resolve_widget_style(
            resolver,
            &refs,
            self.interaction_state().visual_state(),
        ) else {
            return self.paint(origin, size);
        };
        match super::style_tokens::style_paint_commands(self.paint(origin, size), &resolved) {
            Some(styled) => styled,
            None => self.paint(origin, size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CounterState {
        count: i32,
    }

    fn read_count_label(state: &CounterState) -> String {
        format!("count: {}", state.count)
    }

    #[test]
    fn fixed_label_returns_text_immediately_without_sync() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(1), "hello");
        assert_eq!(label.text(), "hello");

        let commands = label.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Text("hello".to_string())]);
    }

    #[test]
    fn bound_label_is_empty_before_first_sync() {
        let label: Label<CounterState> = Label::bound(WidgetId::from_raw(2), read_count_label);
        assert_eq!(label.text(), "");
    }

    #[test]
    fn bound_label_reflects_state_after_sync() {
        let mut label: Label<CounterState> = Label::bound(WidgetId::from_raw(3), read_count_label);
        label.sync_from_state(&CounterState { count: 3 });
        assert_eq!(label.text(), "count: 3");

        let commands = label.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Text("count: 3".to_string())]);
    }

    #[test]
    fn bound_label_cache_follows_state_changes_across_syncs() {
        let mut label: Label<CounterState> = Label::bound(WidgetId::from_raw(4), read_count_label);
        label.sync_from_state(&CounterState { count: 1 });
        assert_eq!(label.text(), "count: 1");

        label.sync_from_state(&CounterState { count: 2 });
        assert_eq!(label.text(), "count: 2");
    }

    #[test]
    fn measure_scales_with_char_count_and_clamps_to_available() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(5), "abcdefghij");
        let unclamped = label.measure(Measurement {
            width: 1000.0,
            height: 100.0,
        });
        assert_eq!(unclamped.width, 10.0 * APPROX_CHAR_WIDTH);

        let clamped = label.measure(Measurement {
            width: 5.0,
            height: 100.0,
        });
        assert_eq!(clamped.width, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        // 「こんにちは」は 5 文字（Unicode スカラ値）だが、UTF-8 では 1 文字 3 バイトの
        // ため 15 バイト。`chars().count()` 基準であることを固定化する回帰テスト。
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(6), "こんにちは");
        let measurement = label.measure(Measurement {
            width: 1000.0,
            height: 100.0,
        });
        assert_eq!(measurement.width, 5.0 * APPROX_CHAR_WIDTH);
    }

    #[test]
    fn hit_test_is_always_false_even_inside_bounds() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(7), "hello");
        let size = Measurement {
            width: 50.0,
            height: 20.0,
        };
        assert!(!label.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn accessibility_role_is_label() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(8), "hello");
        assert_eq!(label.accessibility_role(), AccessibilityRole::Label);
    }

    #[test]
    fn accessibility_label_is_displayed_text() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(8), "hello");
        assert_eq!(label.accessibility_label(), Some("hello".to_string()));
    }

    #[test]
    fn accessibility_label_is_none_when_text_is_empty() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(8), "");
        assert_eq!(label.accessibility_label(), None);
    }

    #[test]
    fn id_and_default_interaction_state_are_reported() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(9), "hello");
        assert_eq!(label.id(), WidgetId::from_raw(9));
        assert_eq!(label.interaction_state(), InteractionState::default());
    }

    #[test]
    fn clone_box_preserves_cached_text_and_id() {
        let mut label: Label<CounterState> = Label::bound(WidgetId::from_raw(10), read_count_label);
        label.sync_from_state(&CounterState { count: 42 });

        let boxed: Box<dyn Widget<CounterState>> = Box::new(label);
        let cloned = boxed.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(10));
        let commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Text("count: 42".to_string())]);
    }

    #[test]
    fn debug_output_does_not_panic() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(11), "hello");
        let output = format!("{label:?}");
        assert!(output.contains("Label"));
    }

    #[test]
    fn label_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<CounterState>> =
            Box::new(Label::fixed(WidgetId::from_raw(12), "hello"));
        let commands = widget.paint(
            (0.0, 0.0),
            Measurement {
                width: 1000.0,
                height: 100.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Text("hello".to_string())]);
    }

    /// `Widget<S>: Send + Sync` 境界を `Label<S>` が満たすことの静的アサーション
    /// （PR #357 レビュー指摘対応と同じ回帰防止パターン、widget.rs 参照）。
    #[test]
    fn label_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Label<CounterState>>();
    }

    /// テスト専用の状態型。`LabelWidget` の動的テキスト読み取り経路を検証するために使う。
    struct GreetingState {
        name: String,
    }

    fn read_greeting(state: &GreetingState) -> String {
        format!("こんにちは、{}さん", state.name)
    }

    #[test]
    fn label_widget_static_sync_from_state_is_no_op() {
        let mut label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(21), "固定テキスト");
        let state = GreetingState {
            name: "太郎".to_string(),
        };

        label.sync_from_state(&state);

        assert_eq!(label.text(), "固定テキスト");
    }

    #[test]
    fn label_widget_dynamic_sync_from_state_updates_cached_text() {
        let mut label = LabelWidget::new_dynamic(WidgetId::from_raw(22), "", read_greeting);
        let state = GreetingState {
            name: "花子".to_string(),
        };

        label.sync_from_state(&state);

        assert_eq!(label.text(), "こんにちは、花子さん");
    }

    #[test]
    fn label_widget_dynamic_sync_from_state_tracks_state_changes() {
        let mut label = LabelWidget::new_dynamic(WidgetId::from_raw(23), "", read_greeting);

        label.sync_from_state(&GreetingState {
            name: "一郎".to_string(),
        });
        assert_eq!(label.text(), "こんにちは、一郎さん");

        label.sync_from_state(&GreetingState {
            name: "二郎".to_string(),
        });
        assert_eq!(label.text(), "こんにちは、二郎さん");
    }

    #[test]
    fn label_widget_hit_test_is_always_false_even_inside_rect() {
        let label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(24), "ラベル");
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        // 座標がウィジェット矩形の内側であっても、ラベルは常にポインタ透過を返す。
        assert!(!label.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!label.hit_test((-10.0, -10.0), (0.0, 0.0), size));
    }

    #[test]
    fn label_widget_set_disabled_updates_interaction_state_and_visual_state() {
        let mut label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(25), "ラベル");

        label.set_disabled(true);

        assert!(label.interaction_state().disabled);
        assert_eq!(label.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn label_widget_disabled_ignores_pointer_and_focus_input() {
        let mut label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(26), "ラベル");
        label.set_disabled(true);

        label.apply_input(InteractionInput::PointerEntered);
        label.apply_input(InteractionInput::PointerPressed);
        label.apply_input(InteractionInput::FocusGained);

        let interaction = label.interaction_state();
        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.focused);
        assert!(interaction.disabled);
    }

    #[test]
    fn label_widget_accessibility_role_is_label() {
        let label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(27), "ラベル");

        assert_eq!(label.accessibility_role(), AccessibilityRole::Label);
    }

    #[test]
    fn label_widget_accessibility_label_is_displayed_text() {
        let label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(27), "ラベル");

        assert_eq!(label.accessibility_label(), Some("ラベル".to_string()));
    }

    #[test]
    fn label_widget_clone_box_preserves_id_and_cached_text() {
        let mut label = LabelWidget::new_dynamic(WidgetId::from_raw(28), "", read_greeting);
        label.sync_from_state(&GreetingState {
            name: "三郎".to_string(),
        });

        let cloned: Box<dyn Widget<GreetingState>> = label.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(28));
        let commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 10.0,
                height: 10.0,
            },
        );
        assert_eq!(
            commands,
            vec![PaintCommand::Text("こんにちは、三郎さん".to_string())]
        );
    }

    #[test]
    fn label_widget_trait_object_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<GreetingState>> =
            Box::new(LabelWidget::new_static(WidgetId::from_raw(29), "テキスト"));

        assert_eq!(widget.accessibility_role(), AccessibilityRole::Label);
        assert!(!widget.interaction_state().disabled);
    }

    #[test]
    fn label_widget_debug_output_does_not_panic() {
        let label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(30), "ラベル");
        let output = format!("{label:?}");
        assert!(output.contains("LabelWidget"));
    }

    // --- TASK-8.4-S14: paint_styled ---

    fn test_resolver_for_display() -> crate::token::TokenResolver {
        use crate::token::{
            SemanticColorMapping, SemanticColorRole, TokenPath, TokenRef, TokenSet,
        };

        let mut base = crate::token::palette_token_set().expect("パレット構築は成功する");
        base.extend(crate::token::typography_token_set().expect("タイポグラフィ構築は成功する"));
        SemanticColorMapping::new()
            .with(
                SemanticColorRole::OnBackground,
                TokenRef::new(TokenPath::from_dotted("color.palette.gray-900").unwrap()),
            )
            .with(
                // display_style_refs の背景スロット（Surface）解決に必要。Label は
                // Rect を持たないため実際の描画には使われないが、`resolve_widget_style`
                // は設定済みスロットを一括で解決するため、未登録だと
                // UndefinedReference で無スタイルへフォールバックしてしまう。
                SemanticColorRole::Surface,
                TokenRef::new(TokenPath::from_dotted("color.palette.gray-100").unwrap()),
            )
            .merge_into(&mut base)
            .expect("マージできる");
        crate::token::TokenResolver::new(base, TokenSet::new())
    }

    #[test]
    fn label_paint_styled_resolves_on_background_color() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(31), "hello");
        let resolver = test_resolver_for_display();
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        let commands = label.paint_styled((0.0, 0.0), size, &resolver);

        assert_eq!(
            commands,
            vec![PaintCommand::StyledText {
                content: "hello".to_string(),
                color: crate::token::palette::GRAY_900,
                typography: match &commands[0] {
                    PaintCommand::StyledText { typography, .. } => typography.clone(),
                    _ => panic!("StyledText を期待"),
                },
            }]
        );
    }

    #[test]
    fn label_paint_styled_falls_back_when_resolver_lacks_tokens() {
        let label: Label<CounterState> = Label::fixed(WidgetId::from_raw(32), "hello");
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        let styled = label.paint_styled((0.0, 0.0), size, &empty_resolver);
        assert_eq!(styled, label.paint((0.0, 0.0), size));
    }

    #[test]
    fn label_widget_paint_styled_resolves_on_background_color() {
        let label: LabelWidget<GreetingState> =
            LabelWidget::new_static(WidgetId::from_raw(33), "ラベル");
        let resolver = test_resolver_for_display();
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        let commands = label.paint_styled((0.0, 0.0), size, &resolver);
        assert!(matches!(commands[0], PaintCommand::StyledText { .. }));
    }
}
