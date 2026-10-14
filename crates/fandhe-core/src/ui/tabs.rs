//! タブウィジェット `Tabs<S>`（TASK-8.3-S52 / REQ-8、Issue #147）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の分解
//! sub-issue（52/60）。タブの API 設計・基本描画を担い、直近の前例である
//! [`super::menu::Menu`]（TASK-8.3-S49 / #144）と
//! [`super::dialog::Dialog`]（TASK-8.3-S46 / #141）が確立した
//! 「選択インデックス束縛 + 子コンテンツ合成」のパターンを組み合わせて [`Tabs`] を
//! 新設する。
//!
//! - 制御型の状態バインディング（[`super::checkbox::Checkbox`] S19 / [`super::slider::Slider`]
//!   S28 / [`super::dialog::Dialog`] S46 / [`super::menu::Menu`] S49）: 選択中タブ
//!   index の SSOT はアプリ状態 `S`（`usize`）に置き、[`crate::state::ReadFn`] 経由の
//!   [`Widget::sync_from_state`] のみが表示キャッシュ `cached_selected` を更新する。
//!   範囲外 index は [`clamp_selected`] により安全側で `None` へフォールバックする
//!   （`menu::clamp_selected` / `radio_group::clamp_index` と同じ根拠、panic させない）。
//! - コンテナ型の子コンテンツ保持（[`super::dialog::Dialog::with_content`]）:
//!   各タブに任意の `content: Option<Box<dyn Widget<S>>>` を保持させ、
//!   `sync_from_state` は選択中タブの子だけでなく全タブの子へ委譲する
//!   （非選択タブも将来のアニメーション・プリフェッチ等で状態同期が必要になり得るため、
//!   `Dialog` の単一子委譲より広い契約を採る。子を持たないタブは委譲が no-op）。
//! - `measure` / `paint` / `hit_test` の 3 箇所は、タブ見出し 1 個の寸法
//!   [`Self::tab_size`]・タブバー全体の寸法 [`Self::bar_size`]・コンテンツ領域の寸法
//!   [`Self::content_area_size`] をプライベートヘルパーへ集約して共有する
//!   （`Dialog` が PR #386 Bugbot 指摘で確立し `Menu` が踏襲した「`measure` の確定寸法と
//!   `hit_test` 対象領域を必ず一致させる」整合性契約と同型）。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま。3 バックエンドの
//!   プレースホルダ契約への波及を避ける安全側判断、`ScrollView` / `Dialog` / `Menu` と
//!   同じ）。出力は `[Rect（タブバー背景）, Text(各タブ。選択中は装飾表記で判別可能),
//!   Rect（コンテンツ領域プレースホルダ）]` に続けて、選択中タブの `content.paint(..)`
//!   を連結する。
//! - `AccessibilityRole::TabList` を新設する（`accesskit::Role::TabList` へ対応予定）。
//!   個々のタブへの `accesskit::Role::Tab` 付与は子ウィジェット階層が前提のため
//!   スコープ外とする（下記スコープ外事項 (c) 参照）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。この契約を
//! 変更しない。検証は `Widget` メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) タブ切替イベント配線（クリック・キーボード操作の `WriteFn` 書き込み、
//! S53 / #148）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S54 / #149）、
//! (c) 個々のタブへの `AccessibilityRole::Tab` 付与（子ウィジェット階層が前提）、
//! (d) スクロール可能タブバー・アイコン付きタブ・閉じるボタン・縦置きタブ、
//! (e) ポップアップ/コンテンツの描画クリッピング・z-order（`PaintCommand` 拡張見送りの
//! ため。`ScrollView` / `Dialog` / `Menu` と同じ判断）、
//! (f) reconcile パス未配線の既知契約（`widget.rs` モジュール doc）は変更しない。

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, VisualState, Widget, WidgetId,
};
use crate::state::ReadFn;

