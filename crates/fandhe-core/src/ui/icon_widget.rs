//! アイコンウィジェットの状態・イベント処理（TASK-8.3-S38 / REQ-8 / REQ-9 / REQ-13、
//! Issue #133）。
//!
//! [`super::image_widget::ImageWidget`]（#130）・[`super::progress::ProgressWidget`]
//! （#127）・[`super::label::LabelWidget`]（#106）と同じ判断（既存 main の公開 API
//! のみに依存する自己完結実装とし、`icon::Icon` との型統合は別作業に委ねる）を
//! 踏襲し、[`Widget<S>`] を実装する自己完結のアイコンウィジェット型 [`IconWidget`]
//! として実装する（`measure` / `paint` は `icon::Icon` 相当の暫定実装）。
//!
//! ## 状態・イベント処理
//!
//! - アイコン識別子の SSOT はアプリ状態 `S`（[`IconWidgetSource::Bound`]）または構築時の固定値
//!   （[`IconWidgetSource::Static`]）。ウィジェットは [`crate::state::ReadFn`] 経由の
//!   [`Widget::sync_from_state`] で表示用キャッシュ `cached_name` を更新する制御型
//!   （`Label` / `ImageWidget` と同じ構造上の理由）。アイコン識別子は不透明文字列として
//!   保持するのみで、`fandhe-core` はグリフ解決・フォント / SVG ロード・I/O を一切
//!   行わない（ヘッドレス契約、OWASP A03 対策）。中間編集状態は持たない。
//! - インタラクション状態（hover/press/disabled/focus）は [`InteractionState`] へ委譲する。
//!   [`IconWidget::apply_input`] / [`IconWidget::set_disabled`] が更新経路を提供し、
//!   `visual_state()` が `Disabled > Pressed > Hovered > Idle` の優先順位で解決する
//!   （[`InteractionState::apply`] / [`InteractionState::set_disabled`] の既存意味論に
//!   そのまま乗る。disabled 中は全入力無視、disabled 化で hover/press/focus を解除する）。
//! - `hit_test` は常に `false`（クリック透過、`Label` / `ImageWidget` と同じ判断）。
//!   アイコン単体は非インタラクティブ要素とし、クリック可能アイコンはボタン（S7〜S9）の
//!   責務としてスコープ外とする。
//! - `paint` は `[Rect（アイコンプレースホルダ枠）, Text(アクセシビリティラベル)]` を返す。
//!   `Text` へは開発者指定のラベル（[`IconWidget::label_text`] が返す構築時の引数）のみを出し、
//!   アイコン識別子（`Bound` 経由で外部入力になり得る）は一切出さない（`ImageWidget` の
//!   alt と同じ OWASP A03 対策）。装飾的アイコン（[`IconWidget::with_decorative`]、`true` の
//!   場合）は `Text` を省略し、支援技術へラベルを公開しない（REQ-9）。
//! - サイズ（[`IconWidget::with_size`]）は非有限値（NaN/inf）・非正値を既定寸法へ
//!   フォールバックする（[`normalize_icon_size`]、`ImageWidget::with_intrinsic_size` と
//!   同型の数値健全性対策、OWASP A04 相当）。
//! - `accessibility_role` は既存の `AccessibilityRole::Image` を流用する（専用バリアントは
//!   追加しない）。accesskit に Icon 専用ロールが存在せず `Role::Image` がアイコンを
//!   包含するロールとして定義されているため（`ProgressIndicator` が `ProgressBar` /
//!   `Spinner` を包含するのと同じ判断）。専用バリアントの要否判断は S37 / S39 に委ね、
//!   並列 issue との衝突面を最小化する安全側の判断とする。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。検証は `Widget`
//! メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) S37（#132）マージ後の型統合（S37 が `icon::Icon<S>` / `icon::IconSource` を導入した
//! 場合の突き合わせ。型名を `IconWidget` / `IconWidgetSource`、ファイル名を
//! `icon_widget.rs` としているため、`image_widget::ImageWidget` と同様シンボル・
//! ファイル衝突は生じない）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S39 / #134）、
//! (c) 実グリフ解決・アセットロード（プラットフォームレイヤ）、
//! (d) 状態別ティントの具体トークン（TASK-8.4）。

use crate::state::ReadFn;

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// `with_size` が非有限値・非正値を無害化する際にフォールバックする既定の幅
/// （論理ピクセル相当のプレースホルダ定数）。
const DEFAULT_ICON_WIDTH: f32 = 24.0;

