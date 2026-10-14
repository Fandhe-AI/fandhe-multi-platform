//! スタック・グリッドレイアウトの状態・イベント処理（TASK-8.3-S44 / REQ-8、Issue #139）。
//!
//! [`super::text_field::TextFieldWidget`]（#109）・
//! [`super::scroll_view_widget::ScrollViewWidget`]（#136）と同じ判断（既存 main の
//! 公開 API のみに依存する自己完結実装とし、`stack_grid::Stack` / `stack_grid::Grid`
//! との型統合は別作業に委ねる）を踏襲し、[`Widget<S>`] を実装する自己完結の
//! コンテナ型 [`StackWidget`] / [`GridWidget`] として実装する。
//!
//! 型名・ファイル名は `stack_grid::Stack` / `stack_grid::Grid` との衝突を避けるため
//! `StackWidget` / `GridWidget` / `stack_grid_widget.rs` とする
//! （`scroll_view_widget::ScrollViewWidget` と同じ命名判断）。型統合は行わない
//! （`.claude/rules/out-of-scope-tracking.md` 対象）。
//!
//! ## 状態
//!
//! - **状態はウィジェットローカル**: 子リスト・軸・列数・ギャップ・フォーカス中の子
//!   インデックスはアプリ状態 `S` ではなくウィジェット内部状態として持つ
//!   （`ScrollViewWidget` と同じ判断。レイアウトコンテナはアプリ状態を変更しないため、
//!   `state::Event` に新バリアントは追加しない）。そのため [`Widget::sync_from_state`] は
//!   全子への委譲のみを行う（`S` 自体は読み書きしない）。
//! - **ギャップ・列数の防御的正規化**: ギャップは非有限値・負値を `0.0` へ正規化し
//!   （`ScrollViewWidget::set_viewport_extent` と同じ判断）、グリッドの列数は `1` 未満を
//!   `1` へクランプする（0 列グリッドは除算・添字計算で未定義動作になるため）。
//!
//! ## イベント処理（本 issue の核心その 2）
//!
//! - **意味づけ済み入力**: [`StackInput`]（`FocusNext`/`FocusPrev`/`FocusFirst`/`FocusLast`）・
//!   [`GridInput`]（`FocusUp`/`FocusDown`/`FocusLeft`/`FocusRight`/`FocusFirst`/`FocusLast`）
//!   のみで構成し、生キーコード・生文字列を一切持たない（`ScrollViewInput` と同じ
//!   OWASP A03 対策、`.claude/rules/security.md` 準拠）。
//! - **disabled 子のスキップ**: フォーカス走査は各子の [`Widget::interaction_state`] の
//!   `disabled` を参照し、無効な子を飛ばす。全子が disabled、または子が空の場合は
//!   何も変化させず `false` を返す。
//! - **端はクランプ（ラップしない）**: [`super::radio_group::RadioGroupWidget`]
//!   （`FocusNext`/`FocusPrev` がグループ内排他選択のためラップアラウンドする）とは
//!   異なり、本ウィジェットは「選択」ではなく「フォーカス走査」のみが責務であるため、
//!   末尾で `FocusNext`（グリッドは各方向）を適用しても先頭へ戻らず変化なし（`false`）を
//!   返す（一般的なツリー/グリッドウィジェットのキーボードナビゲーション慣行に従う）。
//! - **コンテナ disabled との一体性**: コンテナ自体の [`InteractionState::disabled`] の
//!   間は全入力を無視して `false` を返す。[`Self::set_disabled(true)`] はフォーカス
//!   インデックスも解除する（`ScrollViewWidget::set_disabled` が `drag` をクリアするのと
//!   同じ、stale 状態防止の判断）。
//! - **`handle_input` は `bool` を返す**: フォーカス走査はアプリ状態 `S` を変更しないため
//!   （`ScrollViewWidget::handle_input` と同じ設計判断）、フォーカスが変化したか
//!   （再描画要否判定用）のみを返す。
//! - **`apply_input`**: hover/press/focus の反映は [`InteractionState::apply`] へそのまま
//!   委譲する。
//! - **ポインタの子ルーティング**: [`StackWidget::child_rect`] / [`StackWidget::child_at`]
//!   （グリッドは [`GridWidget::child_rect`] / [`GridWidget::child_at`]）が、軸・列数・
//!   ギャップから子ごとの配置矩形を算出し、座標から子インデックスを解決する。
//!   半開区間規約（`[origin, origin + size)`）は [`super::checkbox::Checkbox::hit_test`] に
//!   揃え、ギャップ領域はどの子矩形にも含まれないため `None` を返す。実際のディスパッチは
//!   既存の [`super::pointer::PointerDispatcher`] / [`super::hit_test::hit_test`] 経路に
//!   委ね、本モジュールでは走査ロジックを二重管理しない。
//!
//! ## `Widget<S>` 実装
//!
//! - `sync_from_state`: 全子へ委譲する（`ScrollView` が確立予定のコンテナ委譲パターンを
//!   先取りする形。`&mut self` を要求するため、[`super::widget`] モジュール doc が指摘する
//!   「未配線の契約」（`render_headless` 等は `&Node<S>` の共有参照のみで走査する）は
//!   本ウィジェットにも適用される）。
//! - `measure`: スタックは主軸方向に子の計測値の合計 + ギャップ、交差軸は最大値
//!   （子は `available` に対して独立に計測され、複数子間のアラインメント・伸縮比は
//!   スコープ外、モジュール doc 参照）。グリッドは全子の最大幅・最大高を 1 セル寸法とし、
//!   `列数 × セル幅 + ギャップ` / `行数 × セル高 + ギャップ` を返す（等幅セルの最小実装）。
//! - `paint`: 各子の `paint` 結果を [`Self::child_rect`] が算出した位置でそのまま連結する。
//!   `PaintCommand` は既存の `Text` / `Rect` の 2 種から拡張しない（`ScrollViewWidget` と
//!   同じ判断、3 バックエンドのプレースホルダ契約へ波及するため）。
//! - `hit_test`: 自矩形内判定かつ `!disabled`（`ScrollViewWidget::hit_test` と同じ半開区間
//!   規約。子ごとのヒットテストは [`Self::child_at`] を経由する別経路とする）。
//! - `accessibility_role`: 本 issue で新設する [`AccessibilityRole::Group`] を返す
//!   （accesskit `Role::Group` へ対応予定。コンテナ型はいずれも汎用のグループロールを
//!   共用する）。
//! - `clone_box`: 子は各子の `clone_box` で深い複製を行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) S43（#138）マージ後の型統合、
//! (b) プラットフォームレイヤ（`fandhe-egui-proxy` / `fandhe-web-dom` /
//!     `fandhe-swiftui-bridge`）への実配線、
//! (c) `WidgetCatalog` 登録・結合テスト網羅拡充（S45 / #140）、
//! (d) アラインメント・伸縮比（flex-grow 相当）・セル結合（span）・RTL 対応、
//! (e) `ui::layout`（taffy ラッパ）との統合（既存の各コンテナウィジェットも自己完結
//!     measure の前例に従う）、
//! (f) reconcile パス未配線の既知契約（[`super::widget`] モジュール doc 参照）は
//!     変更しない。

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 非有限値（`NaN`/無限大）または負値を `0.0` へ丸める（ギャップの防御的正規化）。
///
/// [`super::scroll_view_widget`] の同名ロジックと同じ判断だが、モジュールをまたいだ
/// 非公開関数は再利用できないため、本モジュール独自のガードとして複製する。
fn normalize_non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

