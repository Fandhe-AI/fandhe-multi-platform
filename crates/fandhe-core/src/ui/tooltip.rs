//! ツールチップウィジェット `Tooltip<S>`（TASK-8.3-S55 / REQ-8、Issue #150）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の分解
//! sub-issue（55/60）。ツールチップの API 設計・基本描画を担い、
//! `Widget<S>` トレイト（TASK-8.3-S2 / #97）と、先行ウィジェットが確立した
//! 以下のパターンを組み合わせて [`Tooltip`] を新設する。
//!
//! - テキストの状態束縛（[`super::label::Label`] S10 / [`super::image::Image`] S34）:
//!   本文の SSOT はアプリ状態 `S`（[`TooltipSource::Bound`]）または構築時の固定値
//!   （[`TooltipSource::Static`]）。ウィジェットは [`crate::state::ReadFn`] 経由の
//!   [`Widget::sync_from_state`] で表示用キャッシュ `cached_text` を更新するのみで、
//!   中間編集状態を持たない。
//! - 可視状態に応じた描画切替（[`super::dialog::Dialog`] S46）: `visible == false` の
//!   間、ツールチップのバブルは「存在しないもの」として扱う（`paint` に一切含めない）。
//!   `Dialog` と異なり表示制御をアプリ状態 `S` へ束縛せず、明示 API
//!   [`Tooltip::show`] / [`Tooltip::hide`] のみで切り替える最小可視モデルとする
//!   （hover 起因の表示・遅延タイマーは S56 のスコープ、モジュール doc 下部参照）。
//! - コンテナ型のアンカー保持（[`super::dialog::Dialog::with_content`] /
//!   [`super::scroll_view::ScrollView`] S40）: ツールチップが装飾する対象
//!   `anchor: Option<Box<dyn Widget<S>>>` を保持し、`sync_from_state` / `hit_test` は
//!   アンカーへ委譲するのみで `S` へ直接触れない。
//!
//! - **`measure`**: アンカーの計測結果をそのまま返す（アンカーなしはゼロ寸法）。
//!   ツールチップのバブルはオーバーレイでありレイアウト空間を消費しない、という
//!   本ウィジェットの中核セマンティクス。
//! - **`paint`**: アンカーの `paint` 結果に続けて、`visible && !disabled` のときのみ
//!   `[Rect（バブル背景）, Text(cached_text)]` を連結する。非表示時はバブル分を
//!   一切出さない（`Dialog` の「閉=不存在」前例と同じ判断）。`PaintCommand` は拡張しない
//!   （`Text(String)` / `Rect` の 2 種のまま、3 バックエンドのプレースホルダ契約に
//!   波及するため安全側に倒す — `Dialog` / `Tabs` と同じ判断）。
//! - **`hit_test`**: アンカーへ委譲する（アンカーなしは常に `false`）。バブル自体は
//!   非インタラクティブ（ツールチップは装飾であり操作対象ではない）。
//! - [`super::widget::AccessibilityRole::Tooltip`] を本ウィジェット専用のロールとして
//!   `widget.rs` へ加法的に追加する（`accesskit::Role::Tooltip` へ対応予定）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。この契約を
//! 変更しない。検証は `Widget` メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) hover 起因の表示・非表示、表示遅延タイマー、`InteractionInput` 配線（S56 / #151）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S57 / #152）、
//! (c) バブルの配置（上下左右）・座標・z-order（`PaintCommand` に座標・レイヤが表現
//! できないため拡張を見送り、`Dialog` S46 (c) と同じ判断）、
//! (d) バブル内リッチコンテンツ（テキスト以外）、
//! (e) reconcile パス未配線の既知契約（上記参照、既存契約を変更しない）。

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, VisualState, Widget, WidgetId,
};
use crate::state::ReadFn;

// バブルの座標・寸法（パディング・行高等）は `PaintCommand` が座標・レイヤを表現
// できないため算出しない（`measure` はアンカー寸法をそのまま返す契約、
// モジュール doc 参照）。`Dialog::APPROX_LINE_HEIGHT` 等の先行定数は自前の寸法計算が
// 必要な場合のプレースホルダであり、本ウィジェットは計算自体を持たないため定義しない
// （未使用定数による `dead_code` 警告を避ける判断）。

/// [`Tooltip`] の本文テキスト指定方法（[`super::label::LabelSource`] /
/// [`super::image::ImageSource`] の前例を踏襲）。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn<S, String>`]、実体は `fn(&S) -> String`）に限定する。
pub enum TooltipSource<S> {
    /// 固定の本文テキスト。
    Static(String),
    /// アプリ状態 `S` から本文テキストを読み取る関数ポインタ（REQ-13、
    /// 環境キャプチャ不可）。[`Widget::sync_from_state`] で `cached_text` を更新する。
    Bound(ReadFn<S, String>),
}