/// 単一行の高さ近似（論理ピクセル相当）。[`super::menu::Menu`] / [`super::dialog::Dialog`] /
/// [`super::slider::Slider`] / [`super::checkbox::Checkbox`] の `APPROX_LINE_HEIGHT` と
/// 同じ値を採用し、測定近似の基準を揃える（cosmic-text 等の実テキスト計測エンジン
/// 導入前のプレースホルダ定数、折り返し・複数行はスコープ外）。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// 1 文字あたりの近似幅（論理ピクセル相当）。タブ見出しの表示文言の長さから
/// タブ幅を近似するためのプレースホルダ定数（cosmic-text 等の実テキスト計測
/// エンジン導入前の近似、`chars().count()` ベースで折り返し・全角/半角幅の
/// 違いは考慮しない既知の制約）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// タブ見出し・コンテンツ領域の周囲に確保する余白（論理ピクセル相当）。
const TABS_PADDING: f32 = 16.0;

/// 非有限値（NaN/inf）または負値を `0.0` へ丸める。
///
/// `available` の数値健全性確保のため（OWASP A04 相当）、[`super::menu`] /
/// [`super::dialog`] / [`super::scroll_view`] の同名ヘルパーと同じ丸め方針を採用する。
fn normalize_non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// 範囲外インデックスを安全側で `None` にクランプする。
///
/// `items.len()`（本モジュールでは `len` に `Tabs::items` の要素数を渡す）以上の
/// インデックスは無効値として扱い、panic させない（[`super::menu::clamp_selected`] /
/// [`super::radio_group::clamp_index`] と同じ根拠、`.claude/rules/coding-rust.md`
/// のエラーハンドリング方針）。
fn clamp_selected(selected: usize, len: usize) -> Option<usize> {
    if selected < len { Some(selected) } else { None }
}

/// 単一タブの見出し文言と任意のコンテンツを束ねる要素型。
///
/// `label` は表示専用の不透明文字列として保持し、解釈・実行しない（OWASP A03
/// 対策、`.claude/rules/security.md`）。`content` は [`Tabs::with_content`] が
/// 事後的に設定するビルダーパターン（[`Dialog::with_content`][super::dialog::Dialog::with_content]
/// と同型）を取るため、構築時点では常に `None` から始まる。
pub struct TabItem<S> {
    label: String,
    content: Option<Box<dyn Widget<S>>>,
}

impl<S> TabItem<S> {
    /// 見出し文言のみを持つタブ項目を作る（コンテンツは [`Tabs::with_content`] で後付けする）。
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            content: None,
        }
    }
}

impl<S> std::fmt::Debug for TabItem<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // label は UI 表示文字列でありユーザーの秘密情報に該当しないため、
        // `Menu` / `Dialog` と同様に `Debug` へそのまま含める。
        f.debug_struct("TabItem")
            .field("label", &self.label)
            .field("has_content", &self.content.is_some())
            .finish()
    }
}

impl<S> Clone for TabItem<S> {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            content: self.content.as_ref().map(|c| c.clone_box()),
        }
    }
}

