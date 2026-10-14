//! タブウィジェットの状態・イベント処理実装（TASK-8.3-S53 / REQ-8、Issue #148）。
//!
//! `RadioGroupWidget`（#118）と同じ判断（既存 main の公開 API のみに依存する
//! 自己完結実装とし、`tabs::Tabs` との型統合は後工程へ委ねる）を踏襲し、
//! [`Widget<S>`] を実装する最小のタブバー型 [`TabBarWidget`] として実装する
//! （`measure` / `paint` は暫定実装）。
//!
//! **命名の衝突回避**（`.claude/rules/out-of-scope-tracking.md` 隣接の設計判断）:
//! `tabs.rs` / `tabs::Tabs<S>` との file-add 衝突を避けるため、本ファイルは
//! `tab_bar.rs`、型名は `TabBarWidget<S>` / `TabBarInput` とする（`text_input.rs`/
//! `text_field.rs`、`scroll_view.rs`/`scroll_view_widget.rs` と同じ分離判断）。
//! `tabs::Tabs` との型統合は行わない。
//!
//! タブはラジオボタングループと同じ「**排他選択**」が本質のウィジェットのため、
//! `RadioGroupWidget`（`super::radio_group`）を最重要の設計参照とし、以下を本 issue の
//! 設計判断とする。
//!
//! - 選択中タブの SSOT はアプリ状態 `S`（`Option<usize>`）に置く（設計文書 §3 二層モデル）。
//!   ウィジェットは表示キャッシュ（`cached_selected`）のみを保持し、`&mut S` には触れない。
//! - 選択確定（ポインタ・キーボードいずれも）は既存の
//!   [`crate::state::Event::ButtonClicked`]（タブラベル）を返すのみとし、widget 層は
//!   `&mut S` に一切触れない（`RadioGroupWidget` と同じブリッジ設計。状態書き換えの
//!   単一ソースを `StateStore::dispatch` に保つ）。専用バリアント `Event::TabSelected` の
//!   新設・`WidgetId` ベースディスパッチ統合は本 issue のスコープ外とする（PR 本文に記録、
//!   `.claude/rules/out-of-scope-tracking.md` 準拠）。
//! - 範囲外インデックス（`labels.len()` 以上）は `None` へ安全側でクランプし、panic
//!   させない（`.claude/rules/coding-rust.md` のエラーハンドリング方針）。
//! - タブヘッダは横一列として扱い、[`TabBarWidget::tab_at`] で x 座標からヘッダ
//!   インデックスを解決する（ラベル幅 + 左右パディングの累積幅で行内位置を解決）。
//!   press したタブを `pressed_tab` で追跡し、**同一タブ上での release のみ**確定
//!   イベントを発火する（press → 領域外へ exit → release では発火しない）。確定時は
//!   `focused_index` も確定タブへ同期する（`RadioGroupWidget` の Bugbot 指摘
//!   「Pointer select skips focus update」の再発防止）。
//! - キーボード操作は生キーコード・文字列を扱わず、意味づけ済みの列挙 [`TabBarInput`]
//!   のみで構成する（`RadioGroupInput` / `widget::InteractionInput` と同じ設計判断で
//!   OWASP A03 対策を型レベルで担保する、`.claude/rules/security.md` 準拠）。
//!   `FocusNext` / `FocusPrev`（矢印キー相当）はタブ間の roving focus をラップアラウンドで
//!   移動し **selection-follows-focus**（WAI-ARIA Tabs の automatic activation。
//!   `RadioGroupWidget` と挙動を統一）する。`Select`（Space/Enter 相当）はフォーカス中
//!   タブの確定イベントを発火する。`focused`（タブバーがフォーカスを保持）かつ非
//!   disabled のときのみ受け付け、空のタブリストでは常に `None` を返す（panic させない）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: reconcile パス
//! （`&mut self` を `Node::Widget` へ届ける経路）は S3 以降も未確立であり、本モジュールの
//! `sync_from_state` は現時点でユニットテストからのみ駆動される。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! `Event::TabSelected` 専用バリアントの新設・`WidgetId` ベースディスパッチ統合、
//! S52（#147）マージ後の型統合、実描画確認（フォーカスリング等の視覚表現）。