// `TooltipSource<S>` は `String` と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の
// `S: Debug` / `S: Clone` 境界を回避する（`ImageSource` / `LabelSource` と同じ設計判断）。
impl<S> std::fmt::Debug for TooltipSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TooltipSource::Static(text) => f.debug_tuple("Static").field(text).finish(),
            TooltipSource::Bound(_) => f.debug_tuple("Bound").field(&"<fn ptr>").finish(),
        }
    }
}

impl<S> Clone for TooltipSource<S> {
    fn clone(&self) -> Self {
        match self {
            TooltipSource::Static(text) => TooltipSource::Static(text.clone()),
            TooltipSource::Bound(get) => TooltipSource::Bound(*get),
        }
    }
}

/// ツールチップウィジェット（TASK-8.3-S55、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（[`super::dialog::Dialog`] と同じく、状態型 `S` を
/// 所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける。`Clone` は `anchor` を [`Widget::clone_box`] 経由で複製する）。
pub struct Tooltip<S> {
    id: WidgetId,
    source: TooltipSource<S>,
    /// [`Widget::sync_from_state`] が更新する表示用キャッシュ（現在の本文テキスト）。
    cached_text: String,
    /// ツールチップが装飾するアンカー（[`super::dialog::Dialog::with_content`] と
    /// 同型のコンテナパターン）。`measure` / `hit_test` はアンカーへ委譲する。
    anchor: Option<Box<dyn Widget<S>>>,
    /// バブルの表示・非表示。既定は `false`（[`Self::fixed`] / [`Self::bound`] 参照）。
    /// S55 では明示 API [`Self::show`] / [`Self::hide`] のみで切り替える最小可視モデル
    /// とし、hover 起因の表示・遅延タイマーは S56（#151）のスコープとする。
    visible: bool,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Tooltip<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tooltip")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("cached_text", &self.cached_text)
            .field("anchor", &self.anchor)
            .field("visible", &self.visible)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Tooltip<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            cached_text: self.cached_text.clone(),
            anchor: self.anchor.as_ref().map(|a| a.clone_box()),
            visible: self.visible,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Tooltip<S> {
    /// 状態 `S` に依存しない固定の本文テキストを持つツールチップを構築する。
    ///
    /// `cached_text` は構築直後から `source` で初期化されるため、
    /// `sync_from_state` 呼び出し前でも [`Self::text`] が期待どおりの本文を返す
    /// （`Image::fixed` / `Label::fixed` と同じ意味論）。既定は非表示・アンカーなし。
    pub fn fixed(id: WidgetId, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            id,
            cached_text: text.clone(),
            source: TooltipSource::Static(text),
            anchor: None,
            visible: false,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に束縛された本文テキストを持つツールチップを構築する。
    ///
    /// `cached_text` は構築直後は空文字列であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、`Image::bound` と同じ判断）。
    pub fn bound(id: WidgetId, get: ReadFn<S, String>) -> Self {
        Self {
            id,
            source: TooltipSource::Bound(get),
            cached_text: String::new(),
            anchor: None,
            visible: false,
            interaction: InteractionState::default(),
        }
    }

    /// ツールチップが装飾するアンカーを設定するビルダーメソッド
    /// （[`super::dialog::Dialog::with_content`] と同型の単一子コンテナパターン）。
    pub fn with_anchor(mut self, content: impl Widget<S> + 'static) -> Self {
        self.anchor = Some(Box::new(content));
        self
    }

    /// 現在の表示キャッシュ値（本文テキスト）を返す。
    pub fn text(&self) -> &str {
        &self.cached_text
    }

    /// バブルが表示中かどうかを返す。
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// バブルを表示状態にする（明示 API、hover 連動は S56 のスコープ）。
    pub fn show(&mut self) {
        self.visible = true;
    }

    /// バブルを非表示状態にする。
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// disabled フラグを切り替える。
    ///
    /// ツールチップには中間編集状態がなく確定すべき編集バッファも存在しないため、
    /// `InteractionState::set_disabled` へそのまま委譲するのみで済む
    /// （`Image::set_disabled` と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }
}

impl<S: 'static> Widget<S> for Tooltip<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から本文テキストを読み取り `cached_text` を更新し（reconcile
    /// フェーズ）、アンカーへ `sync_from_state` を委譲する
    /// （[`super::dialog::Dialog::sync_from_state`] と同じコンテナ委譲パターン。
    /// ツールチップ自身は表示制御を `S` へ束縛しない、モジュール doc 参照）。
    fn sync_from_state(&mut self, state: &S) {
        match &self.source {
            TooltipSource::Static(text) => {
                // 固定テキストは状態に依存しないため不変。呼び出し自体は許容し、
                // reconcile ループの呼び出し規約（全ウィジェット一律呼び出し）を崩さない。
                self.cached_text = text.clone();
            }
            TooltipSource::Bound(get) => {
                self.cached_text = get(state);
            }
        }
        if let Some(anchor) = self.anchor.as_mut() {
            anchor.sync_from_state(state);
        }
    }

    /// アンカーの計測結果をそのまま返す（アンカーなしはゼロ寸法）。ツールチップの
    /// バブルはオーバーレイでありレイアウト空間を消費しない、という本ウィジェットの
    /// 中核セマンティクス（モジュール doc 参照）。
    fn measure(&self, available: Measurement) -> Measurement {
        let Some(anchor) = self.anchor.as_ref() else {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        };

        // 非有限値（NaN/inf）・非正値はゼロへ倒す（`Image::measure` / `Slider` と同型の
        // 数値健全性担保、OWASP A04 相当）。
        let available = Measurement {
            width: if available.width.is_finite() && available.width > 0.0 {
                available.width
            } else {
                0.0
            },
            height: if available.height.is_finite() && available.height > 0.0 {
                available.height
            } else {
                0.0
            },
        };

        anchor.measure(available)
    }

    /// アンカーの `paint` 結果に続けて、`visible && !disabled` のときのみ
    /// `[Rect（バブル背景）, Text(cached_text)]` を連結する。非表示時はバブル分を
    /// 一切出さない（`Dialog` の「閉=不存在」前例、モジュール doc 参照）。
    /// `PaintCommand` 自体は拡張しない。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        let mut commands = Vec::new();

        if let Some(anchor) = self.anchor.as_ref() {
            commands.extend(anchor.paint(origin, size));
        }

        if self.visible && !self.interaction.disabled {
            commands.push(PaintCommand::Rect);
            commands.push(PaintCommand::Text(self.cached_text.clone()));
        }

        commands
    }

