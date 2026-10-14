//! ボタンウィジェット `Button<S>`（TASK-8.3-S7 / REQ-8、Issue #102）。
//!
//! ウィジェット基盤（`Widget<S>` トレイト・`InteractionState`・ヒットテスト・
//! レイアウト・`WidgetHarness`・`WidgetCatalog`）と、同型の先行ウィジェット
//! [`super::slider::Slider`]（Issue #123）・[`super::checkbox::Checkbox`]（Issue #114）が
//! 確立したパターンを踏襲し、`Widget<S>` を実装する具体型 [`Button`] が
//! 基本描画（measure / paint / hit_test / accessibility_role）を実装する。
//!
//! [`super::widget::AccessibilityRole::Button`] は定義済みのため、
//! `widget.rs` への変更は不要（`Slider` が専用バリアントを追加したのと異なる点）。
//!
//! 既存の宣言層 `Node::Button`（TASK-1.1 由来、`label` + `on_click: fn(&mut S)`）と
//! `event::click_button` / `state::Event::ButtonClicked` は変更しない
//! （後方互換な加法的追加。`docs/architecture/widget-architecture.md` §4.1 の段階的
//! 移行方針どおり `Node::Widget(Box<dyn Widget<S>>)` 経由でツリーへ組み込む）。
//!
//! - ラベルは静的な表示文言であり、アプリ状態 `S` に束縛される値を持たない
//!   （状態束縛ラベルは下記スコープ外 (c)）。そのため [`Widget::sync_from_state`] は
//!   no-op とする。
//! - `on_click: fn(&mut S)` はイベント配線（S8、押下ディスパッチ）に備えたシグネチャ
//!   確保のみで、この関数ポインタを呼び出す経路は実装しない（`Slider::bound` が `set` を
//!   先行して受けたのと同じ判断）。環境キャプチャ不可の関数ポインタに限定することで、
//!   状態アクセス経路を型レベルで `&mut S` 引数のみに制限する（REQ-13）。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3 バックエンドの
//!   プレースホルダ契約への波及を避ける — `Checkbox` / `ToggleSwitch` / `Slider` と同じ
//!   判断）。描画列は `[Rect（ボタン背景）, Text(label)]` の 2 コマンドとする。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。reconcile/sync パスが
//! `Node::Widget` へ実際に `&mut` を届けるまで、上記 3 箇所は `Node::Widget` を
//! 状態非依存のプレースホルダとして扱う契約を維持する。検証は
//! `Widget` メソッドの直接呼び出しで行う。
//!
//! TASK-8.3-S8（REQ-8、Issue #103）: 押下確定・キーボード活性化・disabled/focus 連動を
//! 実装する。[`super::toggle_switch::ToggleSwitch`]（Issue #121）と同じ判断で既存
//! [`Button<S>`] へ直接メソッドを追加する（label/text_field/checkbox で発生した
//! 「同型ウィジェットが 2 つ存在する」問題を繰り返さないための判断）。
//!
//! - 押下確定（ポインタ・キーボード活性化）は既存の [`crate::state::Event::ButtonClicked`]
//!   へブリッジするのみで、widget 層から `&mut S` へ直接触れない（`ToggleSwitch::apply_input` /
//!   `handle_input` と同じブリッジ設計）。専用イベントバリアント `Event::ButtonActivated` の
//!   新設は `Node<S>` enum への variant 追加が設計文書 §4.1 の温存方針に反するためスコープ外
//!   とする（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）。
//! - ポインタ確定は [`InteractionState::apply`] へ入力を適用する**前**の
//!   `pressed && hovered` で判定する（press → 領域外 exit → release では発火しない、
//!   `ToggleSwitch::apply_input` と同一の意味論）。
//! - キーボード活性化は新設 [`ButtonInput`]（Space/Enter 相当）で表現し、生キーコード・
//!   文字列を扱わず意味づけ済み列挙のみで構成する（OWASP A03 対策、`CheckboxInput` /
//!   `ToggleSwitchInput` と同型）。`focused` かつ非 `disabled` のときのみ発火する。
//! - `on_click: fn(&mut S)` / `click_fn()` は宣言層（`Node::Button` / `event::click_button`）
//!   との契約明示アクセサのまま維持し、直接呼び出さない
//!   （`ToggleSwitch::write_fn` が未使用アクセサのままなのと同じ判断）。確定は
//!   すべて `state::Event::ButtonClicked` へブリッジし、ディスパッチは
//!   `StateStore::dispatch` → `event::click_button`（レガシー `.button()` ブリッジ）の
//!   既存経路が担う。
//!
//! TASK-8.3-S9（REQ-8、Issue #104）では、上記 (b) のスコープ（`WidgetHarness` を使った
//! 網羅テスト・`WidgetCatalog` 登録・`tests/button_widget.rs` / ギャラリー掲載）を実施した。
//! `catalog.rs` の `button` エントリをレガシー `Node::Button`（`Screen::button` ビルダー）
//! から本モジュールの `Button<S>`（`Node::Widget` 経由）へ差し替え、`name` はキーとして
//! `button` のまま温存した（`catalog.rs` の `builtin_entries` doc・`button_widget.rs`
//! モジュール doc 参照）。専用 `Screen` ビルダー（`.button_widget()` 等）は新設せず、
//! `Screen::with_node(Node::Widget(Box::new(Button::new(..))))` で構築する
//! （`.button()` の名前がレガシーに占有されているため、命名の API 設計判断は急がない
//! 判断。out-of-scope-tracking.md 準拠、PR 本文に記録する）。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) `Event::ButtonActivated` 等の専用バリアント新設 + `WidgetId` ベースディスパッチ統合
//! （`ToggleSwitch` S26 / `Checkbox` S20 と同じ残課題）、
//! (b) `WidgetHarness` を使った網羅テスト・`WidgetCatalog` 登録・
//! `tests/button_widget.rs` / ギャラリー掲載（S9 / Issue #104 で実施済み）、
//! (c) 状態束縛ラベル（`LabelSource` 相当の `ReadFn<S, String>` 対応）・アイコン付き
//! ボタン等のバリアント（必要になった時点で別 issue）、
//! (d) reconcile パス未配線の既知契約（上記参照、既存契約を変更しない）、
//! (e) cosmic-text による実テキスト計測（設計文書で既にスコープ外と明記済み）、
//! (f) `Screen<S>` に `Button<S>` 専用のビルダー（`.button_widget()` 等）を新設すること
//! （`.button()` はレガシー `Node::Button` に占有済み、S9 では急がない判断）。

