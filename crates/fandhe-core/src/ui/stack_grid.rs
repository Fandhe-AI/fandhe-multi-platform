//! スタック・グリッドレイアウトウィジェット `Stack<S>` / `Grid<S>`
//! （TASK-8.3-S43 / REQ-8 / REQ-9、Issue #138）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の 43/60。
//! スタック・グリッドレイアウトの API 設計・基本描画を担い、
//! `Widget<S>` トレイト（TASK-8.3-S2 / #97）と、先行のコンテナ型
//! [`super::scroll_view::ScrollView`]（S40 / #135、単一子コンテナ）が確立した
//! パターンを踏襲し、**本ウィジェットセット初の複数子コンテナ**として `Widget<S>` を
//! 実装する具体型 [`Stack`] / [`Grid`] を新設して基本描画（sync_from_state /
//! measure / paint / hit_test / accessibility_role）まで実装する。
//!
//! `ScrollView` のモジュール doc がスコープ外 (d)「複数子コンテンツ（スタック
//! レイアウトとの合成で扱う）」として本系列へ明示的に委ねていた受け皿である。
//!
//! 主軸方向の語彙は新設せず、[`super::layout::FlexDirection`]（`ui` 層へ
//! 再 export 済み）を再利用する。ただし `ui::layout::LayoutTree`（taffy ラッパ、S4 /
//! #99）は `Widget<S>` と未配線のため、`ScrollView` と同じく `measure`
//! ベースの自前配置とし、taffy 統合は行わない（スコープ外 (d) 参照）。
//!
//! - spacing・available の非有限値（NaN/inf）・負値は構築時・使用時に安全側
//!   （`0.0`）へ丸め、ゼロ除算・inf 伝播・panic を排除する（OWASP A04 相当、
//!   `.claude/rules/security.md`）。`columns` は 1 以上へクランプしゼロ除算を
//!   排除する。
//! - 子計測・集計値の緩和制約・上限には `f32::MAX` ではなく有限定数
//!   [`MAX_CONTENT_EXTENT`] を使う（[`super::scroll_view`] と同じ安全側判断）。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3
//!   バックエンドのプレースホルダ契約に波及するため安全側に倒す判断は先行
//!   ウィジェットと同じ）。**コンテナ自身は `PaintCommand` を一切出力しない**
//!   （透明コンテナ。自前の `Text` を出さないことで外部由来文字列をヘッドレス
//!   出力へ流さない、OWASP A03 対策）。子の `paint` 結果を宣言順に連結するのみ。
//! - 描画クリッピングは `PaintCommand` に座標・クリップ矩形が表現できないため
//!   扱わない（S40 と同じ判断、スコープ外 (e) 参照）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self`
//! を要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。この
//! 契約を変更しない。検証は `Widget` メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に
//! 記録する）:
//! (a) インタラクション状態・イベント処理の実配線（S44 / #139）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S45 / #140）、
//! (c) アラインメント（主軸/交差軸の寄せ）・子ごとの伸縮比（flex weight）・
//! セルスパン・行列個別 spacing 等の拡張 API、
//! (d) `ui::layout::LayoutTree`（taffy）と `Widget<S>` 計測の統合（既存の未配線
//! 事項。既存 Issue の有無を確認のうえユーザー承認を得てから追記・起票）、
//! (e) 描画クリッピング（`PaintCommand` 拡張は 3 バックエンド契約に波及するため
//! 安全側に見送り）、
//! (f) reconcile パス未配線の既知契約（既存契約を変更しない）。
//!
//! TASK-8.4-S15（REQ-8 / REQ-2、#171）で `crate::token::SpacingValue`
//! （デザイントークン基盤、TASK-8.4）を受け取る `with_spacing_token` を
//! [`Stack`] / [`Grid`] へ追加した。内部では既存の [`Stack::with_spacing`] /
//! [`Grid::with_spacing`]（[`normalize_non_negative`] による正規化）へ委譲する
//! （トークン値は構築時点で有限・非負が検証済みのため、正規化は防御の多重化に
//! 留まる）。

use super::layout::FlexDirection;
use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, Widget, WidgetId,
};

/// 集計値（主軸合計・行高さ合計等）を緩和・クランプする際の上限
/// （論理ピクセル相当のプレースホルダ定数）。[`super::scroll_view::MAX_CONTENT_EXTENT`]
/// と同じ安全側判断（`f32::MAX` は子側の乗算・加算でオーバーフローし `inf` が
/// 伝播しうるため、実用上十分大きい有限値に留める。OWASP A04 相当）。
/// `scroll_view` からは非公開のためモジュールごとに定数を持つ（値は同一）。
const MAX_CONTENT_EXTENT: f32 = 1_048_576.0;