/// 集計値（累積オフセット・セル寸法等）を安全側の範囲へ丸める上限
/// （論理ピクセル相当のプレースホルダ定数）。[`super::stack_grid::MAX_CONTENT_EXTENT`]
/// と同じ安全側判断（`f32::MAX` は子側の乗算・加算でオーバーフローし `inf` が
/// 伝播しうるため、実用上十分大きい有限値に留める。OWASP A04 相当）。
/// `stack_grid` からは非公開のためモジュールごとに定数を持つ（値は同一）。
const MAX_CONTENT_EXTENT: f32 = 1_048_576.0;

/// `Widget::measure` の生の戻り値（幅・高さ）を安全側の範囲へ丸める。
///
/// 非有限値（`NaN`/無限大）または負値は `0.0` へ、極端に大きい有限値は
/// [`MAX_CONTENT_EXTENT`] へクランプする。子が異常値を返した場合にオフセット・
/// ヒットターゲットへ伝播するのを防ぐ（[`super::stack_grid::Stack`]/[`super::stack_grid::Grid`]
/// が採る正規化・クランプと同じ判断、`.claude/rules/security.md` の OWASP A04 相当）。
///
/// 本関数は「子から受け取った生の単一計測値」専用であり、無効値は「寄与なし（0.0）」
/// として扱う（[`measure_normalizes_non_finite_and_negative_child_measurements`] 等が
/// 前提とする挙動）。複数セルにまたがる**累積・乗算後の合計値**（列数・行数を掛けた
/// グリッド全体の幅・高さ等）には本関数を使わず [`clamp_total_extent`] を使うこと。
/// 前者は「異常値を無視する」判断だが、後者は「有効な値の合計がオーバーフローした」
/// 状況のため、`0.0` に丸めると計測値と描画・ヒットテスト位置が食い違う
/// （PR #385 レビュー指摘、`GridWidget::measure`/`GridWidget::child_rect` 不整合）。
fn clamp_extent(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value.min(MAX_CONTENT_EXTENT)
    } else {
        0.0
    }
}

/// 複数セル分の累積・乗算後の合計値（総幅・総高さ、セル位置オフセット等）を
/// 安全側の範囲へ丸める。[`clamp_extent`] とは非有限値の扱いが異なる点に注意
/// （本関数のドキュメンテーションコメント参照）。
///
/// 合計値の入力（セル寸法・ギャップ）はいずれも [`clamp_extent`] 済みの非負有限値
/// のため、本関数の非有限化は乗算オーバーフローによる `+inf` のみを想定する。
/// [`super::stack_grid::clamp_extent`] と同じ判断で、オーバーフロー時は `0.0` では
/// なく [`MAX_CONTENT_EXTENT`] に丸める（「サイズが無い」ではなく「安全側の上限に
/// 達した」ことを表すため）。負値（本来到達しない防御的分岐）は `0.0` にクランプする。
fn clamp_total_extent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_CONTENT_EXTENT)
    } else {
        MAX_CONTENT_EXTENT
    }
}

/// 子ウィジェットの列を、有効な（disabled ではない）子インデックスのみに絞って返す
/// （フォーカス走査の共通ヘルパー、[`StackWidget`] / [`GridWidget`] で共用する）。
fn enabled_indices<S>(children: &[Box<dyn Widget<S>>]) -> Vec<usize> {
    children
        .iter()
        .enumerate()
        .filter(|(_, child)| !child.interaction_state().disabled)
        .map(|(index, _)| index)
        .collect()
}

/// スタックの並び方向（主軸）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackAxis {
    /// 縦方向（主軸は高さ）に子を並べる。
    Vertical,
    /// 横方向（主軸は幅）に子を並べる。
    Horizontal,
}

/// [`StackWidget`] への意味づけ済み入力（本 issue の核心）。
///
/// 生キーコード・生文字列を一切持たない列挙のみで構成する（`ScrollViewInput` と同型の
/// OWASP A03 対策、`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackInput {
    /// 次の有効な（disabled ではない）子へフォーカスを移動する（末尾ではクランプ）。
    FocusNext,
    /// 前の有効な子へフォーカスを移動する（先頭ではクランプ）。
    FocusPrev,
    /// 先頭の有効な子へフォーカスを移動する。
    FocusFirst,
    /// 末尾の有効な子へフォーカスを移動する。
    FocusLast,
}

/// スタックレイアウトウィジェット（TASK-8.3-S44、`Widget<S>` 実装型、状態・イベント処理）。
///
/// `Debug` は手動実装する（`derive` マクロは構造体が持つ型パラメータ `S` に対して
/// 常に `S: Debug` 境界を追加するが、`Box<dyn Widget<S>>` は `Widget<S>: Debug`
/// スーパートレイト経由ですでに `Debug` を満たすため、この境界は不要かつ `S` に
/// `Debug` を要求しないアプリ状態型を扱えなくしてしまう。`ScrollViewWidget` の
/// `PhantomData` 対応とは事情が異なるが、同じ問題を回避する判断）。
pub struct StackWidget<S> {
    id: WidgetId,
    axis: StackAxis,
    gap: f32,
    children: Vec<Box<dyn Widget<S>>>,
    focused_index: Option<usize>,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for StackWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StackWidget")
            .field("id", &self.id)
            .field("axis", &self.axis)
            .field("gap", &self.gap)
            .field("children", &self.children)
            .field("focused_index", &self.focused_index)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for StackWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            axis: self.axis,
            gap: self.gap,
            children: self
                .children
                .iter()
                .map(|child| child.clone_box())
                .collect(),
            focused_index: self.focused_index,
            interaction: self.interaction,
        }
    }
}

impl<S> StackWidget<S> {
    /// 新規スタックを構築する（子は空・ギャップ `0.0` の初期状態）。
    pub fn new(id: WidgetId, axis: StackAxis) -> Self {
        Self {
            id,
            axis,
            gap: 0.0,
            children: Vec::new(),
            focused_index: None,
            interaction: InteractionState::default(),
        }
    }

    /// 子ウィジェットを末尾へ追加する（ビルダー形式）。
    pub fn with_child(mut self, child: Box<dyn Widget<S>>) -> Self {
        self.children.push(child);
        self
    }

    /// 子ウィジェット間のギャップを設定する（ビルダー形式）。
    ///
    /// 非有限値・負値は `0.0` へ正規化する（モジュール doc 参照）。
    pub fn with_gap(mut self, gap: f32) -> Self {
        self.set_gap(gap);
        self
    }

    /// 子ウィジェット間のギャップを設定する。
    pub fn set_gap(&mut self, gap: f32) {
        self.gap = normalize_non_negative_finite(gap);
    }

    /// 現在のギャップを返す。
    pub fn gap(&self) -> f32 {
        self.gap
    }