use crate::state::Event;
use crate::token::TokenResolver;

use super::style_tokens::{StyleTokenRefs, action_style_refs, resolve_widget_style};
use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, StateStyles,
    VisualState, Widget, WidgetId,
};

/// ボタンへのキーボード入力を表す列挙（TASK-8.3-S8、`Button<S>` の核心）。
///
/// 外部由来の生キーコード・文字列を直接扱わず、意味づけ済みの列挙のみで構成することで
/// OWASP A03（インジェクション）対策を型レベルで担保する（`checkbox::CheckboxInput` /
/// `toggle_switch::ToggleSwitchInput` / `widget::InteractionInput` と同じ設計判断、
/// `.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonInput {
    /// 押下を確定する（Space/Enter 相当）。
    Activate,
}

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数であり、
/// `measure` の折り返し・複数行はスコープ外（`Checkbox` / `ToggleSwitch` / `Slider` と
/// 同じ既知の制約）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の高さ近似（論理ピクセル相当）。`APPROX_CHAR_WIDTH` と同じくプレースホルダ。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// ボタン背景の左右パディング（論理ピクセル相当）。
const PADDING_X: f32 = 12.0;

/// ボタン背景の上下パディング（論理ピクセル相当）。
const PADDING_Y: f32 = 6.0;