use crate::state::{Event, ReadFn};

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数
/// （`RadioGroupWidget` / `Checkbox` と同じ判断）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// タブヘッダ 1 行の高さ近似（論理ピクセル相当）。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// タブ 1 個あたりの左右パディング合計（論理ピクセル相当）。
const TAB_PADDING: f32 = 16.0;

/// 選択済みタブを表す描画テキスト（`PaintCommand::Text` へ渡す簡易マーク）。
///
/// `PaintCommand` を拡張しない方針（3 バックエンドのプレースホルダ契約に波及するため
/// 安全側に倒す、`RadioGroupWidget` と同じ判断）のため、選択有無をテキスト内容の差分として
/// 表現する。
const SELECTED_MARK: &str = "\u{25C9}";

/// 未選択タブを表す描画テキスト。
const UNSELECTED_MARK: &str = "\u{25CB}";

/// 範囲外インデックスを安全側で `None` にクランプする。
///
/// `labels.len()` 以上のインデックスは無効値として扱い、panic させない
/// （`.claude/rules/coding-rust.md` のエラーハンドリング方針）。
fn clamp_index(index: Option<usize>, len: usize) -> Option<usize> {
    index.filter(|&i| i < len)
}

/// 各タブの表示幅（パディング込み）を返す。`tab_at` / `measure` / `paint` の
/// 幅計算で共有し、行内位置の解決とレイアウト計算を二重管理しない。
fn tab_width(label: &str) -> f32 {
    label.chars().count() as f32 * APPROX_CHAR_WIDTH + TAB_PADDING
}

/// タブバーへのキーボード入力を表す列挙（本 issue の核心）。
///
/// 外部由来の生キーコード・文字列を一切扱わず、意味づけ済みの列挙のみで構成する
/// ことで OWASP A03（インジェクション）対策を型レベルで担保する
/// （`RadioGroupInput` / `widget::InteractionInput` と同じ設計判断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarInput {
    /// 次のタブへ roving focus をラップアラウンドで移動する（矢印キー相当）。
    /// selection-follows-focus のため、移動先の選択イベントも即座に発火する。
    FocusNext,
    /// 前のタブへ roving focus をラップアラウンドで移動する（矢印キー相当）。
    /// `FocusNext` と同じく selection-follows-focus を伴う。
    FocusPrev,
    /// フォーカス中のタブを確定する（Space/Enter 相当）。
    Select,
}

/// タブバーウィジェット（TASK-8.3-S53 で新設する自己完結の実装）。
///
/// `Debug` / `Clone` は手動実装する（`RadioGroupWidget` / `Checkbox` と同じく、状態型 `S`
/// を所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。タブラベル・選択インデックスは機微情報に該当しないため、
/// `Debug` にそのまま含める（`RadioGroupWidget` と同じ判断）。
pub struct TabBarWidget<S> {
    id: WidgetId,
    /// タブラベル列。`Event::ButtonClicked { label }` へのブリッジに使うキーであり、
    /// 宣言層と共有する（`WidgetId` とは独立し、状態ディスパッチ側の識別に用いる、
    /// `RadioGroupWidget::options` と同じ設計）。
    labels: Vec<String>,
    /// アプリ状態 `S` から選択インデックスを読み取る関数ポインタ（`None` の場合は
    /// 静的な初期値のみを保持し、`sync_from_state` は no-op になる。REQ-13 の
    /// 関数ポインタ限定方針に従い、`ReadFn` のみを介して `S` に触れる）。
    get: Option<ReadFn<S, Option<usize>>>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`selected()` が返す値）。
    /// 範囲外インデックスは構築時・同期時の両方で `None` へクランプする。
    cached_selected: Option<usize>,
    /// roving focus の現在位置（タブバーが `focused` の間のみ意味を持つ）。
    /// `InteractionState::focused` とは直交する、タブバー固有の一時的 UI 状態。
    focused_index: Option<usize>,
    /// ポインタ press 中のタブインデックス（同一タブ上での release のみ確定イベントを
    /// 発火するための追跡状態、モジュール doc 参照）。
    pressed_tab: Option<usize>,
    /// hover/pressed(全体)/disabled/focus。個別タブの press 追跡は `pressed_tab` が
    /// 別途担う。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for TabBarWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabBarWidget")
            .field("id", &self.id)
            .field("labels", &self.labels)
            .field("cached_selected", &self.cached_selected)
            .field("focused_index", &self.focused_index)
            .field("pressed_tab", &self.pressed_tab)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for TabBarWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            labels: self.labels.clone(),
            get: self.get,
            cached_selected: self.cached_selected,
            focused_index: self.focused_index,
            pressed_tab: self.pressed_tab,
            interaction: self.interaction,
        }
    }
}