    /// 並び方向を返す。
    pub fn axis(&self) -> StackAxis {
        self.axis
    }

    /// 子の数を返す。
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// 現在フォーカス中の子インデックスを返す（フォーカスなしは `None`）。
    pub fn focused_index(&self) -> Option<usize> {
        self.focused_index
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// `true` にする際はフォーカスインデックスも解除する（[`super::scroll_view_widget`]
    /// の `drag` クリアと同じ、stale なフォーカス位置が再度 enable 時に残らないようにする
    /// 判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.focused_index = None;
        }
    }

    /// hover/press/focus の入力を反映する（[`InteractionState::apply`] への委譲）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);
    }

    /// [`StackInput`] を適用し、フォーカス位置が変化したかを返す（本 issue の核心 API）。
    ///
    /// コンテナ自体が disabled の間は全入力を無視して `false` を返す。子が空、または
    /// 全子が disabled の場合も `false` を返す。
    pub fn handle_input(&mut self, input: StackInput) -> bool {
        if self.interaction.disabled {
            return false;
        }
        let enabled = enabled_indices(&self.children);
        if enabled.is_empty() {
            return false;
        }

        let previous = self.focused_index;
        let current_position = self
            .focused_index
            .and_then(|index| enabled.iter().position(|&candidate| candidate == index));

        let next_position = match input {
            StackInput::FocusFirst => Some(0),
            StackInput::FocusLast => Some(enabled.len() - 1),
            StackInput::FocusNext => match current_position {
                None => Some(0),
                Some(position) if position + 1 < enabled.len() => Some(position + 1),
                Some(_) => None,
            },
            StackInput::FocusPrev => match current_position {
                None => Some(enabled.len() - 1),
                Some(position) if position > 0 => Some(position - 1),
                Some(_) => None,
            },
        };

        if let Some(position) = next_position {
            self.focused_index = Some(enabled[position]);
        }

        self.focused_index != previous
    }

    /// 子 `index` の配置矩形（原点・寸法）を算出する。
    ///
    /// `available` は各子の `measure` へそのまま渡す計測制約（本ウィジェットは
    /// アラインメント・伸縮比を持たない自己完結の最小実装のため、モジュール doc
    /// 「スコープ外事項 (d)」参照）。`index` が範囲外なら `None` を返す。
    pub fn child_rect(
        &self,
        index: usize,
        origin: (f32, f32),
        available: Measurement,
    ) -> Option<((f32, f32), Measurement)> {
        let target = self.children.get(index)?;
        let mut main_offset = 0.0_f32;
        for child in &self.children[..index] {
            let measurement = child.measure(available);
            let extent = clamp_extent(match self.axis {
                StackAxis::Vertical => measurement.height,
                StackAxis::Horizontal => measurement.width,
            });
            main_offset = clamp_extent(main_offset + extent + self.gap);
        }
        let measurement = target.measure(available);
        let size = Measurement {
            width: clamp_extent(measurement.width),
            height: clamp_extent(measurement.height),
        };
        let position = match self.axis {
            StackAxis::Vertical => (origin.0, origin.1 + main_offset),
            StackAxis::Horizontal => (origin.0 + main_offset, origin.1),
        };
        Some((position, size))
    }

    /// 座標 `point` に対応する子インデックスを解決する。
    ///
    /// 半開区間規約（`[origin, origin + size)`、モジュール doc 参照）。ギャップ領域は
    /// どの子矩形にも含まれないため `None` を返す。
    pub fn child_at(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        available: Measurement,
    ) -> Option<usize> {
        for index in 0..self.children.len() {
            let (child_origin, child_size) = self.child_rect(index, origin, available)?;
            if point.0 >= child_origin.0
                && point.0 < child_origin.0 + child_size.width
                && point.1 >= child_origin.1
                && point.1 < child_origin.1 + child_size.height
            {
                return Some(index);
            }
        }
        None
    }
}

impl<S: 'static> Widget<S> for StackWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// 全子への委譲のみを行う（モジュール doc §「状態はウィジェットローカル」参照。
    /// 本ウィジェット自体は `S` から読み取るべき表示値を持たない）。
    fn sync_from_state(&mut self, state: &S) {
        for child in &mut self.children {
            child.sync_from_state(state);
        }
    }

    /// 主軸方向に子の計測値の合計 + ギャップ、交差軸は最大値を返す
    /// （モジュール doc §「Widget<S> 実装」参照）。
    fn measure(&self, available: Measurement) -> Measurement {
        if self.children.is_empty() {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }
        let mut main_total = 0.0_f32;
        let mut cross_max = 0.0_f32;
        for (index, child) in self.children.iter().enumerate() {
            let measurement = child.measure(available);
            let (main, cross) = match self.axis {
                StackAxis::Vertical => (measurement.height, measurement.width),
                StackAxis::Horizontal => (measurement.width, measurement.height),
            };
            let main = clamp_extent(main);
            let cross = clamp_extent(cross);
            main_total = clamp_extent(main_total + main);
            if index + 1 < self.children.len() {
                main_total = clamp_extent(main_total + self.gap);
            }
            cross_max = cross_max.max(cross);
        }
        match self.axis {
            StackAxis::Vertical => Measurement {
                width: cross_max,
                height: main_total,
            },
            StackAxis::Horizontal => Measurement {
                width: main_total,
                height: cross_max,
            },
        }
    }

    /// 各子の `paint` 結果を [`Self::child_rect`] の位置でそのまま連結する
    /// （モジュール doc §「Widget<S> 実装」参照）。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        let mut commands = Vec::new();
        for index in 0..self.children.len() {
            if let Some((child_origin, child_size)) = self.child_rect(index, origin, size) {
                commands.extend(self.children[index].paint(child_origin, child_size));
            }
        }
        commands
    }

    /// 自矩形内判定かつ `!disabled`（`ScrollViewWidget::hit_test` と同じ半開区間規約）。
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
        AccessibilityRole::Group
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }
}

/// [`GridWidget`] への意味づけ済み入力（本 issue の核心）。
///
/// [`StackInput`] と同じく生キーコード・生文字列を持たない（OWASP A03 対策）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridInput {
    /// 1 行上の同じ列の有効な子へフォーカスを移動する（先頭行ではクランプ）。
    FocusUp,
    /// 1 行下の同じ列の有効な子へフォーカスを移動する（末尾行ではクランプ）。
    FocusDown,
    /// 同じ行の左隣の有効な子へフォーカスを移動する（行頭ではクランプ、次行へは移らない）。
    FocusLeft,
    /// 同じ行の右隣の有効な子へフォーカスを移動する（行末ではクランプ、次行へは移らない）。
    FocusRight,
    /// 先頭の有効な子へフォーカスを移動する。
    FocusFirst,
    /// 末尾の有効な子へフォーカスを移動する。
    FocusLast,
}