    /// アンカーへ委譲する（アンカーなしは常に `false`）。バブル自体は非インタラクティブ
    /// （ツールチップは装飾であり操作対象ではない、モジュール doc 参照）。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        match self.anchor.as_ref() {
            Some(anchor) => anchor.hit_test(point, origin, size),
            None => false,
        }
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Tooltip
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::label::Label;

    struct FormState {
        hint: String,
    }

    fn get_hint(state: &FormState) -> String {
        state.hint.clone()
    }

    fn sized(width: f32, height: f32) -> Measurement {
        Measurement { width, height }
    }

    fn make_anchor() -> Label<FormState> {
        Label::fixed(WidgetId::from_raw(100), "保存")
    }

    // (a) 構築とアクセサ

    #[test]
    fn fixed_returns_text_immediately_without_sync() {
        let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(1), "保存します");

        assert_eq!(tooltip.text(), "保存します");
        assert_eq!(tooltip.id(), WidgetId::from_raw(1));
        assert!(!tooltip.is_visible());
    }

    #[test]
    fn bound_is_empty_before_first_sync() {
        let tooltip: Tooltip<FormState> = Tooltip::bound(WidgetId::from_raw(2), get_hint);

        assert_eq!(tooltip.text(), "");
        assert!(!tooltip.is_visible());
    }

    // (b) sync_from_state による cached_text 更新

    #[test]
    fn sync_from_state_keeps_static_text_unchanged() {
        let mut tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(3), "保存します");
        let state = FormState {
            hint: "無視される".to_string(),
        };

        tooltip.sync_from_state(&state);

        assert_eq!(tooltip.text(), "保存します");
    }

    #[test]
    fn sync_from_state_reflects_bound_text() {
        let mut tooltip: Tooltip<FormState> = Tooltip::bound(WidgetId::from_raw(4), get_hint);
        let state = FormState {
            hint: "入力してください".to_string(),
        };

        tooltip.sync_from_state(&state);

        assert_eq!(tooltip.text(), "入力してください");
    }

    #[test]
    fn sync_from_state_delegates_to_anchor() {
        let mut tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(5), "保存します")
            .with_anchor(Label::bound(WidgetId::from_raw(101), get_hint));
        let state = FormState {
            hint: "アンカー反映".to_string(),
        };

        tooltip.sync_from_state(&state);

        // アンカー（Label）の cached_text が更新されたことは paint 経由の
        // Text コマンドで観測する（Label は自身の text() アクセサを公開している
        // ため直接検証してもよいが、ここでは Widget 契約経由の観測に統一する）。
        let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));
        assert!(commands.contains(&PaintCommand::Text("アンカー反映".to_string())));
    }

    // (c) 既定非表示・show/hide によるバブル描画の切り替え

    #[test]
    fn default_is_hidden_and_paint_has_no_bubble_without_anchor() {
        let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(6), "保存します");

        let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));

        assert!(commands.is_empty());
    }

    #[test]
    fn show_appends_bubble_after_anchor_paint() {
        let mut tooltip: Tooltip<FormState> =
            Tooltip::fixed(WidgetId::from_raw(7), "保存します").with_anchor(make_anchor());

        tooltip.show();
        assert!(tooltip.is_visible());

        let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));

        // アンカー（Label）の描画コマンドに続けて [Rect, Text(本文)] が連結される。
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[1], PaintCommand::Rect);
        assert_eq!(commands[2], PaintCommand::Text("保存します".to_string()));
    }

    // (d) hide / disabled によるバブル描画の抑止

    #[test]
    fn hide_removes_bubble_from_paint() {
        let mut tooltip: Tooltip<FormState> =
            Tooltip::fixed(WidgetId::from_raw(8), "保存します").with_anchor(make_anchor());

        tooltip.show();
        tooltip.hide();

        let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));
        assert_eq!(commands.len(), 1);
        assert!(!commands.contains(&PaintCommand::Rect));
    }

    #[test]
    fn disabled_suppresses_bubble_even_when_visible() {
        let mut tooltip: Tooltip<FormState> =
            Tooltip::fixed(WidgetId::from_raw(9), "保存します").with_anchor(make_anchor());

        tooltip.show();
        tooltip.set_disabled(true);

        let commands = tooltip.paint((0.0, 0.0), sized(80.0, 20.0));
        assert_eq!(commands.len(), 1);
        assert!(!commands.contains(&PaintCommand::Rect));
    }

    // (e) measure: アンカー委譲・アンカーなし・非有限値の健全化

    #[test]
    fn measure_without_anchor_is_zero() {
        let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(10), "保存します");

        let measurement = tooltip.measure(sized(100.0, 40.0));

        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn measure_delegates_to_anchor() {
        let tooltip: Tooltip<FormState> =
            Tooltip::fixed(WidgetId::from_raw(11), "保存します").with_anchor(make_anchor());

        let measurement = tooltip.measure(sized(100.0, 40.0));
        let expected = make_anchor().measure(sized(100.0, 40.0));

        assert_eq!(measurement.width, expected.width);
        assert_eq!(measurement.height, expected.height);
    }

    #[test]
    fn measure_normalizes_non_finite_available_to_zero() {
        let tooltip: Tooltip<FormState> =
            Tooltip::fixed(WidgetId::from_raw(12), "保存します").with_anchor(make_anchor());

        let measurement = tooltip.measure(sized(f32::NAN, f32::INFINITY));
        // アンカー（Label）は幅のみ available へクランプし高さは常に近似行高を返す
        // 契約（`label` モジュール doc 参照）のため、非有限値を正規化した結果は
        // `available` を `(0.0, 0.0)` として anchor.measure に渡したときと一致する
        // ことで検証する（高さそのものをゼロと決め打たない）。
        let expected = make_anchor().measure(sized(0.0, 0.0));

        assert_eq!(measurement.width, expected.width);
        assert_eq!(measurement.height, expected.height);
    }

    // (f) hit_test: アンカー委譲・アンカーなしで false

    #[test]
    fn hit_test_without_anchor_is_false() {
        let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(13), "保存します");

        assert!(!tooltip.hit_test((5.0, 5.0), (0.0, 0.0), sized(80.0, 20.0)));
    }

    #[test]
    fn hit_test_delegates_to_anchor() {
        let tooltip: Tooltip<FormState> =
            Tooltip::fixed(WidgetId::from_raw(14), "保存します").with_anchor(make_anchor());
        let size = sized(80.0, 20.0);

        let expected = make_anchor().hit_test((5.0, 5.0), (0.0, 0.0), size);
        let actual = tooltip.hit_test((5.0, 5.0), (0.0, 0.0), size);

        assert_eq!(actual, expected);
    }

    // (g) accessibility_role・clone_box

    #[test]
    fn accessibility_role_is_tooltip() {
        let tooltip: Tooltip<FormState> = Tooltip::fixed(WidgetId::from_raw(15), "保存します");

        assert_eq!(tooltip.accessibility_role(), AccessibilityRole::Tooltip);
    }

    #[test]
    fn clone_box_produces_equivalent_widget() {
        let mut tooltip: Tooltip<FormState> =
            Tooltip::fixed(WidgetId::from_raw(16), "保存します").with_anchor(make_anchor());
        tooltip.show();

        let cloned = tooltip.clone_box();

        assert_eq!(cloned.id(), tooltip.id());
        assert_eq!(
            cloned.paint((0.0, 0.0), sized(80.0, 20.0)),
            tooltip.paint((0.0, 0.0), sized(80.0, 20.0))
        );
    }
}