/// タブウィジェット（TASK-8.3-S52、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（[`Menu`][super::menu::Menu] /
/// [`Dialog`][super::dialog::Dialog] と同じく、状態型 `S` を所有していないため
/// `derive` マクロが自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける。
/// `Clone` は各タブの `content` を [`Widget::clone_box`] 経由で複製する）。
pub struct Tabs<S> {
    id: WidgetId,
    /// タブ見出しと（任意の）コンテンツの一覧（構築時固定。状態束縛の動的項目リストは
    /// S53 以降のスコープ外事項、モジュール doc 参照）。
    items: Vec<TabItem<S>>,
    /// 選択中タブ index の読み取り経路（REQ-13、環境キャプチャ不可の関数ポインタに
    /// 限定）。選択中タブの SSOT は常に `S` 側に置き、本ウィジェットはキャッシュのみを
    /// 保持する（`Checkbox` / `Slider` / `Dialog` / `Menu` と同じ制御型の設計判断）。
    selected: ReadFn<S, usize>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ。範囲外 index は
    /// [`clamp_selected`] により安全側で `None` へフォールバック済み。
    cached_selected: Option<usize>,
    /// hover/pressed/disabled/focus。実際のタブ切替イベント配線は S53 のスコープ。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Tabs<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tabs")
            .field("id", &self.id)
            .field("items", &self.items)
            .field("cached_selected", &self.cached_selected)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Tabs<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            items: self.items.clone(),
            selected: self.selected,
            cached_selected: self.cached_selected,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Tabs<S> {
    /// 状態 `S` に束縛したタブを作る（ビルダー形式の起点）。
    ///
    /// `selected` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`]）と代入互換の
    /// 関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで拒否する）。
    /// 既定は各タブともコンテンツなし（[`Self::with_content`] で個別に追加可能）。
    pub fn bound(id: WidgetId, labels: Vec<String>, selected: ReadFn<S, usize>) -> Self {
        Self {
            id,
            items: labels.into_iter().map(TabItem::new).collect(),
            selected,
            cached_selected: None,
            interaction: InteractionState::default(),
        }
    }

    /// `index` 番目のタブへ子コンテンツを設定するビルダーメソッド
    /// （[`Dialog::with_content`][super::dialog::Dialog::with_content] と同型）。
    /// `index` が範囲外の場合は安全側で無視する（panic させない、構築時の呼び出し
    /// ミスをここで検知できない代わりに `.claude/rules/coding-rust.md` の
    /// エラーハンドリング方針を優先する）。
    pub fn with_content(mut self, index: usize, content: impl Widget<S> + 'static) -> Self {
        if let Some(item) = self.items.get_mut(index) {
            item.content = Some(Box::new(content));
        }
        self
    }

    /// タブ見出しの表示文言一覧を返す。
    pub fn labels(&self) -> Vec<&str> {
        self.items.iter().map(|item| item.label.as_str()).collect()
    }

    /// `cached_selected`（直近の [`Widget::sync_from_state`] が読み取った選択中タブ
    /// index。範囲外は [`clamp_selected`] により `None` へフォールバック済み）を返す。
    pub fn selected_index(&self) -> Option<usize> {
        self.cached_selected
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// 選択中タブの装飾表記付き表示テキスト（未選択の項目と区別できるよう、
    /// [`Menu::display_text`][super::menu::Menu] と同様に選択中であることを
    /// 文言側で表現する。専用の描画プリミティブを新設しない安全側の判断、
    /// モジュール doc 参照）を返す。
    fn tab_display_text(&self, index: usize, label: &str) -> String {
        if self.cached_selected == Some(index) {
            format!("[{label}]")
        } else {
            label.to_string()
        }
    }

    /// 単一タブ見出しの寸法を表示文言の長さから近似する（`measure` / `paint` /
    /// `hit_test` の 3 箇所が共有する単一の計算根拠）。
    fn tab_size(&self, label: &str, available: Measurement) -> Measurement {
        let width = label.chars().count() as f32 * APPROX_CHAR_WIDTH + TABS_PADDING;
        Measurement {
            width: width.min(available.width),
            height: (APPROX_LINE_HEIGHT + TABS_PADDING).min(available.height),
        }
    }

    /// タブバー全体（各タブ幅の合計）の寸法を `available` へクランプして返す
    /// （`measure` / `paint` / `hit_test` の 3 箇所が共有する単一の計算根拠。
    /// `Menu::popup_size` と同型の集約方針）。
    fn bar_size(&self, available: Measurement) -> Measurement {
        let total_width: f32 = self
            .items
            .iter()
            .map(|item| self.tab_size(&item.label, available).width)
            .sum();
        Measurement {
            width: total_width.min(available.width),
            height: (APPROX_LINE_HEIGHT + TABS_PADDING).min(available.height),
        }
    }

    /// タブバーの下に続くコンテンツ領域（プレースホルダ矩形）の寸法を `available`
    /// へクランプして返す（`measure` / `paint` / `hit_test` の 3 箇所が共有する
    /// 単一の計算根拠）。
    fn content_area_size(&self, available: Measurement) -> Measurement {
        Measurement {
            width: available.width,
            height: (APPROX_LINE_HEIGHT + TABS_PADDING).min(available.height),
        }
    }

    /// タブバー + コンテンツ領域を縦積みした合計寸法を `available` へクランプして
    /// 返す（[`Self::bar_size`] / [`Self::content_area_size`] を合成する、`measure` /
    /// `hit_test` が共有する。[`super::menu::Menu::open_total_size`] と同型の
    /// 「独立クランプ後に合計を再クランプする」設計 — `bar_size` / `content_area_size`
    /// はそれぞれ独立に `available` へクランプするため、高さの単純合計は
    /// `available.height` を超え得る）。
    fn total_size(&self, available: Measurement) -> Measurement {
        let bar = self.bar_size(available);
        let content_area = self.content_area_size(available);
        Measurement {
            width: bar.width.max(content_area.width),
            height: (bar.height + content_area.height).min(available.height),
        }
    }
}

impl<S: 'static> Widget<S> for Tabs<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から選択中タブ index を読み取り `cached_selected` を更新し
    /// （reconcile フェーズ）、全タブの子コンテンツへ `sync_from_state` を委譲する
    /// （[`Dialog::sync_from_state`][super::dialog::Dialog::sync_from_state] と同じ
    /// コンテナ委譲パターン。タブ自身は選択中 index 以外で `S` へ直接触れない）。
    /// 範囲外 index は [`clamp_selected`] により安全側で `None` へフォールバックする。
    fn sync_from_state(&mut self, state: &S) {
        self.cached_selected = clamp_selected((self.selected)(state), self.items.len());
        for item in &mut self.items {
            if let Some(content) = item.content.as_mut() {
                content.sync_from_state(state);
            }
        }
    }

    /// タブバー + コンテンツ領域の合計寸法を返す（[`Self::total_size`] 参照）。
    /// `hit_test` へ渡される確定寸法の根拠と一致させるための設計（`Dialog` が
    /// PR #386 Bugbot 指摘で修正された整合性契約と同型）。
    ///
    /// 非有限値・負値は安全側（`0.0`）へ丸める（OWASP A04 相当の数値健全性対策）。
    fn measure(&self, available: Measurement) -> Measurement {
        let available = Measurement {
            width: normalize_non_negative_finite(available.width),
            height: normalize_non_negative_finite(available.height),
        };

        self.total_size(available)
    }

    /// `[Rect（タブバー背景）, Text(各タブ。選択中は装飾表記で判別可能), Rect（コンテンツ
    /// 領域プレースホルダ）]` に続けて、選択中タブの `content.paint(..)` を連結する。
    /// `PaintCommand` は拡張しない（モジュール doc 参照）。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect];
        commands.extend(
            self.items
                .iter()
                .enumerate()
                .map(|(index, item)| PaintCommand::Text(self.tab_display_text(index, &item.label))),
        );
        commands.push(PaintCommand::Rect);

        if let Some(index) = self.cached_selected {
            if let Some(content) = self.items[index].content.as_ref() {
                // コンテンツはタブバー分を除いたコンテンツ領域の寸法でクランプする
                // （`content_area_size` 参照。バー分の拡大を子へ波及させない、
                // `Dialog::paint` が scrim 分を子へ伝播させない判断と同型）。
                let content_size = self.content_area_size(size);
                commands.extend(content.paint(origin, content_size));
            }
        }

        commands
    }