/// `with_size` が非有限値・非正値を無害化する際にフォールバックする既定の高さ。
/// `DEFAULT_ICON_WIDTH` と同じくプレースホルダ。
const DEFAULT_ICON_HEIGHT: f32 = 24.0;

/// [`IconWidget`] のアイコン識別子指定方法（[`super::label::LabelSource`] /
/// [`super::image_widget::ImageWidgetSource`] の前例を踏襲）。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn<S, String>`]、実体は `fn(&S) -> String`）に限定する。
pub enum IconWidgetSource<S> {
    /// 状態 `S` に依存しない固定のアイコン識別子（不透明文字列）。
    Static(String),
    /// アプリ状態 `S` から識別子を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    /// [`Widget::sync_from_state`] で `cached_name` を更新する。
    Bound(ReadFn<S, String>),
}

// `IconWidgetSource<S>` は `String` と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の `S: Debug` / `S: Clone`
// 境界を回避する（`LabelSource` / `ImageWidgetSource` と同じ設計判断）。
impl<S> std::fmt::Debug for IconWidgetSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconWidgetSource::Static(name) => f.debug_tuple("Static").field(name).finish(),
            IconWidgetSource::Bound(_) => f.debug_tuple("Bound").field(&"<fn ptr>").finish(),
        }
    }
}

impl<S> Clone for IconWidgetSource<S> {
    fn clone(&self) -> Self {
        match self {
            IconWidgetSource::Static(name) => IconWidgetSource::Static(name.clone()),
            IconWidgetSource::Bound(get) => IconWidgetSource::Bound(*get),
        }
    }
}

/// アイコンサイズ（幅・高さ）を正規化する（非有限値・非正値は既定プレースホルダ寸法へ
/// フォールバック）。
///
/// 数値健全性の確保（OWASP A04 相当）のため、後続の `measure` がゼロ除算・NaN 伝播を
/// 起こさないよう構築時点で無害化する（`image_widget::normalize_intrinsic_size` と
/// 同じ判断）。
fn normalize_icon_size(width: f32, height: f32) -> (f32, f32) {
    let width = if width.is_finite() && width > 0.0 {
        width
    } else {
        DEFAULT_ICON_WIDTH
    };
    let height = if height.is_finite() && height > 0.0 {
        height
    } else {
        DEFAULT_ICON_HEIGHT
    };
    (width, height)
}

/// アイコンウィジェット（TASK-8.3-S38、`Widget<S>` 実装型、自己完結の暫定基本描画 +
/// 状態・イベント処理）。
///
/// `Debug` / `Clone` は手動実装する（状態型 `S` を所有していないため `derive` マクロが
/// 自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける）。アイコン識別子・
/// ラベル・寸法は秘密情報に該当しないため、`Debug` にそのまま含める。
pub struct IconWidget<S> {
    id: WidgetId,
    source: IconWidgetSource<S>,
    /// [`Widget::sync_from_state`] が更新する表示用キャッシュ（現在のアイコン識別子）。
    /// 不透明文字列として保持するのみで、`paint` へは出さない（モジュール doc 参照）。
    cached_name: String,
    /// アクセシビリティラベル（REQ-9、`paint` の `Text` 出力に使用）。開発者指定の文字列
    /// であり、アイコン識別子（外部入力になり得る）とは独立させる。
    label: String,
    /// 装飾的アイコンかどうか。`true` の場合、`paint` は `Text`（ラベル）を省略し、
    /// 支援技術へ公開しない（REQ-9）。
    decorative: bool,
    /// アイコンの描画サイズ。構築時 / `with_size` で [`normalize_icon_size`] により
    /// 正規化済み。
    size: Measurement,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for IconWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IconWidget")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("cached_name", &self.cached_name)
            .field("label", &self.label)
            .field("decorative", &self.decorative)
            .field("size", &self.size)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for IconWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            cached_name: self.cached_name.clone(),
            label: self.label.clone(),
            decorative: self.decorative,
            size: self.size,
            interaction: self.interaction,
        }
    }
}