/// ボタンウィジェット（TASK-8.3-S7、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`Slider` / `ToggleSwitch` / `Checkbox` と同じく、
/// 状態型 `S` を所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone`
/// という不要な境界を避ける）。ラベルは秘密情報に該当しないため、`Debug` にそのまま
/// 含める。`on_click` は関数ポインタで `Copy` のため手動実装で問題ない。
pub struct Button<S> {
    id: WidgetId,
    label: String,
    /// 押下ハンドラ（REQ-13、環境キャプチャ不可の関数ポインタ限定）。実際の呼び出し
    /// 経路（イベントディスパッチ）は後続工程（S8）が別途配線する。
    on_click: fn(&mut S),
    /// hover/pressed/disabled/focus。実際の値変更配線は後続工程（S8）のスコープ。
    interaction: InteractionState,
    /// 状態別スタイルトークン参照（TASK-8.4-S14、#170）。既定は
    /// [`action_style_refs`]（背景 Primary・文字 OnPrimary）。基本ウィジェットの中で
    /// 唯一 `style` フィールドをカスタマイズ可能にした例示実装であり、他の基本
    /// ウィジェットは既定マッピングを都度呼び出す軽量な配線に留める
    /// （`.claude/rules/out-of-scope-tracking.md` 準拠、PR 本文に記録）。
    style: StateStyles<StyleTokenRefs>,
}

impl<S> std::fmt::Debug for Button<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Button")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("interaction", &self.interaction)
            .field("style", &self.style)
            .finish()
    }
}

impl<S> Clone for Button<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            on_click: self.on_click,
            interaction: self.interaction,
            style: self.style.clone(),
        }
    }
}

impl<S> Button<S> {
    /// ボタンを作る（ビルダー形式の起点）。
    ///
    /// `on_click` は [`crate::state`] の SSOT 型エイリアスと同型の関数ポインタに限定する
    /// （REQ-13、環境キャプチャクロージャを型レベルで拒否する）。コンストラクタに
    /// `on_click` を含める理由は、後続の押下ディスパッチ実装（S8）でビルダーシグネチャを
    /// 破壊変更しないためであり、この関数ポインタを呼び出す経路は実装しない
    /// （`Slider::bound` と同じ判断）。
    pub fn new(id: WidgetId, label: impl Into<String>, on_click: fn(&mut S)) -> Self {
        Self {
            id,
            label: label.into(),
            on_click,
            interaction: InteractionState::default(),
            // action_style_refs() は通常失敗しないが、`Button::new` は `Result` を
            // 返せない構築 API（設計文書 §4.3 の既存シグネチャを崩さない）ため、
            // panic を避けて全スロット `None` の空マッピングへ委譲する
            // （coding-rust.md の panic 禁止方針。`resolve_widget_style` は空
            // マッピングを解決成功として扱い、`style_paint_commands` が
            // 背景色なしを検出して `paint()` の無スタイル列へフォールバックする、
            // モジュール doc 参照）。
            style: action_style_refs().unwrap_or_default(),
        }
    }

    /// 状態別スタイルトークン参照を差し替える（ビルダー形式、TASK-8.4-S14）。
    pub fn with_style(mut self, style: StateStyles<StyleTokenRefs>) -> Self {
        self.style = style;
        self
    }

    /// 表示文言（ラベル）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// イベント配線が使う押下ハンドラを返す。
    ///
    /// この関数ポインタを呼び出す経路（押下ディスパッチ）は
    /// 実装しない契約明示のためのアクセサであり、`dead_code` 警告の回避も兼ねる
    /// （`Slider::write_fn` と同型）。
    pub fn click_fn(&self) -> fn(&mut S) {
        self.on_click
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// ポインタ・フォーカス入力を適用する（TASK-8.3-S8 の核心）。
    ///
    /// 押下確定の判定は [`InteractionState::apply`] へ入力を適用する**前**の
    /// `pressed && hovered` を見て行う。`PointerReleased` 適用前に判定することで、
    /// press → 領域外へ exit（`pressed` が解除される）→ release という経路では
    /// 発火しない（press-and-drag-away の意味論、
    /// `toggle_switch::ToggleSwitch::apply_input` と同じく `InteractionState` の
    /// 既存意味論をそのまま利用する設計）。disabled 中は `InteractionState::apply` が
    /// 入力そのものを無視するため、`pressed` が立たず押下確定も発火しない。
    pub fn apply_input(&mut self, input: InteractionInput) -> Option<Event> {
        let should_confirm = matches!(input, InteractionInput::PointerReleased)
            && self.interaction.pressed
            && self.interaction.hovered;

        self.interaction.apply(input);

        if should_confirm {
            return Some(Event::ButtonClicked {
                label: self.label.clone(),
            });
        }
        None
    }

    /// キーボード入力を処理する（TASK-8.3-S8 の核心）。
    ///
    /// disabled 中・非フォーカス中はすべての入力を無視する
    /// （`toggle_switch::ToggleSwitch::handle_input` と同じ「フォーカス保持中のみ
    /// 操作可能」という設計判断）。
    pub fn handle_input(&mut self, input: ButtonInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused {
            return None;
        }

        match input {
            ButtonInput::Activate => Some(Event::ButtonClicked {
                label: self.label.clone(),
            }),
        }
    }
}

impl<S: 'static> Widget<S> for Button<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// ラベルは静的でアプリ状態 `S` に束縛される値を持たないため no-op
    /// （状態束縛ラベルはモジュール doc のスコープ外 (c) を参照）。
    fn sync_from_state(&mut self, _state: &S) {}

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ実装、
    /// 折り返し・複数行はスコープ外）。
    ///
    /// 幅 = ラベル幅 + 左右パディングを `available.width` でクランプする。
    /// 高さ = 行高 + 上下パディングを `available.height` でクランプする。マルチバイトは
    /// `chars().count()` 基準の近似（`Checkbox` / `ToggleSwitch` / `Slider` と同じ
    /// 既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let label_width = self.label.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let width = (label_width + 2.0 * PADDING_X).min(available.width);
        let height = (APPROX_LINE_HEIGHT + 2.0 * PADDING_Y).min(available.height);