    /// タブバー + コンテンツ領域（縦積みの合計領域）を対象とする（S53 でのタブ
    /// クリックの布石）。対象領域は [`Self::total_size`] へ集約し、`measure` が
    /// 返す確定寸法と一致させる（モジュール doc の整合性契約、`Dialog` /
    /// `Menu` が PR #386 Bugbot 指摘で修正した設計と同型。呼び出し側から渡される
    /// `size` は `measure` の戻り値の使い回しを想定しない — レイアウトエンジンが
    /// ビューポート全域の `size` をそのまま渡すケースでも対象領域を正しく限定する）。
    /// `interaction.disabled` の間は常に `false` を返す（`checkbox::Checkbox` /
    /// `slider::Slider` / `dialog::Dialog` / `menu::Menu` と同じ、無効化された
    /// ウィジェットがポインタ入力に反応しない安全側の判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if self.interaction.disabled {
            return false;
        }

        let target_size = self.total_size(size);

        point.0 >= origin.0
            && point.0 < origin.0 + target_size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + target_size.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::TabList
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

    struct TabsState {
        selected_tab: usize,
    }

    fn get_selected_tab(state: &TabsState) -> usize {
        state.selected_tab
    }

    fn make_labels() -> Vec<String> {
        vec!["概要".to_string(), "詳細".to_string(), "履歴".to_string()]
    }

    fn make_tabs() -> Tabs<TabsState> {
        Tabs::bound(WidgetId::from_raw(1), make_labels(), get_selected_tab)
    }

    fn sized(width: f32, height: f32) -> Measurement {
        Measurement { width, height }
    }

    // (a) 構築直後の既定値

    #[test]
    fn bound_constructs_with_labels_and_no_selection_default() {
        let tabs = make_tabs();

        assert_eq!(tabs.labels(), vec!["概要", "詳細", "履歴"]);
        assert_eq!(tabs.selected_index(), None);
        assert_eq!(tabs.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn new_interaction_state_is_all_false() {
        let tabs = make_tabs();
        let interaction = tabs.interaction_state();

        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.disabled);
        assert!(!interaction.focused);
    }

    // (b) clamp_selected: 範囲内 / 範囲外 / 空 items

    #[test]
    fn clamp_selected_keeps_in_range_index() {
        assert_eq!(clamp_selected(1, 3), Some(1));
    }

    #[test]
    fn clamp_selected_falls_back_to_none_when_out_of_range() {
        assert_eq!(clamp_selected(99, 3), None);
    }

    #[test]
    fn clamp_selected_is_none_for_empty_items() {
        assert_eq!(clamp_selected(0, 0), None);
    }

    // (c) sync_from_state: cached_selected の更新・panic しないこと

    #[test]
    fn sync_from_state_updates_cached_selected() {
        let mut tabs = make_tabs();
        tabs.sync_from_state(&TabsState { selected_tab: 1 });
        assert_eq!(tabs.selected_index(), Some(1));
    }

    #[test]
    fn sync_from_state_does_not_panic_on_out_of_range_index() {
        let mut tabs = make_tabs();
        tabs.sync_from_state(&TabsState { selected_tab: 99 });
        assert_eq!(tabs.selected_index(), None);
    }

    #[test]
    fn sync_from_state_transitions_selection() {
        let mut tabs = make_tabs();
        let mut state = TabsState { selected_tab: 0 };

        tabs.sync_from_state(&state);
        assert_eq!(tabs.selected_index(), Some(0));

        state.selected_tab = 2;
        tabs.sync_from_state(&state);
        assert_eq!(tabs.selected_index(), Some(2));
    }

    // (d) measure: タブバー + コンテンツ領域の寸法・数値健全性

    #[test]
    fn measure_returns_bar_plus_content_area_height() {
        let tabs = make_tabs();
        let measurement = tabs.measure(sized(800.0, 600.0));

        let expected_row_height = APPROX_LINE_HEIGHT + TABS_PADDING;
        assert_eq!(measurement.height, expected_row_height * 2.0);
        assert!(measurement.width > 0.0);
    }

    #[test]
    fn measure_handles_non_finite_and_negative_available() {
        let tabs = make_tabs();

        let non_finite = tabs.measure(sized(f32::NAN, f32::INFINITY));
        assert!(non_finite.width.is_finite());
        assert!(non_finite.height.is_finite());
        assert_eq!(non_finite.width, 0.0);

        let negative = tabs.measure(sized(-10.0, -10.0));
        assert_eq!(negative.width, 0.0);
        assert_eq!(negative.height, 0.0);
    }

    #[test]
    fn measure_clamps_to_small_available_space() {
        let tabs = make_tabs();

        let measurement = tabs.measure(sized(10.0, 5.0));
        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    // (e) paint: [Rect, Text * N, Rect] 構成・選択中タブの装飾表記・
    // with_content 時の content.paint 連結

    #[test]
    fn paint_produces_bar_rect_labels_and_content_area_rect() {
        let mut tabs = make_tabs();
        tabs.sync_from_state(&TabsState { selected_tab: 0 });

        let commands = tabs.paint((0.0, 0.0), tabs.measure(sized(800.0, 600.0)));

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("[概要]".to_string()),
                PaintCommand::Text("詳細".to_string()),
                PaintCommand::Text("履歴".to_string()),
                PaintCommand::Rect,
            ]
        );
    }

    #[test]
    fn paint_appends_selected_tab_content() {
        let mut tabs = make_tabs().with_content(
            1,
            Label::bound(WidgetId::from_raw(2), |_state: &TabsState| {
                "詳細コンテンツ".to_string()
            }),
        );
        tabs.sync_from_state(&TabsState { selected_tab: 1 });

        let commands = tabs.paint((0.0, 0.0), tabs.measure(sized(800.0, 600.0)));

        assert_eq!(commands.len(), 6);
        assert_eq!(
            commands[5],
            PaintCommand::Text("詳細コンテンツ".to_string())
        );
    }

    #[test]
    fn paint_does_not_append_content_of_non_selected_tab() {
        let mut tabs = make_tabs().with_content(
            2,
            Label::bound(WidgetId::from_raw(3), |_state: &TabsState| {
                "履歴コンテンツ".to_string()
            }),
        );
        tabs.sync_from_state(&TabsState { selected_tab: 0 });

        let commands = tabs.paint((0.0, 0.0), tabs.measure(sized(800.0, 600.0)));

        assert_eq!(commands.len(), 5);
    }

    // (f) hit_test: measure が返す寸法と同一矩形で true、外側で false
    // （整合性契約の回帰テスト、`Dialog` / `Menu` の PR #386 Bugbot 対応と同型）

    #[test]
    fn measure_result_covers_full_range_in_hit_test() {
        let mut tabs = make_tabs();
        tabs.sync_from_state(&TabsState { selected_tab: 0 });

        let available = sized(800.0, 600.0);
        let measured = tabs.measure(available);

        assert!(tabs.hit_test((0.0, 0.0), (0.0, 0.0), measured));
        assert!(tabs.hit_test(
            (measured.width - 1.0, measured.height - 1.0),
            (0.0, 0.0),
            measured
        ));
        assert!(!tabs.hit_test((measured.width, measured.height), (0.0, 0.0), measured));
    }

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_inside() {
        let mut tabs = make_tabs();
        tabs.sync_from_state(&TabsState { selected_tab: 0 });
        tabs.set_disabled(true);

        let measured = tabs.measure(sized(800.0, 600.0));
        assert!(!tabs.hit_test((10.0, 10.0), (0.0, 0.0), measured));
    }

    #[test]
    fn set_disabled_transitions_visual_state() {
        let mut tabs = make_tabs();
        assert_eq!(tabs.visual_state(), VisualState::Idle);

        tabs.set_disabled(true);
        assert_eq!(tabs.visual_state(), VisualState::Disabled);
    }

    // (g) accessibility_role / clone_box / Debug / Send+Sync

    #[test]
    fn accessibility_role_is_tab_list() {
        let tabs = make_tabs();
        assert_eq!(tabs.accessibility_role(), AccessibilityRole::TabList);
    }

    #[test]
    fn clone_box_preserves_state_and_content_independently() {
        let mut tabs = make_tabs().with_content(
            0,
            Label::bound(WidgetId::from_raw(4), |state: &TabsState| {
                if state.selected_tab == 0 {
                    "A".to_string()
                } else {
                    "B".to_string()
                }
            }),
        );
        tabs.sync_from_state(&TabsState { selected_tab: 0 });

        let mut cloned = tabs.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        cloned.sync_from_state(&TabsState { selected_tab: 1 });

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(tabs.selected_index(), Some(0));
        let commands = tabs.paint((0.0, 0.0), tabs.measure(sized(800.0, 600.0)));
        assert_eq!(commands.last(), Some(&PaintCommand::Text("A".to_string())));
    }

    #[test]
    fn debug_output_does_not_panic() {
        let tabs = make_tabs();
        let output = format!("{tabs:?}");
        assert!(output.contains("Tabs"));
    }

    #[test]
    fn tabs_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Tabs<TabsState>>();
    }

    // (h) Box<dyn Widget<S>> への格納（object safety）

    #[test]
    fn tabs_can_be_boxed_as_dyn_widget() {
        let tabs: Box<dyn Widget<TabsState>> = Box::new(make_tabs());
        assert_eq!(tabs.accessibility_role(), AccessibilityRole::TabList);
    }

    // (i) Screen::tabs 糖衣構文

    #[test]
    fn screen_tabs_adds_widget_node() {
        use crate::ui::node::Node;
        use crate::ui::screen::Screen;

        let screen: Screen<TabsState> =
            Screen::new("タブ画面").tabs(WidgetId::from_raw(5), make_labels(), get_selected_tab);

        let mut found_widget = false;
        screen.visit(|node| {
            if matches!(node, Node::Widget(_)) {
                found_widget = true;
            }
        });
        assert!(found_widget);
    }
}