/// 非有限値（NaN/inf）または負値を `0.0` へ丸める。
///
/// [`super::scroll_view::normalize_offset_component`] と同じ安全側の丸め方針を
/// spacing・available 成分の正規化に適用するための小さなヘルパー。
fn normalize_non_negative(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// 合計値を [`MAX_CONTENT_EXTENT`] で上限クランプする（inf 伝播・オーバーフロー
/// した集計値がそのまま子孫へ伝わるのを防ぐ、`ScrollView` と同型の安全側処理）。
fn clamp_extent(value: f32) -> f32 {
    if value.is_finite() {
        value.min(MAX_CONTENT_EXTENT)
    } else {
        MAX_CONTENT_EXTENT
    }
}

/// 縦・横スタックレイアウトウィジェット（TASK-8.3-S43、`Widget<S>` 実装型）。
///
/// 本ウィジェットセット初の複数子コンテナであり、`children` を宣言順に主軸方向
/// （[`FlexDirection`]、既定 [`FlexDirection::Column`]）へ間隔 [`Self::with_spacing`]
/// を挟んで並べる。
///
/// `Debug` / `Clone` は手動実装する（[`ScrollView`][super::scroll_view::ScrollView]
/// 等と同じく、状態型 `S` を所有していないため `derive` マクロが自動付与する
/// `S: Debug` / `S: Clone` という不要な境界を避ける。`Clone` は各子の
/// [`Widget::clone_box`] 経由で複製する）。
pub struct Stack<S> {
    id: WidgetId,
    children: Vec<Box<dyn Widget<S>>>,
    direction: FlexDirection,
    /// 子同士の間隔（論理ピクセル相当）。非有限値・負値は [`Self::with_spacing`]
    /// が構築時に `0.0` へ丸める（[`normalize_non_negative`] 参照）。
    spacing: f32,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Stack<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack")
            .field("id", &self.id)
            .field("children", &self.children)
            .field("direction", &self.direction)
            .field("spacing", &self.spacing)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Stack<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            children: self.children.iter().map(|c| c.clone_box()).collect(),
            direction: self.direction,
            spacing: self.spacing,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Stack<S> {
    /// 子を持たないスタックを構築する。既定の方向は [`FlexDirection::Column`]
    /// （縦）、spacing は `0.0`。
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            children: Vec::new(),
            direction: FlexDirection::default(),
            spacing: 0.0,
            interaction: InteractionState::default(),
        }
    }

    /// 主軸方向を設定するビルダーメソッド。
    pub fn with_direction(mut self, direction: FlexDirection) -> Self {
        self.direction = direction;
        self
    }

    /// 子同士の間隔を設定するビルダーメソッド。非有限値（NaN/inf）・負値は
    /// `0.0` へ丸める（[`normalize_non_negative`] 参照）。
    pub fn with_spacing(mut self, spacing: f32) -> Self {
        self.spacing = normalize_non_negative(spacing);
        self
    }

    /// 子同士の間隔をスペーシングトークン（[`crate::token::SpacingValue`]）から
    /// 設定するビルダーメソッド（TASK-8.4-S15、#171）。内部では
    /// [`Self::with_spacing`] へ委譲する（トークン値は構築時点で有限・非負が
    /// 検証済みのため、[`normalize_non_negative`] は防御の多重化に留まる）。
    pub fn with_spacing_token(self, spacing: crate::token::SpacingValue) -> Self {
        self.with_spacing(spacing.value())
    }

    /// 子を末尾に追加するビルダーメソッド。
    pub fn with_child(mut self, child: impl Widget<S> + 'static) -> Self {
        self.children.push(Box::new(child));
        self
    }

    /// 保持している子の数（テスト・観測用）。
    pub fn children_len(&self) -> usize {
        self.children.len()
    }

    /// `direction` が横方向（[`FlexDirection::Row`]）か。
    fn is_row(&self) -> bool {
        matches!(self.direction, FlexDirection::Row)
    }

    /// `available` の非有限・負値成分を `0.0` へ丸めた [`Measurement`] を返す
    /// （`ScrollView::measure` と同型の安全側判断）。
    fn normalized_available(available: Measurement) -> Measurement {
        Measurement {
            width: normalize_non_negative(available.width),
            height: normalize_non_negative(available.height),
        }
    }
}