        Measurement { width, height }
    }

    /// `[Rect（ボタン背景）, Text(label)]` の描画プリミティブ列を返す。
    ///
    /// `PaintCommand` 自体は拡張しない（モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Rect, PaintCommand::Text(self.label.clone())]
    }

    /// 矩形包含判定（AABB）。`interaction.disabled` の間は `false` を返し、
    /// 無効化されたボタンを入力対象にしない（安全側の判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if self.interaction.disabled {
            return false;
        }
        point.0 >= origin.0
            && point.0 < origin.0 + size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + size.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Button
    }

    /// ボタン文言をアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
    fn accessibility_label(&self) -> Option<String> {
        Some(self.label.clone())
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。`self.style` を `visual_state()` で
    /// 選択し `resolver` 経由で解決した具体値を `[StyledRect（背景）, StyledText(label)]`
    /// として返す。解決失敗時は `paint()` の無スタイル列へフォールバックする。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        resolver: &TokenResolver,
    ) -> Vec<PaintCommand> {
        let Ok(resolved) = resolve_widget_style(resolver, &self.style, self.visual_state()) else {
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
    use crate::token::{
        SemanticColorMapping, SemanticColorRole, TokenPath, TokenRef, TokenSet, palette,
    };

    struct CounterState {
        count: i32,
    }

    /// テスト用の最小 `TokenResolver`（Primary/OnPrimary のみ登録した Action 相当）。
    fn test_resolver() -> TokenResolver {
        let mut base = crate::token::palette_token_set().expect("パレット構築は成功する");
        base.extend(crate::token::typography_token_set().expect("タイポグラフィ構築は成功する"));
        SemanticColorMapping::new()
            .with(
                SemanticColorRole::Primary,
                TokenRef::new(TokenPath::from_dotted("color.palette.blue-500").unwrap()),
            )
            .with(
                SemanticColorRole::OnPrimary,
                TokenRef::new(TokenPath::from_dotted("color.palette.white").unwrap()),
            )
            .merge_into(&mut base)
            .expect("マージできる");
        TokenResolver::new(base, TokenSet::new())
    }

    #[test]
    fn paint_styled_returns_resolved_color_and_typography() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(401), "送信", increment);
        let resolver = test_resolver();

        let commands = button.paint_styled(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 32.0,
            },
            &resolver,
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::StyledRect {
                    fill: palette::BLUE_500
                },
                PaintCommand::StyledText {
                    content: "送信".to_string(),
                    color: palette::WHITE,
                    typography: crate::token::typography_token_set()
                        .unwrap()
                        .get(&TokenPath::from_dotted("typography.body").unwrap())
                        .and_then(|def| match def {
                            crate::token::TokenDefinition::Value {
                                value: crate::token::TokenValue::Typography(t),
                            } => Some(t.clone()),
                            _ => None,
                        })
                        .expect("typography.body は Typography 値"),
                }
            ]
        );
    }

    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_is_missing_tokens() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(402), "送信", increment);
        let empty_resolver = TokenResolver::new(TokenSet::new(), TokenSet::new());

        let commands = button.paint_styled(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 32.0,
            },
            &empty_resolver,
        );

        assert_eq!(
            commands,
            vec![PaintCommand::Rect, PaintCommand::Text("送信".to_string())]
        );
    }

    fn increment(state: &mut CounterState) {
        state.count += 1;
    }

    // (a) 構築と各アクセサ

    #[test]
    fn new_constructs_with_label_and_default_interaction() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        assert_eq!(button.label_text(), "送信");
        assert_eq!(button.id(), WidgetId::from_raw(1));
        assert_eq!(button.interaction_state(), InteractionState::default());
    }

    #[test]
    fn click_fn_returns_configured_on_click_function_pointer() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        let click_fn = button.click_fn();
        let mut state = CounterState { count: 0 };
        click_fn(&mut state);
        assert_eq!(state.count, 1);
    }

    // (b) measure

    #[test]
    fn measure_approximates_width_from_char_count() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "メモ", increment);

        let measurement = button.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = (2.0 * APPROX_CHAR_WIDTH) + 2.0 * PADDING_X;
        let expected_height = APPROX_LINE_HEIGHT + 2.0 * PADDING_Y;
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, expected_height);
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let button: Button<CounterState> = Button::new(
            WidgetId::from_raw(1),
            "非常に長いラベルです非常に長いラベルです",
            increment,
        );

        let measurement = button.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "あ", increment);

        let measurement = button.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = (1.0 * APPROX_CHAR_WIDTH) + 2.0 * PADDING_X;
        assert_eq!(measurement.width, expected_width);
    }

    // (c) paint

    #[test]
    fn paint_returns_background_rect_and_label_text() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        let commands = button.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 32.0,
            },
        );

        assert_eq!(
            commands,
            vec![PaintCommand::Rect, PaintCommand::Text("送信".to_string())]
        );
    }

    // (d) hit_test の半開区間・disabled 時 false

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        let hit = button.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 32.0,
            },
        );

        assert!(hit);
    }

    #[test]
    fn hit_test_returns_false_when_point_outside_rect() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        let hit = button.hit_test(
            (500.0, 500.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 32.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_on_bottom_right_edge() {
        // `Rect::contains` の半開区間仕様（[origin, origin + size)）に合わせ、
        // 右端・下端ちょうどの点は非ヒットとする（隣接ウィジェットとの境界重複を避ける）。
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        let hit = button.hit_test(
            (100.0, 32.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 32.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_for_zero_size_at_origin() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        let hit = button.hit_test(
            (0.0, 0.0),
            (0.0, 0.0),
            Measurement {
                width: 0.0,
                height: 0.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_point_inside_rect() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(1), "送信", increment);
        button.set_disabled(true);

        let hit = button.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 32.0,
            },
        );

        assert!(!hit);
    }

    // (e) sync_from_state の no-op 契約

    #[test]
    fn sync_from_state_is_no_op_and_paint_is_unchanged() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(1), "送信", increment);
        let size = Measurement {
            width: 100.0,
            height: 32.0,
        };
        let before = button.paint((0.0, 0.0), size);

        button.sync_from_state(&CounterState { count: 42 });

        let after = button.paint((0.0, 0.0), size);
        assert_eq!(before, after);
    }

    // (f) accessibility_role / clone_box / Debug / visual_state

    #[test]
    fn accessibility_role_is_button() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        assert_eq!(button.accessibility_role(), AccessibilityRole::Button);
    }

    #[test]
    fn accessibility_label_is_button_text() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        assert_eq!(button.accessibility_label(), Some("送信".to_string()));
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(1), "送信", increment);
        button.set_disabled(true);

        let cloned = button.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));
        assert_eq!(cloned.interaction_state(), button.interaction_state());

        // clone 元とクローンは独立したインスタンスであること（disabled を戻しても
        // クローン側には影響しない）。
        button.set_disabled(false);
        assert!(cloned.interaction_state().disabled);
    }

    #[test]
    fn debug_output_contains_label() {
        let button: Button<CounterState> = Button::new(WidgetId::from_raw(1), "送信", increment);

        let output = format!("{button:?}");

        assert!(output.contains("送信"));
    }

    #[test]
    fn visual_state_priority_is_delegated_to_interaction_state() {
        use super::super::widget::InteractionInput;

        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(1), "送信", increment);

        button.interaction.apply(InteractionInput::PointerEntered);
        assert_eq!(button.visual_state(), VisualState::Hovered);

        button.set_disabled(true);
        assert_eq!(button.visual_state(), VisualState::Disabled);
    }

    // (g) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(1), "送信", increment);
        let state = CounterState { count: 0 };

        button.sync_from_state(&state);
        let measurement = button.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        let commands = button.paint((0.0, 0.0), measurement);

        assert!(measurement.width > 0.0);
        assert_eq!(
            commands,
            vec![PaintCommand::Rect, PaintCommand::Text("送信".to_string())]
        );
    }

    // (h) 状態・イベント処理（TASK-8.3-S8）

    use crate::state::StateStore;
    use crate::ui::Screen;

    fn focus(button: &mut Button<CounterState>) {
        button.apply_input(InteractionInput::FocusGained);
    }

    #[test]
    fn pointer_press_and_release_inside_confirms_click() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(301), "送信", increment);

        button.apply_input(InteractionInput::PointerEntered);
        button.apply_input(InteractionInput::PointerPressed);
        let event = button.apply_input(InteractionInput::PointerReleased);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "送信".to_string(),
            })
        );
    }

    #[test]
    fn pointer_press_and_drag_away_does_not_confirm_click() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(302), "送信", increment);

        button.apply_input(InteractionInput::PointerEntered);
        button.apply_input(InteractionInput::PointerPressed);
        button.apply_input(InteractionInput::PointerExited);
        let event = button.apply_input(InteractionInput::PointerReleased);

        assert_eq!(event, None);
    }

    #[test]
    fn release_without_prior_press_does_not_confirm_click() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(303), "送信", increment);

        button.apply_input(InteractionInput::PointerEntered);
        let event = button.apply_input(InteractionInput::PointerReleased);

        assert_eq!(event, None);
    }

    #[test]
    fn disabled_button_ignores_pointer_press_and_release() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(304), "送信", increment);
        button.set_disabled(true);

        button.apply_input(InteractionInput::PointerEntered);
        button.apply_input(InteractionInput::PointerPressed);
        let event = button.apply_input(InteractionInput::PointerReleased);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_activate_confirms_when_focused() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(305), "送信", increment);
        focus(&mut button);

        let event = button.handle_input(ButtonInput::Activate);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "送信".to_string(),
            })
        );
    }

    #[test]
    fn keyboard_activate_ignored_when_not_focused() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(306), "送信", increment);

        let event = button.handle_input(ButtonInput::Activate);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_activate_ignored_when_disabled_even_if_focused() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(307), "送信", increment);
        focus(&mut button);
        button.set_disabled(true);

        assert!(!button.interaction_state().focused);

        let event = button.handle_input(ButtonInput::Activate);

        assert_eq!(event, None);
    }

    #[test]
    fn visual_state_priority_reflects_pointer_input_sequence() {
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(308), "送信", increment);

        button.apply_input(InteractionInput::PointerEntered);
        assert_eq!(button.visual_state(), VisualState::Hovered);

        button.apply_input(InteractionInput::PointerPressed);
        assert_eq!(button.visual_state(), VisualState::Pressed);

        button.set_disabled(true);
        assert_eq!(button.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn state_store_dispatch_increments_state_via_legacy_button_bridge() {
        // 宣言層はレガシー `.button(label, handler)` で受け、`StateStore::dispatch` を
        // 経由して状態へ反映されることを確認する（モジュール doc の設計判断の E2E 検証。
        // `toggle_switch::ToggleSwitch` の同名テストと同型）。
        let screen: Screen<CounterState> =
            Screen::new("form").button(WidgetId::from_raw(1), "送信", increment);
        let mut store = StateStore::new(CounterState { count: 0 });
        let mut button: Button<CounterState> =
            Button::new(WidgetId::from_raw(309), "送信", increment);
        focus(&mut button);

        let event = button
            .handle_input(ButtonInput::Activate)
            .expect("フォーカス中の Activate は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);

        let event = button
            .handle_input(ButtonInput::Activate)
            .expect("フォーカス中の Activate は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert_eq!(store.state().count, 2);
        assert_eq!(store.version(), 2);
    }
}