/// グリッドレイアウトウィジェット（TASK-8.3-S44、`Widget<S>` 実装型、状態・イベント処理）。
///
/// `Debug` は手動実装する（[`StackWidget`] と同じ理由、モジュール doc §
/// 「スタックレイアウトウィジェット」参照）。
pub struct GridWidget<S> {
    id: WidgetId,
    columns: usize,
    gap: f32,
    children: Vec<Box<dyn Widget<S>>>,
    focused_index: Option<usize>,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for GridWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GridWidget")
            .field("id", &self.id)
            .field("columns", &self.columns)
            .field("gap", &self.gap)
            .field("children", &self.children)
            .field("focused_index", &self.focused_index)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for GridWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            columns: self.columns,
            gap: self.gap,
            children: self
                .children
                .iter()
                .map(|child| child.clone_box())
                .collect(),
            focused_index: self.focused_index,
            interaction: self.interaction,
        }
    }
}

impl<S> GridWidget<S> {
    /// 新規グリッドを構築する（子は空・ギャップ `0.0` の初期状態）。
    ///
    /// `columns` が `0` の場合は `1` へクランプする（0 列グリッドは行・列算出が
    /// 未定義になるため、モジュール doc 参照）。
    pub fn new(id: WidgetId, columns: usize) -> Self {
        Self {
            id,
            columns: columns.max(1),
            gap: 0.0,
            children: Vec::new(),
            focused_index: None,
            interaction: InteractionState::default(),
        }
    }

    /// 子ウィジェットを末尾へ追加する（ビルダー形式）。
    pub fn with_child(mut self, child: Box<dyn Widget<S>>) -> Self {
        self.children.push(child);
        self
    }

    /// 子ウィジェット間のギャップを設定する（ビルダー形式）。
    pub fn with_gap(mut self, gap: f32) -> Self {
        self.set_gap(gap);
        self
    }

    /// 子ウィジェット間のギャップ（縦横共通）を設定する。
    pub fn set_gap(&mut self, gap: f32) {
        self.gap = normalize_non_negative_finite(gap);
    }

    /// 現在のギャップを返す。
    pub fn gap(&self) -> f32 {
        self.gap
    }

    /// 列数を設定する（`0` は `1` へクランプする、[`Self::new`] 参照）。
    pub fn set_columns(&mut self, columns: usize) {
        self.columns = columns.max(1);
    }

    /// 現在の列数を返す。
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// 子の数を返す。
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// 現在フォーカス中の子インデックスを返す（フォーカスなしは `None`）。
    pub fn focused_index(&self) -> Option<usize> {
        self.focused_index
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える（`true` の間はフォーカスインデックスも解除する、
    /// [`StackWidget::set_disabled`] と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.focused_index = None;
        }
    }

    /// hover/press/focus の入力を反映する（[`InteractionState::apply`] への委譲）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);
    }

    /// 有効な（disabled ではない）子かを判定する（内部ヘルパー）。
    fn is_enabled(&self, index: usize) -> bool {
        self.children
            .get(index)
            .is_some_and(|child| !child.interaction_state().disabled)
    }

    /// [`GridInput`] を適用し、フォーカス位置が変化したかを返す（本 issue の核心 API）。
    ///
    /// コンテナ自体が disabled の間は全入力を無視して `false` を返す。子が空、または
    /// 全子が disabled の場合も `false` を返す。方向移動（`FocusUp`/`FocusDown`/
    /// `FocusLeft`/`FocusRight`）は disabled な子を飛ばしつつ同一行・同一列を維持して
    /// 走査し、グリッド範囲を越える場合はクランプする（モジュール doc 参照）。
    pub fn handle_input(&mut self, input: GridInput) -> bool {
        if self.interaction.disabled {
            return false;
        }
        let enabled = enabled_indices(&self.children);
        if enabled.is_empty() {
            return false;
        }

        let previous = self.focused_index;
        let len = self.children.len();
        let columns = self.columns;

        let next = match input {
            GridInput::FocusFirst => Some(enabled[0]),
            GridInput::FocusLast => Some(enabled[enabled.len() - 1]),
            GridInput::FocusRight => self.focused_index.and_then(|current| {
                let row = current / columns;
                let mut col = current % columns;
                while col + 1 < columns {
                    col += 1;
                    let candidate = row * columns + col;
                    if candidate >= len {
                        break;
                    }
                    if self.is_enabled(candidate) {
                        return Some(candidate);
                    }
                }
                None
            }),
            GridInput::FocusLeft => self.focused_index.and_then(|current| {
                let row = current / columns;
                let mut col = current % columns;
                while col > 0 {
                    col -= 1;
                    let candidate = row * columns + col;
                    if self.is_enabled(candidate) {
                        return Some(candidate);
                    }
                }
                None
            }),
            GridInput::FocusDown => self.focused_index.and_then(|current| {
                let mut row = current / columns;
                let col = current % columns;
                loop {
                    row += 1;
                    let candidate = row * columns + col;
                    if candidate >= len {
                        break;
                    }
                    if self.is_enabled(candidate) {
                        return Some(candidate);
                    }
                }
                None
            }),
            GridInput::FocusUp => self.focused_index.and_then(|current| {
                let mut row = current / columns;
                let col = current % columns;
                while row > 0 {
                    row -= 1;
                    let candidate = row * columns + col;
                    if self.is_enabled(candidate) {
                        return Some(candidate);
                    }
                }
                None
            }),
        };

        // フォーカス未取得中の方向入力は、`FocusFirst` と同じく先頭の有効な子へ移動する
        // （`StackInput::FocusNext` が `current_position: None` を先頭扱いするのと同じ
        // 判断、モジュール doc §「イベント処理」参照）。
        let next = match (self.focused_index, next) {
            (None, _) if !matches!(input, GridInput::FocusFirst | GridInput::FocusLast) => {
                Some(enabled[0])
            }
            _ => next,
        };

        if let Some(candidate) = next {
            self.focused_index = Some(candidate);
        }

        self.focused_index != previous
    }

    /// 子 `index` の配置矩形（原点・寸法）を算出する（等幅セルの最小実装、
    /// モジュール doc §「Widget<S> 実装」参照）。
    ///
    /// セル位置オフセット（`col`/`row` × セル寸法 + ギャップ）は [`Self::measure`] の
    /// 総幅・総高さクランプと同じ [`clamp_total_extent`] を通す。クランプせず生の
    /// 乗算のみで配置すると、[`Self::measure`] が返すクランプ後の計測サイズと
    /// 実際の描画・ヒットテスト位置が食い違いうる（PR #385 レビュー指摘）。
    pub fn child_rect(
        &self,
        index: usize,
        origin: (f32, f32),
        available: Measurement,
    ) -> Option<((f32, f32), Measurement)> {
        if index >= self.children.len() {
            return None;
        }
        let (cell_width, cell_height) = self.cell_size(available);
        let row = (index / self.columns) as f32;
        let col = (index % self.columns) as f32;
        let x_step = clamp_total_extent(cell_width + self.gap);
        let y_step = clamp_total_extent(cell_height + self.gap);
        let position = (
            origin.0 + clamp_total_extent(col * x_step),
            origin.1 + clamp_total_extent(row * y_step),
        );
        Some((
            position,
            Measurement {
                width: cell_width,
                height: cell_height,
            },
        ))
    }

    /// 座標 `point` に対応する子インデックスを解決する（[`StackWidget::child_at`] と
    /// 同じ半開区間規約）。
    pub fn child_at(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        available: Measurement,
    ) -> Option<usize> {
        for index in 0..self.children.len() {
            let (child_origin, child_size) = self.child_rect(index, origin, available)?;
            if point.0 >= child_origin.0
                && point.0 < child_origin.0 + child_size.width
                && point.1 >= child_origin.1
                && point.1 < child_origin.1 + child_size.height
            {
                return Some(index);
            }
        }
        None
    }

    /// 全子の最大幅・最大高を 1 セル寸法として返す（内部ヘルパー）。
    fn cell_size(&self, available: Measurement) -> (f32, f32) {
        let mut width = 0.0_f32;
        let mut height = 0.0_f32;
        for child in &self.children {
            let measurement = child.measure(available);
            width = width.max(clamp_extent(measurement.width));
            height = height.max(clamp_extent(measurement.height));
        }
        (width, height)
    }

    /// 現在の子数・列数から求まる行数（`0` は子が空の場合のみ）。
    fn row_count(&self) -> usize {
        if self.children.is_empty() {
            0
        } else {
            self.children.len().div_ceil(self.columns)
        }
    }
}