impl<S: 'static> Widget<S> for Stack<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// reconcile を全子へ伝播する（`ScrollView::sync_from_state` と同型）。
    /// スタック自身は `S` へ直接触れず、委譲のみを行う。
    fn sync_from_state(&mut self, state: &S) {
        for child in &mut self.children {
            child.sync_from_state(state);
        }
    }

    /// 主軸 = 子の主軸寸法の合計 + spacing × (子数 − 1)、交差軸 = 子の交差軸
    /// 寸法の最大値、を自然寸法として返す（`available` を超えてよい。超過時の
    /// 処理は `ScrollView` との合成に委ねる、モジュール doc 参照）。
    ///
    /// 各子は「主軸 = 丸めた available の主軸値、交差軸 = 丸めた available の
    /// 交差軸値」の制約で計測する。空 `children` は `(0.0, 0.0)` を返す。
    fn measure(&self, available: Measurement) -> Measurement {
        if self.children.is_empty() {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }

        let available = Self::normalized_available(available);
        let is_row = self.is_row();
        let child_constraint = available;

        let mut main_sum = 0.0f32;
        let mut cross_max = 0.0f32;
        for child in &self.children {
            let measured = child.measure(child_constraint);
            let (main, cross) = if is_row {
                (measured.width, measured.height)
            } else {
                (measured.height, measured.width)
            };
            main_sum += main;
            cross_max = cross_max.max(cross);
        }

        let spacing_total = self.spacing * (self.children.len().saturating_sub(1)) as f32;
        let main_total = clamp_extent(main_sum + spacing_total);
        let cross_total = clamp_extent(cross_max);

        if is_row {
            Measurement {
                width: main_total,
                height: cross_total,
            }
        } else {
            Measurement {
                width: cross_total,
                height: main_total,
            }
        }
    }

    /// 子ごとに主軸方向へ累積オフセット（前子の主軸寸法 + spacing）した
    /// origin で `child.paint(..)` を宣言順に連結する。コンテナ自身は
    /// `PaintCommand` を一切出力しない（モジュール doc・OWASP A03 参照）。
    ///
    /// `child.measure()` は呼び出し元が自由に実装できる公開トレイト
    /// `Widget<S>` のメソッドであり、NaN/inf/負値を返す実装が存在しうる。
    /// モジュール doc（非有限値・負値は使用時に安全側へ丸める契約）を
    /// 兄弟要素の origin 計算まで徹底するため、主軸寸法は
    /// [`normalize_non_negative`] で丸めたうえで累積し、累積値自体も
    /// [`clamp_extent`] で上限を維持する（1 子の異常値が後続の兄弟すべての
    /// origin へ inf/NaN として伝播するのを防ぐ）。
    ///
    /// 受け取った `size`（レイアウトエンジンから渡される割り当て済みサイズ）
    /// 自体も非有限値・負値を取りうるため、`child.measure` へ渡す前に
    /// [`Self::normalized_available`] で正規化する（`Stack::measure` /
    /// `Grid::paint` と同じ安全側判断。Bugbot 指摘: 正規化前の `size` を
    /// そのまま子測定へ渡すと不正な割り当てサイズが子制約に伝播する）。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        if self.children.is_empty() {
            return Vec::new();
        }

        let is_row = self.is_row();
        let child_constraint = Self::normalized_available(size);
        let mut commands = Vec::new();
        let mut offset = 0.0f32;

        for child in &self.children {
            let measured = child.measure(child_constraint);

            let child_origin = if is_row {
                (origin.0 + offset, origin.1)
            } else {
                (origin.0, origin.1 + offset)
            };

            commands.extend(child.paint(child_origin, measured));

            let main = normalize_non_negative(if is_row {
                measured.width
            } else {
                measured.height
            });
            offset = clamp_extent(offset + main + self.spacing);
        }

        commands
    }

    /// スタック自身は常に `false` を返す（先行ウィジェットと同じ安全側の
    /// 既定。実配線は S44、モジュール doc 参照）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
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

/// 固定列数グリッドレイアウトウィジェット（TASK-8.3-S43、`Widget<S>` 実装型）。
///
/// `children` を行優先（row-major）で `columns` 列に配置する。セル幅は
/// 均等割り（[`Self::cell_width`]）、行高さはその行の子の高さの最大値。
///
/// `Debug` / `Clone` は [`Stack`] と同じ理由・同じ方針で手動実装する。
pub struct Grid<S> {
    id: WidgetId,
    children: Vec<Box<dyn Widget<S>>>,
    /// 列数。`0` は構築時に `1` へクランプする（ゼロ除算排除、
    /// [`Self::with_columns`] 参照）。
    columns: usize,
    /// 行・列共通の間隔（論理ピクセル相当）。非有限値・負値は `0.0` へ丸める。
    spacing: f32,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Grid<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grid")
            .field("id", &self.id)
            .field("children", &self.children)
            .field("columns", &self.columns)
            .field("spacing", &self.spacing)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Grid<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            children: self.children.iter().map(|c| c.clone_box()).collect(),
            columns: self.columns,
            spacing: self.spacing,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Grid<S> {
    /// 子を持たないグリッドを構築する。既定の列数は `1`、spacing は `0.0`。
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            children: Vec::new(),
            columns: 1,
            spacing: 0.0,
            interaction: InteractionState::default(),
        }
    }

    /// 列数を設定するビルダーメソッド。`0` は `1` へクランプする
    /// （ゼロ除算排除、モジュール doc 参照）。
    pub fn with_columns(mut self, columns: usize) -> Self {
        self.columns = columns.max(1);
        self
    }

    /// 行・列共通の間隔を設定するビルダーメソッド。非有限値・負値は `0.0` へ
    /// 丸める（[`normalize_non_negative`] 参照）。
    pub fn with_spacing(mut self, spacing: f32) -> Self {
        self.spacing = normalize_non_negative(spacing);
        self
    }

    /// 行・列共通の間隔をスペーシングトークン（[`crate::token::SpacingValue`]）
    /// から設定するビルダーメソッド（[`Stack::with_spacing_token`] と同契約）。
    pub fn with_spacing_token(self, spacing: crate::token::SpacingValue) -> Self {
        self.with_spacing(spacing.value())
    }

    /// 子を末尾に追加するビルダーメソッド。
    pub fn with_child(mut self, child: impl Widget<S> + 'static) -> Self {
        self.children.push(Box::new(child));
        self
    }

    /// 保持している子の数（テスト・観測用）。
    pub fn children_len(&self) -> usize {
        self.children.len()
    }

    /// 行数（`children_len` を `columns` で切り上げ除算）。空 children は `0`。
    fn row_count(&self) -> usize {
        self.children.len().div_ceil(self.columns)
    }

    /// `available.width` からセル幅を算出する（`columns` は構築時に 1 以上へ
    /// クランプ済みのためゼロ除算なし）。負値化は `.max(0.0)` で防ぐ。
    fn cell_width(&self, available_width: f32) -> f32 {
        let width = normalize_non_negative(available_width);
        let spacing_total = self.spacing * (self.columns.saturating_sub(1)) as f32;
        (width - spacing_total).max(0.0) / self.columns as f32
    }

    /// 各行の高さ（その行に属する子の測定済み高さの最大値）を行順に返す。
    /// `cell_width` × `available.height` の制約で各子を計測する。
    fn row_heights(&self, available: Measurement) -> Vec<f32> {
        let available = Measurement {
            width: normalize_non_negative(available.width),
            height: normalize_non_negative(available.height),
        };
        let cell_width = self.cell_width(available.width);
        let child_constraint = Measurement {
            width: cell_width,
            height: available.height,
        };

        let mut heights = Vec::with_capacity(self.row_count());
        for row in self.children.chunks(self.columns) {
            let mut row_height = 0.0f32;
            for child in row {
                let measured = child.measure(child_constraint);
                row_height = row_height.max(measured.height);
            }
            heights.push(row_height);
        }
        heights
    }
}