impl<S> TabBarWidget<S> {
    /// 静的なタブラベル・初期選択のみを持つタブバーを構築する
    /// （`sync_from_state` は no-op）。
    pub fn new_static(
        id: WidgetId,
        labels: impl IntoIterator<Item = impl Into<String>>,
        initial_selected: Option<usize>,
    ) -> Self {
        let labels: Vec<String> = labels.into_iter().map(Into::into).collect();
        let cached_selected = clamp_index(initial_selected, labels.len());
        Self {
            id,
            labels,
            get: None,
            cached_selected,
            focused_index: None,
            pressed_tab: None,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` から動的に選択インデックスを読み取るタブバーを構築する。
    pub fn new_dynamic(
        id: WidgetId,
        labels: impl IntoIterator<Item = impl Into<String>>,
        initial_selected: Option<usize>,
        get: ReadFn<S, Option<usize>>,
    ) -> Self {
        let labels: Vec<String> = labels.into_iter().map(Into::into).collect();
        let cached_selected = clamp_index(initial_selected, labels.len());
        Self {
            id,
            labels,
            get: Some(get),
            cached_selected,
            focused_index: None,
            pressed_tab: None,
            interaction: InteractionState::default(),
        }
    }

    /// タブラベル列を返す。
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// `cached_selected`（直近の [`Widget::sync_from_state`] が読み取った選択インデックス）
    /// を返す。
    pub fn selected(&self) -> Option<usize> {
        self.cached_selected
    }

    /// roving focus の現在位置を返す（タブバーが `focused` でない間も直近の位置を
    /// 保持する。次にフォーカスを得たときの復帰位置として使う）。
    pub fn focused_index(&self) -> Option<usize> {
        self.focused_index
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。`true` にした時点で press 追跡もクリアする
    /// （[`InteractionState::set_disabled`] が hover/pressed/focused を解除するのと
    /// 揃え、無効化されたタブバーが press 中の見た目のまま残る不整合を防ぐ）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.pressed_tab = None;
        }
    }

    /// 座標からヒットしたタブのヘッダインデックスを解決する（横一列の暫定近似）。
    ///
    /// ラベル幅 + 左右パディングの累積幅で行内位置を解決する。disabled 中・矩形外・
    /// タブ数を超える位置は `None` を返す（`RadioGroupWidget::option_at` と同じ半開
    /// 区間 `[origin, origin + size)` の意味論）。
    pub fn tab_at(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> Option<usize> {
        if self.interaction.disabled {
            return None;
        }
        if point.0 < origin.0 || point.0 >= origin.0 + size.width {
            return None;
        }
        if point.1 < origin.1 || point.1 >= origin.1 + size.height {
            return None;
        }
        let relative_x = point.0 - origin.0;
        let mut cursor = 0.0_f32;
        for (index, label) in self.labels.iter().enumerate() {
            let width = tab_width(label);
            if relative_x < cursor + width {
                return Some(index);
            }
            cursor += width;
        }
        None
    }

    /// ポインタ押下を記録する（本 issue の核心）。
    ///
    /// `tab_at` で解決したヘッダを `pressed_tab` として追跡し、この時点ではまだ確定
    /// イベントを発火しない（確定は [`Self::release`] が同一タブ上でのみ行う）。
    /// あわせて `InteractionState::apply(PointerPressed)` を適用し、[`Self::visual_state`]
    /// が `Pressed` を返せるようにする（`press`/`release` は `InteractionState` の press
    /// 遷移を自前で管理する契約とし、呼び出し側が `apply_input(PointerPressed)` を
    /// 二重に呼ぶ必要はない）。
    pub fn press(&mut self, point: (f32, f32), origin: (f32, f32), size: Measurement) {
        self.pressed_tab = self.tab_at(point, origin, size);
        self.interaction.apply(InteractionInput::PointerPressed);
    }

    /// ポインタ解放を処理し、press 時と同一タブ上での release のみ確定イベントを
    /// 発火する（本 issue の核心）。
    ///
    /// press → 領域外へ exit（[`Self::cancel_press`]）→ release という経路では
    /// `pressed_tab` が既に `None` のため発火しない。press したタブと異なるタブ上で
    /// release した場合も発火しない（ドラッグして離す操作の安全側の扱い）。あわせて
    /// `InteractionState::apply(PointerReleased)` を適用する（[`Self::press`] と対称、
    /// `Self::press` の doc 参照）。
    ///
    /// 確定イベントを発火する分岐では `focused_index` も確定したタブへ同期する
    /// （roving focus とポインタ確定の位置がずれると、直後の `FocusNext` /
    /// `FocusPrev` / `Select` がポインタ操作前のフォーカス位置から動作してしまうため。
    /// `RadioGroupWidget` の Bugbot 指摘「Pointer select skips focus update」対応）。
    pub fn release(
        &mut self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> Option<Event> {
        let released_at = self.tab_at(point, origin, size);
        let pressed = self.pressed_tab.take();
        self.interaction.apply(InteractionInput::PointerReleased);
        match (pressed, released_at) {
            (Some(p), Some(r)) if p == r => {
                self.focused_index = Some(r);
                Some(Event::ButtonClicked {
                    label: self.labels[r].clone(),
                })
            }
            _ => None,
        }
    }

    /// press 追跡をキャンセルする（ポインタが領域外へ出た場合の意味論。
    /// `InteractionState::apply` の `PointerExited` が hover/press を解除するのと揃え、
    /// タブバー固有の `pressed_tab` も同様にクリアする）。
    pub fn cancel_press(&mut self) {
        self.pressed_tab = None;
    }

    /// ポインタ出入り・フォーカス移動を適用する（`InteractionState::apply` への委譲）。
    ///
    /// `PointerExited` は `pressed_tab` も連動してクリアする（[`Self::cancel_press`]
    /// 参照）。`FocusGained` で `focused_index` が未設定の場合、現在の選択（なければ
    /// 先頭）を roving focus の初期位置とする（WAI-ARIA Tabs の標準的なフォーカス挙動）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);

        if matches!(input, InteractionInput::PointerExited) {
            self.cancel_press();
        }

        if matches!(input, InteractionInput::FocusGained)
            && self.interaction.focused
            && self.focused_index.is_none()
            && !self.labels.is_empty()
        {
            self.focused_index = Some(self.cached_selected.unwrap_or(0));
        }
    }

    /// キーボード操作を処理する（本 issue の核心、WAI-ARIA Tabs 準拠の最小実装）。
    ///
    /// `focused`（タブバーがフォーカス保持）かつ非 disabled のときのみ受け付ける。
    /// 空のタブリストでは常に `None` を返し panic させない。`FocusNext` / `FocusPrev` は
    /// ラップアラウンドで roving focus を移動し selection-follows-focus（移動先の選択
    /// イベントを即座に発火）する。
    pub fn handle_input(&mut self, input: TabBarInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused || self.labels.is_empty() {
            return None;
        }

        match input {
            TabBarInput::FocusNext => {
                let next = match self.focused_index {
                    Some(current) => (current + 1) % self.labels.len(),
                    None => 0,
                };
                self.focused_index = Some(next);
                Some(Event::ButtonClicked {
                    label: self.labels[next].clone(),
                })
            }
            TabBarInput::FocusPrev => {
                let prev = match self.focused_index {
                    Some(current) => (current + self.labels.len() - 1) % self.labels.len(),
                    None => self.labels.len() - 1,
                };
                self.focused_index = Some(prev);
                Some(Event::ButtonClicked {
                    label: self.labels[prev].clone(),
                })
            }
            TabBarInput::Select => {
                let index = self.focused_index?;
                Some(Event::ButtonClicked {
                    label: self.labels[index].clone(),
                })
            }
        }
    }
}

impl<S: 'static> Widget<S> for TabBarWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から選択インデックスを読み取り `cached_selected` を更新する
    /// （reconcile フェーズ）。範囲外インデックスは `None` へ安全側でクランプする
    /// （モジュール doc 参照）。未配線の契約はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        let Some(get) = self.get else {
            return;
        };
        let raw = get(state);
        self.cached_selected = clamp_index(raw, self.labels.len());
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ
    /// 実装、折り返し・複数行はスコープ外、`RadioGroupWidget::measure` と同じ判断）。
    ///
    /// 幅 = Σ(ラベル幅 + パディング) を `available.width` でクランプする。高さ = 1 行分
    /// （`APPROX_LINE_HEIGHT`）を `available.height` でクランプする。マルチバイトは
    /// `chars().count()` 基準の近似（既存ウィジェットと同じ既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let total_width: f32 = self.labels.iter().map(|label| tab_width(label)).sum();
        let width = total_width.min(available.width);
        let height = APPROX_LINE_HEIGHT.min(available.height);

        Measurement { width, height }
    }

    /// タブごとに `[Rect（ヘッダ枠）, Text(選択マーク), Text(label)]` の描画プリミティブ列を
    /// 並べて返す。
    ///
    /// ヘッドレス環境でも選択状態がコマンド列の差分として検証できる構成とし、
    /// `PaintCommand` 自体は拡張しない（`RadioGroupWidget` と同じ判断、モジュール doc
    /// 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = Vec::with_capacity(self.labels.len() * 3);
        for (index, label) in self.labels.iter().enumerate() {
            commands.push(PaintCommand::Rect);
            let mark = if self.cached_selected == Some(index) {
                SELECTED_MARK
            } else {
                UNSELECTED_MARK
            };
            commands.push(PaintCommand::Text(mark.to_string()));
            commands.push(PaintCommand::Text(label.clone()));
        }
        commands
    }

    /// タブバー全体の矩形包含判定（AABB）。`interaction.disabled` の間は `false` を
    /// 返し、無効化されたタブバーを入力対象にしない（`RadioGroupWidget::hit_test` と
    /// 同じ設計判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、左上を含み
    /// 右下を含まない半開区間（`[origin, origin + size)`）で判定する。個別タブの行内
    /// 位置解決は [`Self::tab_at`] が別途担う。
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
    use crate::state::StateStore;
    use crate::ui::Screen;

    struct TabState {
        active: Option<usize>,
    }

    fn read_active(state: &TabState) -> Option<usize> {
        state.active
    }

    fn full_size() -> Measurement {
        Measurement {
            width: 200.0,
            height: 40.0,
        }
    }

    fn focus(bar: &mut TabBarWidget<TabState>) {
        bar.apply_input(InteractionInput::FocusGained);
    }

    // (a) 構築・アクセサ

    #[test]
    fn new_static_constructs_with_labels_and_initial_selection() {
        let bar: TabBarWidget<TabState> = TabBarWidget::new_static(
            WidgetId::from_raw(1),
            ["home", "search", "profile"],
            Some(1),
        );

        assert_eq!(bar.labels(), &["home", "search", "profile"]);
        assert_eq!(bar.selected(), Some(1));
        assert_eq!(bar.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn new_static_clamps_out_of_range_initial_selection_to_none() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search"], Some(9));

        assert_eq!(bar.selected(), None);
    }

    #[test]
    fn new_dynamic_constructs_with_clamped_initial_selection() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_dynamic(WidgetId::from_raw(1), ["home", "search"], None, read_active);

        assert_eq!(bar.selected(), None);
    }

    // (b) sync_from_state

    #[test]
    fn sync_from_state_static_bar_is_no_op() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search"], Some(0));
        let state = TabState { active: Some(1) };

        bar.sync_from_state(&state);

        assert_eq!(bar.selected(), Some(0));
    }

    #[test]
    fn sync_from_state_dynamic_bar_tracks_state_changes() {
        let mut bar: TabBarWidget<TabState> = TabBarWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["home", "search", "profile"],
            None,
            read_active,
        );
        let mut state = TabState { active: Some(2) };

        bar.sync_from_state(&state);
        assert_eq!(bar.selected(), Some(2));

        state.active = None;
        bar.sync_from_state(&state);
        assert_eq!(bar.selected(), None);
    }

    #[test]
    fn sync_from_state_clamps_out_of_range_index_to_none() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_dynamic(WidgetId::from_raw(1), ["home", "search"], None, read_active);
        let state = TabState { active: Some(42) };

        bar.sync_from_state(&state);

        assert_eq!(bar.selected(), None);
    }

    // (c) ポインタ確定

    #[test]
    fn press_then_release_on_same_tab_fires_event() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        let size = full_size();
        // "home" 幅 = 4*8 + 16 = 48。"search" のヘッダは x=48〜48+(6*8+16)=112 の範囲。
        let point = (60.0, 5.0);

        bar.press(point, (0.0, 0.0), size);
        let event = bar.release(point, (0.0, 0.0), size);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "search".to_string(),
            })
        );
    }

    #[test]
    fn release_on_different_tab_does_not_fire() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        let size = full_size();

        bar.press((5.0, 5.0), (0.0, 0.0), size);
        let event = bar.release((60.0, 5.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn drag_away_then_release_does_not_fire() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        let size = full_size();

        bar.press((5.0, 5.0), (0.0, 0.0), size);
        bar.apply_input(InteractionInput::PointerExited);
        let event = bar.release((5.0, 5.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn press_then_release_on_same_tab_updates_focused_index() {
        // RadioGroupWidget の Bugbot 指摘「Pointer select skips focus update」の
        // 回帰テスト。ポインタでタブを確定した後、roving focus がそのタブへ追従して
        // いないと直後の FocusNext/FocusPrev/Select がポインタ操作前の位置から動作
        // してしまう。
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        let size = full_size();
        // "home"(4*8+16=48) + "search"(6*8+16=64) = 112 起点の "profile" ヘッダ内。
        let point = (120.0, 5.0);

        bar.press(point, (0.0, 0.0), size);
        let event = bar.release(point, (0.0, 0.0), size);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "profile".to_string(),
            })
        );
        assert_eq!(bar.focused_index(), Some(2));
    }

    #[test]
    fn press_sets_pressed_visual_state_and_release_clears_it() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        let size = full_size();

        bar.press((5.0, 5.0), (0.0, 0.0), size);
        assert_eq!(bar.visual_state(), VisualState::Pressed);

        bar.release((5.0, 5.0), (0.0, 0.0), size);
        assert_eq!(bar.visual_state(), VisualState::Idle);
    }

    #[test]
    fn disabled_bar_ignores_press_and_release() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        bar.set_disabled(true);
        let size = full_size();

        bar.press((5.0, 5.0), (0.0, 0.0), size);
        let event = bar.release((5.0, 5.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn tab_at_resolves_header_by_horizontal_position() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        let size = full_size();

        // "home"(4*8+16=48) + "search"(6*8+16=64) = 112 起点の "profile" ヘッダ。
        assert_eq!(bar.tab_at((5.0, 5.0), (0.0, 0.0), size), Some(0));
        assert_eq!(bar.tab_at((60.0, 5.0), (0.0, 0.0), size), Some(1));
        assert_eq!(bar.tab_at((120.0, 5.0), (0.0, 0.0), size), Some(2));
        assert_eq!(bar.tab_at((199.0, 5.0), (0.0, 0.0), size), None);
    }

    #[test]
    fn tab_at_boundary_is_exclusive_of_next_header() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search"], None);
        let size = full_size();
        // "home" 幅ちょうど 48.0 は次ヘッダ（"search"）の開始位置。
        assert_eq!(bar.tab_at((48.0, 5.0), (0.0, 0.0), size), Some(1));
        assert_eq!(bar.tab_at((47.9, 5.0), (0.0, 0.0), size), Some(0));
    }

    // (d) キーボード

    #[test]
    fn focus_next_wraps_around_and_selects() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        focus(&mut bar);
        bar.handle_input(TabBarInput::FocusNext);
        bar.handle_input(TabBarInput::FocusNext);

        let event = bar.handle_input(TabBarInput::FocusNext);

        assert_eq!(bar.focused_index(), Some(0));
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "home".to_string(),
            })
        );
    }

    #[test]
    fn focus_prev_wraps_around_and_selects() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        focus(&mut bar);

        let event = bar.handle_input(TabBarInput::FocusPrev);

        assert_eq!(bar.focused_index(), Some(2));
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "profile".to_string(),
            })
        );
    }

    #[test]
    fn select_fires_event_for_focused_tab() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        focus(&mut bar);
        // focus() は未選択（cached_selected == None）のため roving focus を index 0
        // （"home"）から開始する。FocusNext で index 1（"search"）へ移動する。
        bar.handle_input(TabBarInput::FocusNext);

        let event = bar.handle_input(TabBarInput::Select);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "search".to_string(),
            })
        );
    }

    #[test]
    fn keyboard_input_ignored_when_not_focused() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);

        let event = bar.handle_input(TabBarInput::FocusNext);

        assert_eq!(event, None);
        assert_eq!(bar.focused_index(), None);
    }

    #[test]
    fn keyboard_input_ignored_when_disabled() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search", "profile"], None);
        focus(&mut bar);
        bar.set_disabled(true);

        let event = bar.handle_input(TabBarInput::Select);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_input_on_empty_tabs_is_safe_none() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), Vec::<String>::new(), None);
        focus(&mut bar);

        assert_eq!(bar.handle_input(TabBarInput::FocusNext), None);
        assert_eq!(bar.handle_input(TabBarInput::FocusPrev), None);
        assert_eq!(bar.handle_input(TabBarInput::Select), None);
    }

    #[test]
    fn focus_gained_defaults_roving_focus_to_current_selection() {
        let mut bar: TabBarWidget<TabState> = TabBarWidget::new_static(
            WidgetId::from_raw(1),
            ["home", "search", "profile"],
            Some(2),
        );

        focus(&mut bar);

        assert_eq!(bar.focused_index(), Some(2));
    }

    // (e) 排他性の StateStore 結合往復

    #[test]
    fn state_store_dispatch_round_trip_enforces_exclusive_selection() {
        fn select_home(state: &mut TabState) {
            state.active = Some(0);
        }
        fn select_search(state: &mut TabState) {
            state.active = Some(1);
        }
        fn select_profile(state: &mut TabState) {
            state.active = Some(2);
        }

        let screen: Screen<TabState> = Screen::new("tabs")
            .button(WidgetId::from_raw(101), "home", select_home)
            .button(WidgetId::from_raw(102), "search", select_search)
            .button(WidgetId::from_raw(103), "profile", select_profile);
        let mut store = StateStore::new(TabState { active: Some(0) });
        let mut bar: TabBarWidget<TabState> = TabBarWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["home", "search", "profile"],
            None,
            read_active,
        );
        bar.sync_from_state(store.state());
        assert_eq!(bar.selected(), Some(0));

        let size = full_size();
        let point = (60.0, 5.0);
        bar.press(point, (0.0, 0.0), size);
        let event = bar
            .release(point, (0.0, 0.0), size)
            .expect("search タブの press/release は Some を返す");
        let handled = store.dispatch(&screen, &event);
        assert!(handled);

        bar.sync_from_state(store.state());

        assert_eq!(store.state().active, Some(1));
        assert_eq!(bar.selected(), Some(1));
    }

    // (f) hit_test / Debug / clone_box / measure

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search"], None);

        assert!(bar.hit_test((5.0, 5.0), (0.0, 0.0), full_size()));
    }

    #[test]
    fn hit_test_returns_false_on_bottom_right_edge() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search"], None);
        let size = full_size();

        assert!(!bar.hit_test((size.width, size.height), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_returns_false_when_disabled() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search"], None);
        bar.set_disabled(true);

        assert!(!bar.hit_test((5.0, 5.0), (0.0, 0.0), full_size()));
    }

    #[test]
    fn accessibility_role_is_tab_list() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["home", "search"], None);

        assert_eq!(bar.accessibility_role(), AccessibilityRole::TabList);
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut bar: TabBarWidget<TabState> = TabBarWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["home", "search"],
            Some(0),
            read_active,
        );
        let state = TabState { active: Some(1) };
        bar.sync_from_state(&state);

        let mut cloned = bar.clone_box();
        let other_state = TabState { active: Some(0) };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(bar.selected(), Some(1));
        let cloned_commands = cloned.paint((0.0, 0.0), full_size());
        assert!(cloned_commands.contains(&PaintCommand::Text(UNSELECTED_MARK.to_string())));
    }

    #[test]
    fn debug_output_contains_cached_selected_and_labels() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_dynamic(WidgetId::from_raw(1), ["home", "search"], None, read_active);
        let state = TabState { active: Some(1) };
        bar.sync_from_state(&state);

        let output = format!("{bar:?}");

        assert!(output.contains("cached_selected"));
        assert!(output.contains("Some(1)"));
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let bar: TabBarWidget<TabState> =
            TabBarWidget::new_static(WidgetId::from_raw(1), ["あ"], None);

        let measurement = bar.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = 1.0 * APPROX_CHAR_WIDTH + TAB_PADDING;
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, APPROX_LINE_HEIGHT);
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let bar: TabBarWidget<TabState> = TabBarWidget::new_static(
            WidgetId::from_raw(1),
            ["非常に長いラベルです非常に長いラベルです", "短い"],
            None,
        );

        let measurement = bar.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn paint_marks_selected_tab_and_lists_all_labels() {
        let mut bar: TabBarWidget<TabState> =
            TabBarWidget::new_dynamic(WidgetId::from_raw(1), ["home", "search"], None, read_active);
        let state = TabState { active: Some(1) };
        bar.sync_from_state(&state);

        let commands = bar.paint((0.0, 0.0), full_size());

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text(UNSELECTED_MARK.to_string()),
                PaintCommand::Text("home".to_string()),
                PaintCommand::Rect,
                PaintCommand::Text(SELECTED_MARK.to_string()),
                PaintCommand::Text("search".to_string()),
            ]
        );
    }
}