impl<S> IconWidget<S> {
    /// 状態 `S` に依存しない固定のアイコン識別子を持つアイコンを構築する。
    ///
    /// `cached_name` は構築直後から `name` で初期化されるため、`sync_from_state`
    /// 呼び出し前でも `icon_name()` が期待どおりの識別子を返す（`Label::fixed` /
    /// `ImageWidget::fixed` と同じ意味論）。`decorative` は `false`（非装飾）で
    /// 初期化する。
    pub fn fixed(id: WidgetId, name: impl Into<String>, label: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id,
            cached_name: name.clone(),
            source: IconWidgetSource::Static(name),
            label: label.into(),
            decorative: false,
            size: Measurement {
                width: DEFAULT_ICON_WIDTH,
                height: DEFAULT_ICON_HEIGHT,
            },
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に束縛されたアイコン識別子を持つアイコンを構築する。
    ///
    /// `cached_name` は構築直後は空文字列であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、`Label::bound` /
    /// `ImageWidget::bound` と同じ判断）。
    pub fn bound(id: WidgetId, get: ReadFn<S, String>, label: impl Into<String>) -> Self {
        Self {
            id,
            source: IconWidgetSource::Bound(get),
            cached_name: String::new(),
            label: label.into(),
            decorative: false,
            size: Measurement {
                width: DEFAULT_ICON_WIDTH,
                height: DEFAULT_ICON_HEIGHT,
            },
            interaction: InteractionState::default(),
        }
    }

    /// 描画サイズ（幅・高さ）を設定するビルダーメソッド。
    ///
    /// 非有限値（NaN/inf）または `<= 0.0` の成分は既定プレースホルダ寸法へ
    /// フォールバックする（[`normalize_icon_size`] 参照、安全側の判断）。
    pub fn with_size(mut self, width: f32, height: f32) -> Self {
        let (width, height) = normalize_icon_size(width, height);
        self.size = Measurement { width, height };
        self
    }

    /// 装飾的アイコンかどうかを設定するビルダーメソッド。
    ///
    /// `true` を指定すると `paint` はラベルの `Text` を省略し、支援技術へ公開しない
    /// （REQ-9、モジュール doc 参照）。
    pub fn with_decorative(mut self, decorative: bool) -> Self {
        self.decorative = decorative;
        self
    }

    /// 現在の表示キャッシュ値（アイコン識別子）を返す（テスト・観測用）。
    pub fn icon_name(&self) -> &str {
        &self.cached_name
    }

    /// アクセシビリティラベルを返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// 装飾的アイコンかどうかを返す。
    pub fn is_decorative(&self) -> bool {
        self.decorative
    }

    /// 描画サイズ（構築時・`with_size` 時点で正規化済み）を返す。
    pub fn size(&self) -> Measurement {
        self.size
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える（[`InteractionState::set_disabled`] への委譲）。
    ///
    /// アイコンには中間編集状態がなく確定すべき編集バッファも存在しないため、
    /// そのまま委譲するのみで済む（`ImageWidget::set_disabled` と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// ポインタ・フォーカス入力を適用する（[`InteractionState::apply`] への委譲）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);
    }
}

impl<S: 'static> Widget<S> for IconWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` からアイコン識別子を読み取り `cached_name` を更新する
    /// （reconcile フェーズ）。未配線の契約はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        match &self.source {
            IconWidgetSource::Static(name) => {
                // 固定識別子は状態に依存しないため不変。呼び出し自体は許容し、
                // reconcile ループの呼び出し規約（全ウィジェット一律呼び出し）を崩さない。
                self.cached_name = name.clone();
            }
            IconWidgetSource::Bound(get) => {
                self.cached_name = get(state);
            }
        }
    }

    /// 構築時 / `with_size` で正規化済みの描画サイズを、`available` を超えないよう
    /// クランプして返す（`Label::measure` と同じ単純なクランプ方針）。
    fn measure(&self, available: Measurement) -> Measurement {
        Measurement {
            width: self.size.width.min(available.width.max(0.0)),
            height: self.size.height.min(available.height.max(0.0)),
        }
    }

    /// `[Rect（アイコンプレースホルダ枠）, Text(アクセシビリティラベル)]` の描画
    /// プリミティブ列を返す。`decorative` が `true` の場合は `Text` を省略する
    /// （モジュール doc 参照）。アイコン識別子（外部入力になり得る）は一切出さない
    /// （OWASP A03 対策）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        if self.decorative {
            vec![PaintCommand::Rect]
        } else {
            vec![PaintCommand::Rect, PaintCommand::Text(self.label.clone())]
        }
    }

    /// アイコンは表示専用要素であり、常に `false` を返す（`Label::hit_test` /
    /// `ImageWidget::hit_test` と同じ安全側の判断）。クリック可能アイコンはボタン
    /// （S7〜S9）の責務でありスコープ外（モジュール doc 参照）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    /// 既存の `AccessibilityRole::Image` を流用する（モジュール doc 参照。専用バリアント
    /// の要否判断は S37 / S39 に委ねる）。
    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Image
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。[`super::icon::Icon::paint_styled`]
    /// と同型（Display カテゴリの既定マッピングを用い、構造を保ったまま色・
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

    struct NavState {
        active_icon: String,
    }

    fn read_active_icon(state: &NavState) -> String {
        state.active_icon.clone()
    }

    // (a) 構築・sync_from_state による cached_name 更新

    #[test]
    fn fixed_returns_name_immediately_without_sync() {
        let icon: IconWidget<NavState> = IconWidget::fixed(WidgetId::from_raw(1), "home", "ホーム");

        assert_eq!(icon.icon_name(), "home");
        assert_eq!(icon.label_text(), "ホーム");
        assert!(!icon.is_decorative());
    }

    #[test]
    fn bound_is_empty_before_first_sync() {
        let icon: IconWidget<NavState> =
            IconWidget::bound(WidgetId::from_raw(2), read_active_icon, "ナビゲーション");

        assert_eq!(icon.icon_name(), "");
    }

    #[test]
    fn sync_from_state_keeps_static_name_unchanged() {
        let mut icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(3), "home", "ホーム");
        let state = NavState {
            active_icon: "settings".to_string(),
        };

        icon.sync_from_state(&state);

        assert_eq!(icon.icon_name(), "home");
    }

    #[test]
    fn sync_from_state_reflects_bound_state_across_updates() {
        let mut icon: IconWidget<NavState> =
            IconWidget::bound(WidgetId::from_raw(4), read_active_icon, "ナビゲーション");

        icon.sync_from_state(&NavState {
            active_icon: "home".to_string(),
        });
        assert_eq!(icon.icon_name(), "home");

        icon.sync_from_state(&NavState {
            active_icon: "settings".to_string(),
        });
        assert_eq!(icon.icon_name(), "settings");
    }

    // (b) disabled 中は apply_input が no-op / disabled 化で hover・press 解除

    #[test]
    fn disabled_ignores_pointer_and_focus_input() {
        let mut icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(5), "home", "ホーム");
        icon.set_disabled(true);

        icon.apply_input(InteractionInput::PointerEntered);
        icon.apply_input(InteractionInput::PointerPressed);
        icon.apply_input(InteractionInput::FocusGained);

        let interaction = icon.interaction_state();
        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.focused);
        assert!(interaction.disabled);
    }

    #[test]
    fn set_disabled_true_clears_hover_and_press() {
        let mut icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(6), "home", "ホーム");
        icon.apply_input(InteractionInput::PointerEntered);
        icon.apply_input(InteractionInput::PointerPressed);
        assert!(icon.interaction_state().hovered);
        assert!(icon.interaction_state().pressed);

        icon.set_disabled(true);

        let interaction = icon.interaction_state();
        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(interaction.disabled);
    }

    // (c) apply_input による hover/press 遷移と visual_state の優先順位

    #[test]
    fn apply_input_transitions_hovered_and_pressed() {
        let mut icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(7), "home", "ホーム");

        icon.apply_input(InteractionInput::PointerEntered);
        assert_eq!(icon.visual_state(), VisualState::Hovered);

        icon.apply_input(InteractionInput::PointerPressed);
        assert_eq!(icon.visual_state(), VisualState::Pressed);

        icon.apply_input(InteractionInput::PointerReleased);
        assert_eq!(icon.visual_state(), VisualState::Hovered);

        icon.apply_input(InteractionInput::PointerExited);
        assert_eq!(icon.visual_state(), VisualState::Idle);
    }

    #[test]
    fn visual_state_priority_disabled_over_pressed_over_hovered() {
        let mut icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(8), "home", "ホーム");
        icon.apply_input(InteractionInput::PointerEntered);
        icon.apply_input(InteractionInput::PointerPressed);
        assert_eq!(icon.visual_state(), VisualState::Pressed);

        icon.set_disabled(true);
        assert_eq!(icon.visual_state(), VisualState::Disabled);
    }

    // (d) hit_test は常に false

    #[test]
    fn hit_test_is_always_false_even_inside_bounds() {
        let icon: IconWidget<NavState> = IconWidget::fixed(WidgetId::from_raw(9), "home", "ホーム");
        let size = Measurement {
            width: 24.0,
            height: 24.0,
        };

        assert!(!icon.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!icon.hit_test((-10.0, -10.0), (0.0, 0.0), size));
    }

    // (e) paint の decorative / 非 decorative 差分、識別子が Text へ漏れないこと

    #[test]
    fn paint_includes_rect_and_label_when_not_decorative() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(10), "home", "ホームへ移動");

        let commands = icon.paint(
            (0.0, 0.0),
            Measurement {
                width: 24.0,
                height: 24.0,
            },
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("ホームへ移動".to_string()),
            ]
        );
    }

    #[test]
    fn paint_omits_text_when_decorative() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(11), "sparkle", "キラキラ").with_decorative(true);

        let commands = icon.paint(
            (0.0, 0.0),
            Measurement {
                width: 24.0,
                height: 24.0,
            },
        );

        assert_eq!(commands, vec![PaintCommand::Rect]);
    }

    #[test]
    fn paint_never_leaks_bound_icon_identifier_into_text() {
        let mut icon: IconWidget<NavState> =
            IconWidget::bound(WidgetId::from_raw(12), read_active_icon, "現在のタブ");
        icon.sync_from_state(&NavState {
            active_icon: "secret-internal-id".to_string(),
        });

        let commands = icon.paint(
            (0.0, 0.0),
            Measurement {
                width: 24.0,
                height: 24.0,
            },
        );

        for command in &commands {
            if let PaintCommand::Text(text) = command {
                assert!(!text.contains("secret-internal-id"));
            }
        }
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("現在のタブ".to_string()),
            ]
        );
    }

    // (f) サイズ正規化（非有限値・非正値のフォールバック）

    #[test]
    fn with_size_sets_positive_finite_dimensions() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(13), "home", "ホーム").with_size(32.0, 32.0);

        let size = icon.size();
        assert_eq!(size.width, 32.0);
        assert_eq!(size.height, 32.0);
    }

    #[test]
    fn with_size_falls_back_when_invalid() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(14), "home", "ホーム").with_size(f32::NAN, -1.0);

        let size = icon.size();
        assert_eq!(size.width, DEFAULT_ICON_WIDTH);
        assert_eq!(size.height, DEFAULT_ICON_HEIGHT);
    }

    #[test]
    fn default_size_is_placeholder_constant() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(15), "home", "ホーム");

        let size = icon.size();
        assert_eq!(size.width, DEFAULT_ICON_WIDTH);
        assert_eq!(size.height, DEFAULT_ICON_HEIGHT);
    }

    // (g) 付随の Widget<S> 契約（id / accessibility_role / clone_box / object safety）

    #[test]
    fn accessibility_role_is_image() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(16), "home", "ホーム");
        assert_eq!(icon.accessibility_role(), AccessibilityRole::Image);
    }

    #[test]
    fn clone_box_preserves_cached_name_and_id() {
        let mut icon: IconWidget<NavState> =
            IconWidget::bound(WidgetId::from_raw(17), read_active_icon, "現在のタブ");
        icon.sync_from_state(&NavState {
            active_icon: "profile".to_string(),
        });

        let boxed: Box<dyn Widget<NavState>> = Box::new(icon);
        let cloned = boxed.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(17));
        let commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 24.0,
                height: 24.0,
            },
        );
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("現在のタブ".to_string()),
            ]
        );
    }

    #[test]
    fn debug_output_does_not_panic() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(18), "home", "ホーム");
        let output = format!("{icon:?}");
        assert!(output.contains("IconWidget"));
    }

    #[test]
    fn icon_widget_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<NavState>> =
            Box::new(IconWidget::fixed(WidgetId::from_raw(19), "home", "ホーム"));
        assert_eq!(widget.accessibility_role(), AccessibilityRole::Image);
        assert!(!widget.interaction_state().disabled);
    }

    #[test]
    fn icon_widget_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<IconWidget<NavState>>();
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let icon: IconWidget<NavState> =
            IconWidget::fixed(WidgetId::from_raw(200), "home", "ホーム");
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 24.0,
            height: 24.0,
        };

        assert_eq!(
            icon.paint_styled((0.0, 0.0), size, &empty_resolver),
            icon.paint((0.0, 0.0), size)
        );
    }
}