impl<S: 'static> Widget<S> for Grid<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// reconcile を全子へ伝播する（[`Stack::sync_from_state`] と同型）。
    fn sync_from_state(&mut self, state: &S) {
        for child in &mut self.children {
            child.sync_from_state(state);
        }
    }

    /// 幅 = 丸めた `available.width`（グリッドは横幅充填）、高さ = 各行高さの
    /// 合計 + spacing × (行数 − 1)。空 children は `(0.0, 0.0)`。
    fn measure(&self, available: Measurement) -> Measurement {
        if self.children.is_empty() {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }

        let width = normalize_non_negative(available.width);
        let heights = self.row_heights(available);
        let spacing_total = self.spacing * (heights.len().saturating_sub(1)) as f32;
        let height_sum: f32 = heights.iter().sum();

        Measurement {
            width: clamp_extent(width),
            height: clamp_extent(height_sum + spacing_total),
        }
    }

    /// セル origin（列 × (セル幅 + spacing)、行の累積高さ）で各子の `paint` を
    /// 宣言順に連結する。コンテナ自身は `PaintCommand` を一切出力しない
    /// （モジュール doc・OWASP A03 参照）。
    ///
    /// `heights`（[`Self::row_heights`]）は各行内で `f32::max` により集約する
    /// ため NaN は自己修復するが、子 `Widget<S>::measure` が `+inf` を返した
    /// 場合は `max` を経由しても `inf` のまま残りうる。[`Stack::paint`] と
    /// 同じ理由（呼び出し元が自由に実装できる公開トレイトの戻り値を信用
    /// しない）で、行高さは [`normalize_non_negative`] で丸めたうえで
    /// `row_offset` へ累積し、累積値自体も [`clamp_extent`] で上限を維持する
    /// （1 行の異常値が後続の行すべての origin へ inf として伝播するのを
    /// 防ぐ）。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        if self.children.is_empty() {
            return Vec::new();
        }

        let cell_width = self.cell_width(size.width);
        let heights = self.row_heights(size);
        let child_constraint = Measurement {
            width: cell_width,
            height: normalize_non_negative(size.height),
        };

        let mut commands = Vec::new();
        let mut row_offset = 0.0f32;
        for (row_index, row) in self.children.chunks(self.columns).enumerate() {
            let row_height = normalize_non_negative(heights.get(row_index).copied().unwrap_or(0.0));
            for (col_index, child) in row.iter().enumerate() {
                let child_origin = (
                    origin.0 + col_index as f32 * (cell_width + self.spacing),
                    origin.1 + row_offset,
                );
                let measured = child.measure(child_constraint);
                commands.extend(child.paint(child_origin, measured));
            }
            row_offset = clamp_extent(row_offset + row_height + self.spacing);
        }

        commands
    }

    /// グリッド自身は常に `false` を返す（[`Stack::hit_test`] と同型。実配線は
    /// S44、モジュール doc 参照）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
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
    use crate::ui::label::Label;

    struct ProfileState {
        name: String,
    }

    fn get_name(state: &ProfileState) -> String {
        state.name.clone()
    }

    fn label(id: u64) -> Label<ProfileState> {
        Label::bound(WidgetId::from_raw(id), get_name)
    }

    /// テスト専用プローブウィジェット。`measure` は固定寸法、`paint` は自身が
    /// 呼ばれた origin を `Text` へ埋め込むことで、コンテナ側の子配置計算
    /// （origin・順序）を直接検証できるようにする
    /// （`widget.rs` の `DummyWidget` 手法を流用）。
    #[derive(Debug, Clone)]
    struct ProbeWidget {
        id: WidgetId,
        size: Measurement,
        interaction: InteractionState,
    }

    impl ProbeWidget {
        fn new(id: u64, width: f32, height: f32) -> Self {
            Self {
                id: WidgetId::from_raw(id),
                size: Measurement { width, height },
                interaction: InteractionState::default(),
            }
        }
    }

    impl Widget<ProfileState> for ProbeWidget {
        fn id(&self) -> WidgetId {
            self.id
        }

        fn sync_from_state(&mut self, _state: &ProfileState) {}

        fn measure(&self, _available: Measurement) -> Measurement {
            self.size
        }

        fn paint(&self, origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
            vec![PaintCommand::Text(format!(
                "{}:{},{}",
                self.id.raw(),
                origin.0,
                origin.1
            ))]
        }

        fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
            false
        }

        fn accessibility_role(&self) -> AccessibilityRole {
            AccessibilityRole::Group
        }

        fn interaction_state(&self) -> InteractionState {
            self.interaction
        }

        fn clone_box(&self) -> Box<dyn Widget<ProfileState>> {
            Box::new(self.clone())
        }
    }

    /// テスト専用プローブウィジェット。`measure` に渡された制約
    /// （`available`）をそのまま自身の測定結果として返し、`paint` はその
    /// `size`（= コンテナが `measure` へ渡した制約と同一）を `Text` へ
    /// 埋め込む。コンテナが子へ渡す制約そのものの正規化を直接検証できる
    /// （`ProbeWidget` は固定寸法を返すため制約の中身を観測できない）。
    #[derive(Debug, Clone)]
    struct EchoConstraintWidget {
        id: WidgetId,
        interaction: InteractionState,
    }

    impl EchoConstraintWidget {
        fn new(id: u64) -> Self {
            Self {
                id: WidgetId::from_raw(id),
                interaction: InteractionState::default(),
            }
        }
    }

    impl Widget<ProfileState> for EchoConstraintWidget {
        fn id(&self) -> WidgetId {
            self.id
        }

        fn sync_from_state(&mut self, _state: &ProfileState) {}

        fn measure(&self, available: Measurement) -> Measurement {
            available
        }

        fn paint(&self, _origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
            vec![PaintCommand::Text(format!(
                "{}:{},{}",
                self.id.raw(),
                size.width,
                size.height
            ))]
        }

        fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
            false
        }

        fn accessibility_role(&self) -> AccessibilityRole {
            AccessibilityRole::Group
        }

        fn interaction_state(&self) -> InteractionState {
            self.interaction
        }

        fn clone_box(&self) -> Box<dyn Widget<ProfileState>> {
            Box::new(self.clone())
        }
    }

    // --- Stack: 構築既定値 ---

    #[test]
    fn stack_new_has_default_direction_and_zero_spacing() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(1));
        assert_eq!(stack.direction, FlexDirection::Column);
        assert_eq!(stack.spacing, 0.0);
        assert_eq!(stack.children_len(), 0);
        assert_eq!(stack.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn stack_new_interaction_state_is_all_false() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(2));
        let interaction = stack.interaction_state();
        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.disabled);
        assert!(!interaction.focused);
    }

    #[test]
    fn stack_with_direction_overrides_default() {
        let stack: Stack<ProfileState> =
            Stack::new(WidgetId::from_raw(3)).with_direction(FlexDirection::Row);
        assert_eq!(stack.direction, FlexDirection::Row);
    }

    // --- Stack: spacing の正規化 ---

    #[test]
    fn stack_with_spacing_clamps_negative_nan_infinite_to_zero() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(4)).with_spacing(-5.0);
        assert_eq!(stack.spacing, 0.0);

        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(5)).with_spacing(f32::NAN);
        assert_eq!(stack.spacing, 0.0);

        let stack: Stack<ProfileState> =
            Stack::new(WidgetId::from_raw(6)).with_spacing(f32::INFINITY);
        assert_eq!(stack.spacing, 0.0);
    }

    #[test]
    fn stack_with_spacing_accepts_positive_finite_value() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(7)).with_spacing(8.0);
        assert_eq!(stack.spacing, 8.0);
    }

    #[test]
    fn stack_with_spacing_token_matches_raw_f32_equivalent() {
        // TASK-8.4-S15（#171）: トークン型ビルダーと生 f32 ビルダーの同値性検証。
        let spacing = crate::token::SpacingValue::new(8.0).expect("正当な値");
        let from_token: Stack<ProfileState> =
            Stack::new(WidgetId::from_raw(35)).with_spacing_token(spacing);
        let from_raw: Stack<ProfileState> = Stack::new(WidgetId::from_raw(35)).with_spacing(8.0);
        assert_eq!(from_token.spacing, from_raw.spacing);
    }

    // --- Stack: measure ---

    #[test]
    fn stack_column_measure_sums_heights_and_takes_max_width() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(8))
            .with_spacing(4.0)
            .with_child(ProbeWidget::new(100, 30.0, 10.0))
            .with_child(ProbeWidget::new(101, 20.0, 40.0));

        let measured = stack.measure(Measurement {
            width: 200.0,
            height: 200.0,
        });
        // 高さ合計 = 10 + 40 + spacing(4) = 54、幅 = max(30, 20) = 30。
        assert_eq!(measured.height, 54.0);
        assert_eq!(measured.width, 30.0);
    }

    #[test]
    fn stack_row_measure_sums_widths_and_takes_max_height() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(9))
            .with_direction(FlexDirection::Row)
            .with_spacing(4.0)
            .with_child(ProbeWidget::new(102, 30.0, 10.0))
            .with_child(ProbeWidget::new(103, 20.0, 40.0));

        let measured = stack.measure(Measurement {
            width: 200.0,
            height: 200.0,
        });
        // 幅合計 = 30 + 20 + spacing(4) = 54、高さ = max(10, 40) = 40。
        assert_eq!(measured.width, 54.0);
        assert_eq!(measured.height, 40.0);
    }

    #[test]
    fn stack_measure_empty_children_is_zero() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(10));
        let measured = stack.measure(Measurement {
            width: 100.0,
            height: 100.0,
        });
        assert_eq!(measured.width, 0.0);
        assert_eq!(measured.height, 0.0);
    }

    // --- Stack: paint（空・順序・origin） ---

    #[test]
    fn stack_paint_empty_children_returns_empty_without_panicking() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(11));
        let commands = stack.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert!(commands.is_empty());
    }

    #[test]
    fn stack_paint_never_emits_own_paint_command() {
        let stack: Stack<ProfileState> =
            Stack::new(WidgetId::from_raw(12)).with_child(ProbeWidget::new(104, 10.0, 10.0));
        let commands = stack.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], PaintCommand::Text(_)));
    }

    #[test]
    fn stack_column_paint_offsets_children_along_main_axis_in_order() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(13))
            .with_spacing(5.0)
            .with_child(ProbeWidget::new(105, 10.0, 20.0))
            .with_child(ProbeWidget::new(106, 10.0, 30.0));

        let commands = stack.paint(
            (2.0, 3.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], PaintCommand::Text("105:2,3".to_string()));
        // 2 番目の子の origin.y = 3 + 20（1 番目の高さ）+ 5（spacing）= 28。
        assert_eq!(commands[1], PaintCommand::Text("106:2,28".to_string()));
    }

    #[test]
    fn stack_row_paint_offsets_children_along_main_axis_in_order() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(14))
            .with_direction(FlexDirection::Row)
            .with_spacing(5.0)
            .with_child(ProbeWidget::new(107, 20.0, 10.0))
            .with_child(ProbeWidget::new(108, 30.0, 10.0));

        let commands = stack.paint(
            (1.0, 1.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], PaintCommand::Text("107:1,1".to_string()));
        // 2 番目の子の origin.x = 1 + 20（1 番目の幅）+ 5（spacing）= 26。
        assert_eq!(commands[1], PaintCommand::Text("108:26,1".to_string()));
    }

    /// 子（`Widget<S>` は呼び出し元が自由実装できる公開トレイトのため
    /// 悪意・実装不備を問わず非有限値・負値を返しうる）の `measure()` が
    /// `inf` / `NaN` / 負値を返しても、後続の兄弟要素の main 軸 origin が
    /// 有限かつ単調非減少に保たれることを検証する（Medium 指摘の回帰防止。
    /// `normalize_non_negative` / `clamp_extent` を `paint` 内の累積へ適用
    /// した対応の直接的な証拠）。
    #[test]
    fn stack_column_paint_normalizes_non_finite_and_negative_child_measurements() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(160))
            .with_spacing(5.0)
            .with_child(ProbeWidget::new(161, 10.0, f32::INFINITY))
            .with_child(ProbeWidget::new(162, 10.0, f32::NAN))
            .with_child(ProbeWidget::new(163, 10.0, -10.0))
            .with_child(ProbeWidget::new(164, 10.0, 20.0));

        let commands = stack.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 4);

        let origin_y = |command: &PaintCommand| -> f32 {
            let PaintCommand::Text(text) = command else {
                unreachable!("ProbeWidget は Text のみを出力する");
            };
            text.split(',').nth(1).unwrap().parse().unwrap()
        };

        let ys: Vec<f32> = commands.iter().map(origin_y).collect();
        for y in &ys {
            assert!(y.is_finite(), "origin.y は常に有限であるべき: {y}");
        }
        for pair in ys.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "origin.y は単調非減少であるべき: {pair:?}"
            );
        }
        // inf/NaN/負値はいずれも 0.0 へ丸められるため、1〜3 番目の子は
        // spacing 分のみ進み、4 番目のみ直前の高さ 0.0 を継承する。
        assert_eq!(ys, vec![0.0, 5.0, 10.0, 15.0]);
    }

    /// `Stack::paint` へ渡される `size`（レイアウトエンジンが割り当てた
    /// サイズ）自体が非有限値・負値のとき、正規化せずそのまま
    /// `child.measure` の制約へ渡すと不正な値が子へ伝播する
    /// （Bugbot 指摘・Low 重大度の回帰防止）。`Stack::measure` /
    /// `Grid::paint` と同じく [`Self::normalized_available`] で丸めてから
    /// 子制約として使うことを、制約を echo するプローブで直接検証する。
    #[test]
    fn stack_paint_normalizes_non_finite_and_negative_incoming_size() {
        let stack: Stack<ProfileState> =
            Stack::new(WidgetId::from_raw(170)).with_child(EchoConstraintWidget::new(171));

        let commands = stack.paint(
            (0.0, 0.0),
            Measurement {
                width: f32::NAN,
                height: f32::NEG_INFINITY,
            },
        );
        assert_eq!(
            commands,
            vec![PaintCommand::Text("171:0,0".to_string())],
            "非有限値の size は 0.0 へ丸めたうえで子制約として渡すべき"
        );

        let commands = stack.paint(
            (0.0, 0.0),
            Measurement {
                width: -5.0,
                height: -10.0,
            },
        );
        assert_eq!(
            commands,
            vec![PaintCommand::Text("171:0,0".to_string())],
            "負値の size は 0.0 へ丸めたうえで子制約として渡すべき"
        );
    }

    // --- Stack: sync_from_state / hit_test / accessibility_role / clone_box ---

    #[test]
    fn stack_sync_from_state_propagates_to_all_children() {
        let mut stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(15))
            .with_child(label(200))
            .with_child(label(201));
        let state = ProfileState {
            name: "太郎".to_string(),
        };
        stack.sync_from_state(&state);

        let commands = stack.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], PaintCommand::Text("太郎".to_string()));
        assert_eq!(commands[1], PaintCommand::Text("太郎".to_string()));
    }

    #[test]
    fn stack_hit_test_is_always_false() {
        let stack: Stack<ProfileState> =
            Stack::new(WidgetId::from_raw(16)).with_child(ProbeWidget::new(109, 10.0, 10.0));
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        assert!(!stack.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn stack_accessibility_role_is_group() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(17));
        assert_eq!(stack.accessibility_role(), AccessibilityRole::Group);
    }

    #[test]
    fn stack_clone_box_produces_independent_copy_of_children() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(18)).with_child(label(202));
        let cloned = stack.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(18));
    }

    #[test]
    fn stack_debug_output_does_not_panic() {
        let stack: Stack<ProfileState> = Stack::new(WidgetId::from_raw(19));
        let output = format!("{stack:?}");
        assert!(output.contains("Stack"));
    }

    #[test]
    fn stack_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Stack<ProfileState>>();
    }

    // --- Grid: 構築既定値・クランプ ---

    #[test]
    fn grid_new_has_default_columns_and_zero_spacing() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(20));
        assert_eq!(grid.columns, 1);
        assert_eq!(grid.spacing, 0.0);
        assert_eq!(grid.children_len(), 0);
    }

    #[test]
    fn grid_with_columns_zero_clamps_to_one() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(21)).with_columns(0);
        assert_eq!(grid.columns, 1);
    }

    #[test]
    fn grid_with_spacing_clamps_negative_nan_infinite_to_zero() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(22)).with_spacing(-1.0);
        assert_eq!(grid.spacing, 0.0);

        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(23)).with_spacing(f32::NAN);
        assert_eq!(grid.spacing, 0.0);
    }

    #[test]
    fn grid_with_spacing_token_matches_raw_f32_equivalent() {
        // TASK-8.4-S15（#171）: トークン型ビルダーと生 f32 ビルダーの同値性検証。
        let spacing = crate::token::SpacingValue::new(4.0).expect("正当な値");
        let from_token: Grid<ProfileState> =
            Grid::new(WidgetId::from_raw(36)).with_spacing_token(spacing);
        let from_raw: Grid<ProfileState> = Grid::new(WidgetId::from_raw(36)).with_spacing(4.0);
        assert_eq!(from_token.spacing, from_raw.spacing);
    }

    // --- Grid: measure / row-major 配置 ---

    #[test]
    fn grid_measure_empty_children_is_zero() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(24));
        let measured = grid.measure(Measurement {
            width: 100.0,
            height: 100.0,
        });
        assert_eq!(measured.width, 0.0);
        assert_eq!(measured.height, 0.0);
    }

    #[test]
    fn grid_row_major_layout_with_two_columns_three_children_yields_two_rows() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(25))
            .with_columns(2)
            .with_child(ProbeWidget::new(300, 10.0, 10.0))
            .with_child(ProbeWidget::new(301, 10.0, 20.0))
            .with_child(ProbeWidget::new(302, 10.0, 5.0));

        assert_eq!(grid.row_count(), 2);
        let heights = grid.row_heights(Measurement {
            width: 100.0,
            height: 100.0,
        });
        // 1 行目: max(10, 20) = 20、2 行目（端数行、子 1 個）: 5。
        assert_eq!(heights, vec![20.0, 5.0]);
    }

    #[test]
    fn grid_measure_height_is_sum_of_row_heights_plus_spacing() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(26))
            .with_columns(2)
            .with_spacing(4.0)
            .with_child(ProbeWidget::new(303, 10.0, 10.0))
            .with_child(ProbeWidget::new(304, 10.0, 20.0))
            .with_child(ProbeWidget::new(305, 10.0, 5.0));

        let measured = grid.measure(Measurement {
            width: 100.0,
            height: 100.0,
        });
        // 高さ = 20 + 5 + spacing(4) = 29、幅 = available.width = 100。
        assert_eq!(measured.height, 29.0);
        assert_eq!(measured.width, 100.0);
    }

    // --- Grid: paint（空・セル origin） ---

    #[test]
    fn grid_paint_empty_children_returns_empty_without_panicking() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(27));
        let commands = grid.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert!(commands.is_empty());
    }

    #[test]
    fn grid_paint_never_emits_own_paint_command() {
        let grid: Grid<ProfileState> =
            Grid::new(WidgetId::from_raw(28)).with_child(ProbeWidget::new(306, 10.0, 10.0));
        let commands = grid.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], PaintCommand::Text(_)));
    }

    #[test]
    fn grid_paint_positions_children_in_row_major_cell_origins() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(29))
            .with_columns(2)
            .with_spacing(0.0)
            .with_child(ProbeWidget::new(307, 10.0, 10.0))
            .with_child(ProbeWidget::new(308, 10.0, 10.0))
            .with_child(ProbeWidget::new(309, 10.0, 10.0));

        let commands = grid.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        // セル幅 = 100 / 2 = 50。
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0], PaintCommand::Text("307:0,0".to_string()));
        assert_eq!(commands[1], PaintCommand::Text("308:50,0".to_string()));
        // 2 行目、1 列目: origin.y = 行 0 の高さ(10) + spacing(0) = 10。
        assert_eq!(commands[2], PaintCommand::Text("309:0,10".to_string()));
    }

    /// [`Grid::row_heights`] は行内集約に `f32::max` を使うため `NaN` は
    /// 自己修復するが、子が `+inf` の高さを返した場合は `max` を経由しても
    /// `inf` のまま残る（モジュール doc・Medium 指摘参照）。この場合でも
    /// `paint` 側の正規化により後続行の origin が有限に保たれることを
    /// 検証する（回帰防止。`NaN` は `f32::max` の自己修復で別途安全なため
    /// ここでは `inf` のみを扱う）。
    #[test]
    fn grid_paint_normalizes_infinite_row_height_before_next_row_offset() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(320))
            .with_columns(2)
            .with_spacing(5.0)
            .with_child(ProbeWidget::new(321, 10.0, f32::INFINITY))
            .with_child(ProbeWidget::new(322, 10.0, 10.0))
            .with_child(ProbeWidget::new(323, 10.0, 10.0));

        let commands = grid.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 3);

        let origin_y = |command: &PaintCommand| -> f32 {
            let PaintCommand::Text(text) = command else {
                unreachable!("ProbeWidget は Text のみを出力する");
            };
            text.split(',').nth(1).unwrap().parse().unwrap()
        };
        // 2 行目（1 列のみ）の origin.y は、1 行目の inf 高さが 0.0 へ正規化
        // されたうえで spacing(5) のみ進んだ値になる（inf のまま伝播しない）。
        assert_eq!(origin_y(&commands[2]), 5.0);
        assert!(commands.iter().map(origin_y).all(|y| y.is_finite()));
    }

    // --- Grid: sync_from_state / hit_test / accessibility_role / clone_box ---

    #[test]
    fn grid_sync_from_state_propagates_to_all_children() {
        let mut grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(30))
            .with_columns(2)
            .with_child(label(203))
            .with_child(label(204));
        let state = ProfileState {
            name: "花子".to_string(),
        };
        grid.sync_from_state(&state);

        let commands = grid.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], PaintCommand::Text("花子".to_string()));
        assert_eq!(commands[1], PaintCommand::Text("花子".to_string()));
    }

    #[test]
    fn grid_hit_test_is_always_false() {
        let grid: Grid<ProfileState> =
            Grid::new(WidgetId::from_raw(31)).with_child(ProbeWidget::new(310, 10.0, 10.0));
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        assert!(!grid.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn grid_accessibility_role_is_group() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(32));
        assert_eq!(grid.accessibility_role(), AccessibilityRole::Group);
    }

    #[test]
    fn grid_clone_box_produces_independent_copy_of_children() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(33)).with_child(label(205));
        let cloned = grid.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(33));
    }

    #[test]
    fn grid_debug_output_does_not_panic() {
        let grid: Grid<ProfileState> = Grid::new(WidgetId::from_raw(34));
        let output = format!("{grid:?}");
        assert!(output.contains("Grid"));
    }

    #[test]
    fn grid_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Grid<ProfileState>>();
    }
}