impl<S: 'static> Widget<S> for GridWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// 全子への委譲のみを行う（[`StackWidget::sync_from_state`] と同じ判断）。
    fn sync_from_state(&mut self, state: &S) {
        for child in &mut self.children {
            child.sync_from_state(state);
        }
    }

    /// `列数 × セル幅 + ギャップ` / `行数 × セル高 + ギャップ` を返す
    /// （モジュール doc §「Widget<S> 実装」参照）。
    ///
    /// 総幅・総高さは複数セル分の累積・乗算後の値のため [`clamp_total_extent`] で
    /// クランプする（[`Self::child_rect`] のセル位置オフセットと同じ関数、
    /// オーバーフロー時は `0.0` ではなく [`MAX_CONTENT_EXTENT`] へ丸める）。
    fn measure(&self, available: Measurement) -> Measurement {
        if self.children.is_empty() {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }
        let (cell_width, cell_height) = self.cell_size(available);
        let columns = self.columns.min(self.children.len()).max(1);
        let rows = self.row_count();
        Measurement {
            width: clamp_total_extent(
                columns as f32 * cell_width + (columns.saturating_sub(1)) as f32 * self.gap,
            ),
            height: clamp_total_extent(
                rows as f32 * cell_height + (rows.saturating_sub(1)) as f32 * self.gap,
            ),
        }
    }

    /// 各子の `paint` 結果を [`Self::child_rect`] の位置でそのまま連結する。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        let mut commands = Vec::new();
        for index in 0..self.children.len() {
            if let Some((child_origin, child_size)) = self.child_rect(index, origin, size) {
                commands.extend(self.children[index].paint(child_origin, child_size));
            }
        }
        commands
    }

    /// 自矩形内判定かつ `!disabled`（[`StackWidget::hit_test`] と同じ半開区間規約）。
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
        AccessibilityRole::Group
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

    struct DummyState;

    /// テスト専用の最小 `Widget<S>` 実装（葉ノード相当）。
    ///
    /// `disabled` を構築時に固定できる以外は無害な no-op ウィジェットとし、
    /// `StackWidget` / `GridWidget` のフォーカス走査・子ルーティング・`sync_from_state`
    /// 委譲・`measure`/`paint` 連結を検証するための道具として使う。
    #[derive(Debug, Clone)]
    struct LeafWidget {
        id: WidgetId,
        size: Measurement,
        interaction: InteractionState,
        // `Widget<S>: Send + Sync` を満たす必要があるため、`Rc<Cell<_>>` ではなく
        // `Arc<AtomicU32>` を使う（テスト専用の呼び出し回数カウンタ）。
        sync_calls: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }

    impl LeafWidget {
        fn new(id: u64, width: f32, height: f32) -> Self {
            Self {
                id: WidgetId::from_raw(id),
                size: Measurement { width, height },
                interaction: InteractionState::default(),
                sync_calls: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }

        fn disabled(mut self) -> Self {
            self.interaction.set_disabled(true);
            self
        }
    }

    impl Widget<DummyState> for LeafWidget {
        fn id(&self) -> WidgetId {
            self.id
        }

        fn sync_from_state(&mut self, _state: &DummyState) {
            self.sync_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn measure(&self, _available: Measurement) -> Measurement {
            self.size
        }

        fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
            vec![PaintCommand::Rect]
        }

        fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
            !self.interaction.disabled
        }

        fn accessibility_role(&self) -> AccessibilityRole {
            AccessibilityRole::Label
        }

        fn interaction_state(&self) -> InteractionState {
            self.interaction
        }

        fn clone_box(&self) -> Box<dyn Widget<DummyState>> {
            Box::new(self.clone())
        }
    }

    fn available() -> Measurement {
        Measurement {
            width: 1000.0,
            height: 1000.0,
        }
    }

    // --- StackWidget ---

    fn stack_with_children(count: usize) -> StackWidget<DummyState> {
        let mut stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical);
        for index in 0..count {
            stack = stack.with_child(Box::new(LeafWidget::new(index as u64, 10.0, 20.0)));
        }
        stack
    }

    #[test]
    fn focus_next_moves_sequentially_and_clamps_at_end() {
        let mut stack = stack_with_children(3);
        assert!(stack.handle_input(StackInput::FocusNext));
        assert_eq!(stack.focused_index(), Some(0));
        assert!(stack.handle_input(StackInput::FocusNext));
        assert_eq!(stack.focused_index(), Some(1));
        assert!(stack.handle_input(StackInput::FocusNext));
        assert_eq!(stack.focused_index(), Some(2));

        // 末尾でのクランプ: これ以上進まず false を返す。
        let changed = stack.handle_input(StackInput::FocusNext);
        assert!(!changed);
        assert_eq!(stack.focused_index(), Some(2));
    }

    #[test]
    fn focus_prev_moves_backwards_and_clamps_at_start() {
        let mut stack = stack_with_children(3);
        stack.handle_input(StackInput::FocusLast);
        assert_eq!(stack.focused_index(), Some(2));

        assert!(stack.handle_input(StackInput::FocusPrev));
        assert_eq!(stack.focused_index(), Some(1));
        assert!(stack.handle_input(StackInput::FocusPrev));
        assert_eq!(stack.focused_index(), Some(0));

        let changed = stack.handle_input(StackInput::FocusPrev);
        assert!(!changed);
        assert_eq!(stack.focused_index(), Some(0));
    }

    #[test]
    fn focus_first_and_last() {
        let mut stack = stack_with_children(4);
        assert!(stack.handle_input(StackInput::FocusLast));
        assert_eq!(stack.focused_index(), Some(3));
        assert!(stack.handle_input(StackInput::FocusFirst));
        assert_eq!(stack.focused_index(), Some(0));
    }

    #[test]
    fn focus_next_skips_disabled_children() {
        let mut stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 10.0)))
            .with_child(Box::new(LeafWidget::new(1, 10.0, 10.0).disabled()))
            .with_child(Box::new(LeafWidget::new(2, 10.0, 10.0)));

        assert!(stack.handle_input(StackInput::FocusNext));
        assert_eq!(stack.focused_index(), Some(0));
        assert!(stack.handle_input(StackInput::FocusNext));
        // index 1 は disabled のため飛ばして index 2 へ。
        assert_eq!(stack.focused_index(), Some(2));
    }

    #[test]
    fn empty_stack_returns_false_for_any_input() {
        let mut stack: StackWidget<DummyState> =
            StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical);
        assert!(!stack.handle_input(StackInput::FocusNext));
        assert_eq!(stack.focused_index(), None);
    }

    #[test]
    fn all_disabled_children_returns_false() {
        let stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 10.0).disabled()))
            .with_child(Box::new(LeafWidget::new(1, 10.0, 10.0).disabled()));
        let mut stack = stack;
        assert!(!stack.handle_input(StackInput::FocusNext));
    }

    #[test]
    fn disabled_container_ignores_all_input() {
        let mut stack = stack_with_children(3);
        stack.set_disabled(true);
        assert!(!stack.handle_input(StackInput::FocusNext));
        assert_eq!(stack.focused_index(), None);
    }

    #[test]
    fn set_disabled_true_clears_focus_index() {
        let mut stack = stack_with_children(3);
        stack.handle_input(StackInput::FocusNext);
        assert_eq!(stack.focused_index(), Some(0));

        stack.set_disabled(true);
        assert_eq!(stack.focused_index(), None);

        stack.set_disabled(false);
        assert_eq!(stack.focused_index(), None);
    }

    #[test]
    fn set_gap_normalizes_non_finite_and_negative_values() {
        let mut stack: StackWidget<DummyState> =
            StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical);
        stack.set_gap(f32::NAN);
        assert_eq!(stack.gap(), 0.0);
        stack.set_gap(-5.0);
        assert_eq!(stack.gap(), 0.0);
        stack.set_gap(f32::INFINITY);
        assert_eq!(stack.gap(), 0.0);
        stack.set_gap(10.0);
        assert_eq!(stack.gap(), 10.0);
    }

    #[test]
    fn measure_vertical_sums_height_and_maxes_width() {
        let stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 20.0)))
            .with_child(Box::new(LeafWidget::new(1, 30.0, 40.0)));
        let measurement = stack.measure(available());
        assert_eq!(measurement.width, 30.0);
        assert_eq!(measurement.height, 20.0 + 5.0 + 40.0);
    }

    #[test]
    fn measure_horizontal_sums_width_and_maxes_height() {
        let stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Horizontal)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 20.0)))
            .with_child(Box::new(LeafWidget::new(1, 30.0, 40.0)));
        let measurement = stack.measure(available());
        assert_eq!(measurement.width, 10.0 + 5.0 + 30.0);
        assert_eq!(measurement.height, 40.0);
    }

    #[test]
    fn measure_empty_stack_is_zero() {
        let stack: StackWidget<DummyState> =
            StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical);
        let measurement = stack.measure(available());
        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn child_rect_places_children_sequentially_with_gap() {
        let stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 20.0)))
            .with_child(Box::new(LeafWidget::new(1, 10.0, 30.0)));

        let (origin0, size0) = stack.child_rect(0, (0.0, 0.0), available()).unwrap();
        assert_eq!(origin0, (0.0, 0.0));
        assert_eq!(size0.height, 20.0);

        let (origin1, size1) = stack.child_rect(1, (0.0, 0.0), available()).unwrap();
        assert_eq!(origin1, (0.0, 25.0));
        assert_eq!(size1.height, 30.0);

        assert!(stack.child_rect(2, (0.0, 0.0), available()).is_none());
    }

    #[test]
    fn child_at_resolves_index_and_none_in_gap() {
        let stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 20.0)))
            .with_child(Box::new(LeafWidget::new(1, 10.0, 30.0)));

        assert_eq!(stack.child_at((5.0, 5.0), (0.0, 0.0), available()), Some(0));
        assert_eq!(
            stack.child_at((5.0, 30.0), (0.0, 0.0), available()),
            Some(1)
        );
        // ギャップ領域（y: 20.0..25.0）はどの子にも属さない。
        assert_eq!(stack.child_at((5.0, 22.0), (0.0, 0.0), available()), None);
        // 領域外。
        assert_eq!(
            stack.child_at((500.0, 500.0), (0.0, 0.0), available()),
            None
        );
    }

    #[test]
    fn sync_from_state_delegates_to_all_children() {
        use std::sync::atomic::Ordering;

        let counter_a = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_b = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let mut child_a = LeafWidget::new(0, 10.0, 10.0);
        child_a.sync_calls = counter_a.clone();
        let mut child_b = LeafWidget::new(1, 10.0, 10.0);
        child_b.sync_calls = counter_b.clone();

        let mut stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_child(Box::new(child_a))
            .with_child(Box::new(child_b));

        stack.sync_from_state(&DummyState);
        assert_eq!(counter_a.load(Ordering::SeqCst), 1);
        assert_eq!(counter_b.load(Ordering::SeqCst), 1);
    }

    /// レビュー回帰テスト: 子 `Widget::measure` が `NaN`/`inf`/負値を返しても
    /// `measure` の集計値が有限・非負に留まることを検証する（`stack_grid::Stack`
    /// と同型の正規化・クランプ、モジュール doc §「ギャップ・列数の防御的正規化」参照）。
    #[test]
    fn measure_normalizes_non_finite_and_negative_child_measurements() {
        let stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, f32::NAN, f32::INFINITY)))
            .with_child(Box::new(LeafWidget::new(1, -10.0, -20.0)))
            .with_child(Box::new(LeafWidget::new(2, 30.0, 40.0)));

        let measurement = stack.measure(available());
        assert!(measurement.width.is_finite(), "width は常に有限であるべき");
        assert!(
            measurement.height.is_finite(),
            "height は常に有限であるべき"
        );
        assert!(measurement.width >= 0.0);
        assert!(measurement.height >= 0.0);
        // NaN/inf/負値は 0.0 に正規化されるため、有効な高さは子 2（40.0）とギャップ 2 個分のみ。
        assert_eq!(measurement.height, 40.0 + 5.0 + 5.0);
        assert_eq!(measurement.width, 30.0);
    }

    /// レビュー回帰テスト: `child_rect` の累積オフセット・寸法が子の異常な `measure`
    /// 戻り値に汚染されず、常に有限であることを検証する。
    #[test]
    fn child_rect_normalizes_non_finite_and_negative_child_measurements() {
        let stack = StackWidget::new(WidgetId::from_raw(1), StackAxis::Vertical)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, f32::NAN, f32::NAN)))
            .with_child(Box::new(LeafWidget::new(1, 10.0, -30.0)))
            .with_child(Box::new(LeafWidget::new(2, 10.0, 20.0)));

        for index in 0..3 {
            let (origin, size) = stack.child_rect(index, (0.0, 0.0), available()).unwrap();
            assert!(origin.0.is_finite() && origin.1.is_finite());
            assert!(size.width.is_finite() && size.width >= 0.0);
            assert!(size.height.is_finite() && size.height >= 0.0);
        }
        // 子 0 の height (NaN) と子 1 の height (-30.0) はどちらも 0.0 として累積される。
        let (origin2, _) = stack.child_rect(2, (0.0, 0.0), available()).unwrap();
        assert_eq!(origin2, (0.0, 0.0 + 5.0 + 5.0));
    }

    #[test]
    fn paint_concatenates_child_paint_results() {
        let stack = stack_with_children(3);
        let commands = stack.paint((0.0, 0.0), available());
        assert_eq!(commands, vec![PaintCommand::Rect; 3]);
    }

    #[test]
    fn hit_test_true_inside_false_outside_and_when_disabled() {
        let mut stack = stack_with_children(2);
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        assert!(stack.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!stack.hit_test((500.0, 500.0), (0.0, 0.0), size));

        stack.set_disabled(true);
        assert!(!stack.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn accessibility_role_is_group() {
        let stack = stack_with_children(1);
        assert_eq!(stack.accessibility_role(), AccessibilityRole::Group);
    }

    #[test]
    fn clone_box_preserves_focus_independently() {
        let mut stack = stack_with_children(3);
        stack.handle_input(StackInput::FocusNext);
        assert_eq!(stack.focused_index(), Some(0));

        let mut cloned = stack.clone_box();
        cloned.sync_from_state(&DummyState);

        assert_eq!(stack.focused_index(), Some(0));
    }

    #[test]
    fn stack_widget_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StackWidget<DummyState>>();
    }

    // --- GridWidget ---

    fn grid_with_children(count: usize, columns: usize) -> GridWidget<DummyState> {
        let mut grid = GridWidget::new(WidgetId::from_raw(1), columns);
        for index in 0..count {
            grid = grid.with_child(Box::new(LeafWidget::new(index as u64, 10.0, 10.0)));
        }
        grid
    }

    #[test]
    fn grid_columns_zero_is_clamped_to_one() {
        let grid: GridWidget<DummyState> = GridWidget::new(WidgetId::from_raw(1), 0);
        assert_eq!(grid.columns(), 1);
    }

    #[test]
    fn focus_right_and_left_move_within_row_and_clamp() {
        // 3x2 グリッド（columns = 3、6 個の子）。
        let mut grid = grid_with_children(6, 3);
        grid.handle_input(GridInput::FocusFirst);
        assert_eq!(grid.focused_index(), Some(0));

        assert!(grid.handle_input(GridInput::FocusRight));
        assert_eq!(grid.focused_index(), Some(1));
        assert!(grid.handle_input(GridInput::FocusRight));
        assert_eq!(grid.focused_index(), Some(2));

        // 行末でのクランプ（次行へは移らない）。
        let changed = grid.handle_input(GridInput::FocusRight);
        assert!(!changed);
        assert_eq!(grid.focused_index(), Some(2));

        assert!(grid.handle_input(GridInput::FocusLeft));
        assert_eq!(grid.focused_index(), Some(1));
    }

    #[test]
    fn focus_down_and_up_move_within_column_and_clamp() {
        // 3x2 グリッド（columns = 3、6 個の子）。index: 0 1 2 / 3 4 5
        let mut grid = grid_with_children(6, 3);
        grid.handle_input(GridInput::FocusFirst); // index 0

        assert!(grid.handle_input(GridInput::FocusDown));
        assert_eq!(grid.focused_index(), Some(3));

        // 末尾行でのクランプ。
        let changed = grid.handle_input(GridInput::FocusDown);
        assert!(!changed);
        assert_eq!(grid.focused_index(), Some(3));

        assert!(grid.handle_input(GridInput::FocusUp));
        assert_eq!(grid.focused_index(), Some(0));

        let changed = grid.handle_input(GridInput::FocusUp);
        assert!(!changed);
    }

    #[test]
    fn focus_down_skips_disabled_and_clamps_beyond_partial_last_row() {
        // 3 列・5 個の子（最終行は 2 個のみ）: index 0 1 2 / 3 4
        let grid = GridWidget::new(WidgetId::from_raw(1), 3)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 10.0)))
            .with_child(Box::new(LeafWidget::new(1, 10.0, 10.0)))
            .with_child(Box::new(LeafWidget::new(2, 10.0, 10.0).disabled()))
            .with_child(Box::new(LeafWidget::new(3, 10.0, 10.0)))
            .with_child(Box::new(LeafWidget::new(4, 10.0, 10.0)));
        let mut grid = grid;

        // index 2 列（col=2）の下は存在しないため FocusDown はクランプする。
        grid.handle_input(GridInput::FocusFirst);
        assert!(grid.handle_input(GridInput::FocusRight)); // -> 1
        // index 2 は disabled のため FocusRight はクランプ（1 のまま）。
        let changed = grid.handle_input(GridInput::FocusRight);
        assert!(!changed);
        assert_eq!(grid.focused_index(), Some(1));

        assert!(grid.handle_input(GridInput::FocusDown));
        assert_eq!(grid.focused_index(), Some(4));
    }

    #[test]
    fn grid_focus_first_and_last() {
        let mut grid = grid_with_children(5, 2);
        assert!(grid.handle_input(GridInput::FocusLast));
        assert_eq!(grid.focused_index(), Some(4));
        assert!(grid.handle_input(GridInput::FocusFirst));
        assert_eq!(grid.focused_index(), Some(0));
    }

    #[test]
    fn empty_grid_returns_false() {
        let mut grid: GridWidget<DummyState> = GridWidget::new(WidgetId::from_raw(1), 3);
        assert!(!grid.handle_input(GridInput::FocusFirst));
    }

    #[test]
    fn grid_disabled_container_ignores_all_input() {
        let mut grid = grid_with_children(4, 2);
        grid.set_disabled(true);
        assert!(!grid.handle_input(GridInput::FocusFirst));
        assert_eq!(grid.focused_index(), None);
    }

    #[test]
    fn grid_set_disabled_true_clears_focus_index() {
        let mut grid = grid_with_children(4, 2);
        grid.handle_input(GridInput::FocusFirst);
        assert_eq!(grid.focused_index(), Some(0));
        grid.set_disabled(true);
        assert_eq!(grid.focused_index(), None);
    }

    #[test]
    fn measure_computes_rows_and_columns_from_max_cell_size() {
        let grid = GridWidget::new(WidgetId::from_raw(1), 2)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 20.0)))
            .with_child(Box::new(LeafWidget::new(1, 30.0, 10.0)))
            .with_child(Box::new(LeafWidget::new(2, 15.0, 25.0)));
        // 3 個の子、2 列 → 2 行。セル寸法は最大値 (30.0, 25.0)。
        let measurement = grid.measure(available());
        assert_eq!(measurement.width, 2.0 * 30.0 + 5.0);
        assert_eq!(measurement.height, 2.0 * 25.0 + 5.0);
    }

    #[test]
    fn measure_empty_grid_is_zero() {
        let grid: GridWidget<DummyState> = GridWidget::new(WidgetId::from_raw(1), 3);
        let measurement = grid.measure(available());
        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn child_rect_and_child_at_use_row_column_layout() {
        let grid = GridWidget::new(WidgetId::from_raw(1), 2)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, 10.0, 10.0)))
            .with_child(Box::new(LeafWidget::new(1, 10.0, 10.0)))
            .with_child(Box::new(LeafWidget::new(2, 10.0, 10.0)));

        let (origin1, _) = grid.child_rect(1, (0.0, 0.0), available()).unwrap();
        assert_eq!(origin1, (15.0, 0.0));
        let (origin2, _) = grid.child_rect(2, (0.0, 0.0), available()).unwrap();
        assert_eq!(origin2, (0.0, 15.0));

        assert_eq!(grid.child_at((3.0, 3.0), (0.0, 0.0), available()), Some(0));
        assert_eq!(grid.child_at((18.0, 3.0), (0.0, 0.0), available()), Some(1));
        // ギャップ領域（x: 10.0..15.0）は None。
        assert_eq!(grid.child_at((12.0, 3.0), (0.0, 0.0), available()), None);
    }

    /// レビュー回帰テスト: `cell_size`（`measure`/`child_rect` の内部ヘルパー）が
    /// 子の `NaN`/`inf`/負値の `measure` 戻り値を正規化・クランプすることを検証する
    /// （`stack_grid::Grid` と同型の防御、[`StackWidget`] 側の同名テスト参照）。
    #[test]
    fn grid_measure_normalizes_non_finite_and_negative_child_measurements() {
        let grid = GridWidget::new(WidgetId::from_raw(1), 2)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, f32::NAN, f32::INFINITY)))
            .with_child(Box::new(LeafWidget::new(1, -10.0, -20.0)))
            .with_child(Box::new(LeafWidget::new(2, 30.0, 40.0)));

        let measurement = grid.measure(available());
        assert!(measurement.width.is_finite());
        assert!(measurement.height.is_finite());
        assert!(measurement.width >= 0.0);
        assert!(measurement.height >= 0.0);
        // NaN/inf/負値は 0.0 に正規化されるため、セル寸法の最大値は子 2（30.0, 40.0）。
        // 3 個の子・2 列 → 2 行。
        assert_eq!(measurement.width, 2.0 * 30.0 + 5.0);
        assert_eq!(measurement.height, 2.0 * 40.0 + 5.0);
    }

    /// レビュー回帰テスト: `child_rect` が返す寸法・原点が子の異常な `measure`
    /// 戻り値に汚染されず、常に有限であることを検証する。
    #[test]
    fn grid_child_rect_normalizes_non_finite_and_negative_child_measurements() {
        let grid = GridWidget::new(WidgetId::from_raw(1), 2)
            .with_gap(5.0)
            .with_child(Box::new(LeafWidget::new(0, f32::NAN, f32::NAN)))
            .with_child(Box::new(LeafWidget::new(1, -10.0, -10.0)))
            .with_child(Box::new(LeafWidget::new(2, 10.0, 10.0)));

        for index in 0..3 {
            let (origin, size) = grid.child_rect(index, (0.0, 0.0), available()).unwrap();
            assert!(origin.0.is_finite() && origin.1.is_finite());
            assert!(size.width.is_finite() && size.width >= 0.0);
            assert!(size.height.is_finite() && size.height >= 0.0);
        }
    }

    /// レビュー回帰テスト（PR #385 指摘: 「Grid measure clamp mismatches layout」）:
    /// セル寸法が [`MAX_CONTENT_EXTENT`] 相当まで大きく、列数を掛けた総幅が
    /// クランプ閾値を超える状況で、`measure` が返すクランプ後の総幅と `child_rect` が
    /// 返す各セルの位置が食い違わないこと（後者が前者を超えないこと）を検証する。
    /// 修正前は `child_rect` が未クランプの `col * (cell_width + gap)` 演算で位置を
    /// 算出していたため、`measure` の総幅（`MAX_CONTENT_EXTENT` にクランプ済み）を
    /// 大きく超える位置に子を配置し、計測サイズとヒットテスト位置が乖離していた。
    #[test]
    fn grid_measure_and_child_rect_stay_consistent_when_cell_totals_exceed_clamp() {
        // セル寸法は MAX_CONTENT_EXTENT ちょうど（子の測定値は clamp_extent を素通り
        // する境界値）。4 列分の合計は MAX_CONTENT_EXTENT を大きく超える。
        let columns = 4usize;
        let grid = GridWidget::new(WidgetId::from_raw(1), columns)
            .with_child(Box::new(LeafWidget::new(
                0,
                MAX_CONTENT_EXTENT,
                MAX_CONTENT_EXTENT,
            )))
            .with_child(Box::new(LeafWidget::new(
                1,
                MAX_CONTENT_EXTENT,
                MAX_CONTENT_EXTENT,
            )))
            .with_child(Box::new(LeafWidget::new(
                2,
                MAX_CONTENT_EXTENT,
                MAX_CONTENT_EXTENT,
            )))
            .with_child(Box::new(LeafWidget::new(
                3,
                MAX_CONTENT_EXTENT,
                MAX_CONTENT_EXTENT,
            )));

        let measurement = grid.measure(available());
        assert!(measurement.width.is_finite());
        assert_eq!(
            measurement.width, MAX_CONTENT_EXTENT,
            "4 列 × MAX_CONTENT_EXTENT はクランプ閾値を超えるため MAX_CONTENT_EXTENT に丸まる"
        );

        for index in 0..columns {
            let (origin, _) = grid.child_rect(index, (0.0, 0.0), available()).unwrap();
            assert!(origin.0.is_finite(), "origin は常に有限であるべき");
            assert!(
                origin.0 <= measurement.width,
                "child_rect の位置（列 {index}: {}）は measure が返す総幅（{}）を超えてはならない",
                origin.0,
                measurement.width
            );
        }
    }

    #[test]
    fn grid_sync_from_state_delegates_to_all_children() {
        use std::sync::atomic::Ordering;

        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let mut child = LeafWidget::new(0, 10.0, 10.0);
        child.sync_calls = counter.clone();
        let mut grid = GridWidget::new(WidgetId::from_raw(1), 2).with_child(Box::new(child));

        grid.sync_from_state(&DummyState);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn grid_paint_concatenates_child_paint_results() {
        let grid = grid_with_children(4, 2);
        let commands = grid.paint((0.0, 0.0), available());
        assert_eq!(commands, vec![PaintCommand::Rect; 4]);
    }

    #[test]
    fn grid_hit_test_true_inside_false_outside_and_when_disabled() {
        let mut grid = grid_with_children(4, 2);
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        assert!(grid.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!grid.hit_test((500.0, 500.0), (0.0, 0.0), size));

        grid.set_disabled(true);
        assert!(!grid.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn grid_accessibility_role_is_group() {
        let grid = grid_with_children(1, 2);
        assert_eq!(grid.accessibility_role(), AccessibilityRole::Group);
    }

    #[test]
    fn grid_clone_box_preserves_focus_independently() {
        let mut grid = grid_with_children(4, 2);
        grid.handle_input(GridInput::FocusFirst);
        let mut cloned = grid.clone_box();
        cloned.sync_from_state(&DummyState);
        assert_eq!(grid.focused_index(), Some(0));
    }

    #[test]
    fn grid_widget_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GridWidget<DummyState>>();
    }
}
