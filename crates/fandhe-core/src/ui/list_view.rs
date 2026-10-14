//! リストウィジェット（TASK-8.3-S16、REQ-8 / REQ-13、Issue #111）。
//!
//! [`super::widget::Widget`] トレイト（TASK-8.3-S2 / #97）を実装する具体ウィジェット型
//! として、[`Label`](super::label::Label)（#105）の前例を踏襲したリスト版を提供する。
//!
//! **既存の `Node::List` / `Node::VirtualList` とは別物**: 本モジュールの
//! [`ListView`] は `Node::Widget(Box<dyn Widget<S>>)` 経由で組み込む新ウィジェット
//! 実装層の型であり、TASK-1.1-S1/S4 由来の `Node::List { expand }` / TASK-5.5-S2 由来の
//! `Node::VirtualList { len, item, item_extent }`（子 `Node<S>` 列を展開する DSL 側の
//! 機構）とは名前・責務ともに独立している。名前衝突を避けるため型名は `ListView<S>` とした。
//!
//! **未配線契約（widget.rs 参照）**: `render_headless`（本クレート）・`convert_node`
//! （`fandhe-web-dom`）・`to_snapshot_node`（`fandhe-swiftui-bridge`）は `&Node<S>`
//! 共有参照のみを持ち `sync_from_state`（`&mut`）を呼べないため、`Node::Widget` 経由で
//! 組み込んだ本モジュールのウィジェットは現状これらのバックエンドでは無出力
//! （プレースホルダ）のままとなる。reconcile/sync パスが確立するまでこの契約は
//! 変更しない。検証は `WidgetHarness` 等を通じた直接呼び出し経路で行う。
//!
//! **状態・イベント処理（TASK-8.3-S17 / REQ-8、Issue #112）**:
//! `toggle_switch::ToggleSwitch`（#121）と同じ判断で **別型を新設せず既存
//! [`ListView<S>`] へ直接メソッドを追加**する。縦方向の座標→行 index 解決・
//! press/release 同一項目確定・roving focus は `radio_group::RadioGroupWidget`
//! （#118）・`tab_bar::TabBarWidget`（#148）の縦方向版を踏襲する:
//!
//! - 項目確定（ポインタ・キーボードいずれも）は既存の [`crate::state::Event::ButtonClicked`]
//!   （項目文字列をラベルとして使う）へブリッジするのみとし、widget 層は `&mut S` に
//!   一切触れない（専用イベントバリアント `Event::ListItemActivated` 等の新設は
//!   スコープ外、`.claude/rules/out-of-scope-tracking.md` へ記録）。
//! - キーボード入力は生キーコードを扱わず、意味づけ済み列挙 [`ListViewInput`]
//!   のみを受け付ける（`TabBarInput` / `RadioGroupInput` と同じ設計判断で OWASP A03
//!   対策を型レベルで担保する、`.claude/rules/security.md` 準拠）。選択は Activate
//!   確定型とし、`FocusNext` / `FocusPrev` では確定イベントを発火しない
//!   （selection-follows-focus は採らない、リスト特有の判断。タブの
//!   automatic activation とは異なる）。
//! - [`InteractionState::apply`]（disabled 中の全入力無視・`PointerExited` での
//!   pressed 解除）へ委譲し、press/release の意味論を widget.rs 側と重複実装しない。
//! - `hit_test` は「disabled でなく、point が `origin` + `size` の矩形内」を返すよう
//!   実配線した（半開区間 `[origin, origin + size)`、`Checkbox::hit_test` と同じ
//!   ヒットジオメトリ規約）。
//!
//! **スコープ境界**: スクロールインタラクション実配線（`InteractionInput` にホイール系
//! バリアントがなく、`scroll_view_widget::ScrollViewWidget` / `Node::VirtualList` の
//! 責務）・複数選択・ドラッグ並べ替え・仮想化統合・選択状態の `S` 束縛表示（選択
//! ハイライト描画）は本モジュールには含めない。
//!
//! **`WidgetCatalog` 登録・テスト拡充（TASK-8.3-S18 / Issue #113）**: `hit_test` を
//! 常に `false` へ固定した S16 時点の契約を前提に着手されたが、PR #407 のマージ
//! コンフリクト解消で本 S17 の実配線を取り込んだため、`catalog.rs` の `list` エントリが
//! 実際に接続する [`ListView<S>`] は上記の実配線済み `hit_test` / 選択状態を反映する
//! （`catalog.rs` の `builtin_entries` doc・`list_preview_screen` 参照）。`WidgetCatalog`
//! 登録・結合テスト自体は `hit_test` の実装詳細に依存しないため、追加の実装変更は
//! 不要だった。

use crate::state::{Event, ReadFn, ReadIndexedFn};

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// リストへのキーボード入力を表す列挙（本 issue の核心）。
///
/// 外部由来の生キーコード・文字列を一切扱わず、意味づけ済みの列挙のみで構成する
/// ことで OWASP A03（インジェクション）対策を型レベルで担保する
/// （`TabBarInput` / `RadioGroupInput` と同じ設計判断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListViewInput {
    /// 次の行へ roving focus をラップアラウンドで移動する（下矢印キー相当）。
    /// リストは Activate 確定型のため、移動先の確定イベントは発火しない
    /// （タブの selection-follows-focus とは異なるリスト特有の判断）。
    FocusNext,
    /// 前の行へ roving focus をラップアラウンドで移動する（上矢印キー相当）。
    /// `FocusNext` と同じく確定イベントは発火しない。
    FocusPrev,
    /// フォーカス中の行を確定する（Space/Enter 相当）。
    Activate,
}

/// 1 文字あたりの近似幅（論理ピクセル相当のプレースホルダ定数、[`ListView`] 用）。
///
/// `cosmic-text`（REQ-8）未導入のため、[`super::label::Label`] と同じ単純な
/// 文字数比例の近似計測を用いる。`cosmic-text` 導入時にこの近似は置き換えられる想定
/// （`docs/architecture/widget-architecture.md` §8 でスコープ外と明記済み）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の近似行高（論理ピクセル相当のプレースホルダ定数、[`ListView`] 用）。
///
/// リストは縦に項目数ぶん積み上がるため、`measure` では
/// `項目数 * APPROX_LINE_HEIGHT` の合計高さを算出する（単一行のみの `Label` とは
/// 異なる計測方針。詳細は [`Widget::measure`] 実装のコメント参照）。
const APPROX_LINE_HEIGHT: f32 = 16.0;

/// [`ListView`] の項目供給源。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn`] / [`ReadIndexedFn`]）に限定する。`Bound` は
/// `Node::VirtualList { len, item }`（node.rs 参照）と同じ len + index ベースの形に
/// そろえ、将来の仮想化統合（TASK-5.5 系 `virtual_list` / `recycle` / `scroll` との
/// 接続）を見据える。ただし本 issue（基本描画スコープ）では非仮想化の全件展開のみを
/// 行い、仮想化そのものの統合は行わない。
pub enum ListItemSource<S> {
    /// 状態 `S` に依存しない固定項目列。
    Static(Vec<String>),
    /// 状態 `S` から都度読み取る束縛項目列（[`Widget::sync_from_state`] で
    /// キャッシュを更新する。reconcile フェーズより前は空のまま）。
    Bound {
        /// 総項目数を返す関数ポインタ。
        len: ReadFn<S, usize>,
        /// index から 1 項目分の表示文字列を取得する関数ポインタ。
        item: ReadIndexedFn<S, String>,
    },
}

// `ListItemSource<S>` は `Vec<String>` と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の
// `S: Debug` / `S: Clone` 境界を回避する（`LabelSource` / `node.rs` / `screen.rs` と
// 同じ設計判断）。
impl<S> std::fmt::Debug for ListItemSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ListItemSource::Static(items) => f.debug_tuple("Static").field(items).finish(),
            ListItemSource::Bound { .. } => f
                .debug_struct("Bound")
                .field("len", &"<fn ptr>")
                .field("item", &"<fn ptr>")
                .finish(),
        }
    }
}

impl<S> Clone for ListItemSource<S> {
    fn clone(&self) -> Self {
        match self {
            ListItemSource::Static(items) => ListItemSource::Static(items.clone()),
            ListItemSource::Bound { len, item } => ListItemSource::Bound {
                len: *len,
                item: *item,
            },
        }
    }
}

/// リストウィジェット（TASK-8.3-S16 / Issue #111）。
///
/// [`Widget<S>`] を実装する具体型。`sync_from_state` で更新される表示キャッシュ
/// （`cached_items`）を保持し、`paint` は常にこのキャッシュ値のみを参照する
/// （`Bound` の場合、構築直後は空であり初回 `sync_from_state` まで表示に反映されない
/// reconcile 意味論。[`super::label::Label`] と同一の判断）。
///
/// `Debug` / `Clone` は `Label<S>` と同じ理由（状態 `S` を所有しないフィールドのみで
/// 構成されるため、`derive` が付与する不要な `S: Debug` / `S: Clone` 境界を避ける）で
/// 手動実装する。
pub struct ListView<S> {
    id: WidgetId,
    source: ListItemSource<S>,
    cached_items: Vec<String>,
    /// hover/press(全体)/disabled/focus の一時的状態。press/release/apply_input が
    /// 更新する（本 issue で配線）。
    interaction: InteractionState,
    /// roving focus の現在行（リストが `interaction.focused` の間のみ意味を持つ）。
    /// `sync_from_state` で項目数が縮んだ場合は範囲外へ残さず `None` へクランプする
    /// （`menu::clamp_selected` / `radio_group::clamp_index` と同じ安全側判断）。
    focused_index: Option<usize>,
    /// ポインタ press 中の行インデックス（同一行上での release のみ確定イベントを
    /// 発火するための追跡状態、モジュール doc 参照。`tab_bar::TabBarWidget::pressed_tab`
    /// と同型）。
    pressed_item: Option<usize>,
}

impl<S> std::fmt::Debug for ListView<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListView")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("cached_items", &self.cached_items)
            .field("interaction", &self.interaction)
            .field("focused_index", &self.focused_index)
            .field("pressed_item", &self.pressed_item)
            .finish()
    }
}

impl<S> Clone for ListView<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            cached_items: self.cached_items.clone(),
            interaction: self.interaction,
            focused_index: self.focused_index,
            pressed_item: self.pressed_item,
        }
    }
}

impl<S> ListView<S> {
    /// 状態 `S` に依存しない固定項目列のリストを構築する。
    ///
    /// `cached_items` は構築直後から `items` で初期化されるため、`sync_from_state`
    /// 呼び出し前でも `paint` が期待どおりの項目列を返す。
    pub fn fixed(id: WidgetId, items: Vec<String>) -> Self {
        Self {
            id,
            cached_items: items.clone(),
            source: ListItemSource::Static(items),
            interaction: InteractionState::default(),
            focused_index: None,
            pressed_item: None,
        }
    }

    /// 状態 `S` に束縛されたリストを構築する。
    ///
    /// `cached_items` は構築直後は空であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、[`Widget::sync_from_state`]
    /// 参照）。
    pub fn bound(id: WidgetId, len: ReadFn<S, usize>, item: ReadIndexedFn<S, String>) -> Self {
        Self {
            id,
            source: ListItemSource::Bound { len, item },
            cached_items: Vec::new(),
            interaction: InteractionState::default(),
            focused_index: None,
            pressed_item: None,
        }
    }

    /// 現在の表示キャッシュ項目列を返す（テスト・観測用）。
    pub fn items(&self) -> &[String] {
        &self.cached_items
    }

    /// roving focus の現在行を返す（テスト・観測用アクセサ）。
    pub fn focused_index(&self) -> Option<usize> {
        self.focused_index
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。`true` にした時点で press 追跡もクリアする
    /// （[`InteractionState::set_disabled`] が hover/pressed/focused を解除するのと
    /// 揃え、無効化されたリストが press 中の見た目のまま残る不整合を防ぐ、
    /// `tab_bar::TabBarWidget::set_disabled` と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.pressed_item = None;
        }
    }

    /// 座標からヒットした行インデックスを解決する（縦積み 1 行高さの暫定近似、
    /// `radio_group::RadioGroupWidget::option_at` の縦方向版）。
    ///
    /// disabled 中・矩形外・項目数を超える行は `None` を返す（`Checkbox::hit_test` と
    /// 同じ半開区間 `[origin, origin + size)` の意味論）。NaN / inf な座標は
    /// 領域外判定で弾かれるか、`as usize` の飽和キャスト（NaN は 0 になる）により
    /// panic しない（`radio_group::RadioGroupWidget::option_at` と同じ数値健全性方針）。
    pub fn item_at(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> Option<usize> {
        if self.interaction.disabled {
            return None;
        }
        if !(point.0 >= origin.0 && point.0 < origin.0 + size.width) {
            return None;
        }
        if !(point.1 >= origin.1 && point.1 < origin.1 + size.height) {
            return None;
        }
        let relative_y = point.1 - origin.1;
        let index = (relative_y / APPROX_LINE_HEIGHT) as usize;
        (index < self.cached_items.len()).then_some(index)
    }

    /// ポインタ押下を記録する（本 issue の核心）。
    ///
    /// `item_at` で解決した行を `pressed_item` として追跡し、この時点ではまだ確定
    /// イベントを発火しない（確定は [`Self::release`] が同一行上でのみ行う）。あわせて
    /// `InteractionState::apply(PointerPressed)` を適用し、[`Self::visual_state`] が
    /// `Pressed` を返せるようにする（`press`/`release` は `InteractionState` の press
    /// 遷移を自前で管理する契約とし、呼び出し側が `apply_input(PointerPressed)` を
    /// 二重に呼ぶ必要はない、`tab_bar::TabBarWidget::press` と同じ設計）。
    pub fn press(&mut self, point: (f32, f32), origin: (f32, f32), size: Measurement) {
        self.pressed_item = self.item_at(point, origin, size);
        self.interaction.apply(InteractionInput::PointerPressed);
    }

    /// ポインタ解放を処理し、press 時と同一行上での release のみ確定イベントを
    /// 発火する（本 issue の核心）。
    ///
    /// press → 領域外へ exit（[`Self::apply_input`] の `PointerExited`）→ release と
    /// いう経路では `pressed_item` が既に `None` のため発火しない。press した行と
    /// 異なる行上で release した場合も発火しない（ドラッグして離す操作の安全側の扱い）。
    /// あわせて `InteractionState::apply(PointerReleased)` を適用する（[`Self::press`]
    /// と対称）。
    ///
    /// 確定イベントを発火する分岐では `focused_index` も確定した行へ同期する
    /// （roving focus とポインタ確定の位置がずれると、直後の `FocusNext` /
    /// `FocusPrev` / `Activate` がポインタ操作前のフォーカス位置から動作してしまう
    /// ため。`tab_bar::TabBarWidget::release` が対応した Bugbot 指摘
    /// 「Pointer select skips focus update」と同じ回帰防止判断）。
    pub fn release(
        &mut self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> Option<Event> {
        let released_at = self.item_at(point, origin, size);
        let pressed = self.pressed_item.take();
        self.interaction.apply(InteractionInput::PointerReleased);
        match (pressed, released_at) {
            (Some(p), Some(r)) if p == r => {
                self.focused_index = Some(r);
                Some(Event::ButtonClicked {
                    label: self.cached_items[r].clone(),
                })
            }
            _ => None,
        }
    }

    /// ポインタ出入り・フォーカス移動を適用する（`InteractionState::apply` への委譲）。
    ///
    /// `PointerExited` は `pressed_item` も連動してクリアする（press 中に領域外へ
    /// 出たら押下追跡をキャンセルする意味論、`tab_bar::TabBarWidget::apply_input` と
    /// 同じ判断）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);

        if matches!(input, InteractionInput::PointerExited) {
            self.pressed_item = None;
        }
    }

    /// キーボード操作を処理する（本 issue の核心）。
    ///
    /// disabled または項目が空なら常に `None` を返し panic させない
    /// （`.claude/rules/coding-rust.md` のエラーハンドリング方針）。`Button` /
    /// `ToggleSwitch` / `RadioGroupWidget` / `TabBarWidget` と同様、非フォーカス時は
    /// 入力を無視する（`interaction.focused` チェック。フォーカス外での
    /// `FocusNext` / `FocusPrev` による `focused_index` 移動や、フォーカス外での
    /// `Activate` による `Event::ButtonClicked` 誤発火を防ぐ）。`FocusNext` /
    /// `FocusPrev` はラップアラウンドで roving focus を移動するのみで確定イベントは
    /// 発火しない（selection-follows-focus は採らないリスト特有の判断、モジュール doc
    /// 参照）。`Activate` はフォーカス中の行があれば確定イベントを発火する。
    pub fn handle_input(&mut self, input: ListViewInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused || self.cached_items.is_empty() {
            return None;
        }

        match input {
            ListViewInput::FocusNext => {
                let next = match self.focused_index {
                    Some(current) => (current + 1) % self.cached_items.len(),
                    None => 0,
                };
                self.focused_index = Some(next);
                None
            }
            ListViewInput::FocusPrev => {
                let prev = match self.focused_index {
                    Some(current) => {
                        (current + self.cached_items.len() - 1) % self.cached_items.len()
                    }
                    None => self.cached_items.len() - 1,
                };
                self.focused_index = Some(prev);
                None
            }
            ListViewInput::Activate => {
                let index = self.focused_index?;
                Some(Event::ButtonClicked {
                    label: self.cached_items[index].clone(),
                })
            }
        }
    }
}

impl<S: 'static> Widget<S> for ListView<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn sync_from_state(&mut self, state: &S) {
        match &self.source {
            ListItemSource::Static(items) => {
                // 固定項目列は状態に依存しないため不変。呼び出し自体は許容し、
                // reconcile ループの呼び出し規約（全ウィジェット一律呼び出し）を崩さない。
                self.cached_items = items.clone();
            }
            ListItemSource::Bound { len, item } => {
                // 基本描画スコープでは非仮想化の全件マテリアライズとする。大規模リストの
                // 仮想化（性能経路）は既存 `Node::VirtualList` / TASK-5.5 系との統合
                // （後続工程）に委ねる（DoS/リソース考慮、`.claude/rules/security.md`）。
                let count = len(state);
                self.cached_items = (0..count).map(|index| item(state, index)).collect();
            }
        }

        // 項目数が縮んだ場合に `focused_index` / `pressed_item` が範囲外に残らない
        // ようクランプする（`menu::clamp_selected` / `radio_group::clamp_index` と
        // 同じ安全側判断。範囲外インデックスを保持したまま `handle_input` /
        // `release` が `cached_items[index]` へアクセスすると panic しうるため）。
        let len = self.cached_items.len();
        self.focused_index = self.focused_index.filter(|&index| index < len);
        self.pressed_item = self.pressed_item.filter(|&index| index < len);
    }

    /// 項目列の近似計測を返す（`cosmic-text` 未導入時のプレースホルダ、モジュール doc の
    /// `APPROX_CHAR_WIDTH` / `APPROX_LINE_HEIGHT` 参照）。
    ///
    /// 幅は最長項目の `chars().count()`（Unicode スカラ値数、マルチバイト文字対応）を
    /// 基準に `available.width` でクランプする。高さは単一行の `Label` と異なり
    /// `項目数 * APPROX_LINE_HEIGHT`（リストは縦に伸びるため）を `available.height` で
    /// クランプする。項目が 0 件の場合は幅・高さともに 0.0 になる。
    fn measure(&self, available: Measurement) -> Measurement {
        let longest_chars = self
            .cached_items
            .iter()
            .map(|item| item.chars().count())
            .max()
            .unwrap_or(0);
        let natural_width = longest_chars as f32 * APPROX_CHAR_WIDTH;
        let natural_height = self.cached_items.len() as f32 * APPROX_LINE_HEIGHT;
        Measurement {
            width: natural_width.min(available.width),
            height: natural_height.min(available.height),
        }
    }

    /// 現在の表示キャッシュを項目順の [`PaintCommand::Text`] 列へ変換する。
    ///
    /// `origin` / `size` は将来の描画バックエンド（項目背景・区切り線等）向けの
    /// 拡張点として受け取るのみで、現時点では描画コマンド生成に使わない。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        self.cached_items
            .iter()
            .cloned()
            .map(PaintCommand::Text)
            .collect()
    }

    /// リスト全体の矩形包含判定（AABB、本 issue で実配線）。`interaction.disabled` の
    /// 間は `false` を返し、無効化されたリストを入力対象にしない（`Checkbox::hit_test`
    /// / `tab_bar::TabBarWidget::hit_test` と同じ設計判断）。
    ///
    /// 共有ジオメトリ契約である半開区間 `[origin, origin + size)` で判定する
    /// （`Checkbox::hit_test` と同じ規約）。行内位置の解決は [`Self::item_at`] が
    /// 別途担う。
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
        AccessibilityRole::List
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

    struct ItemsState {
        items: Vec<String>,
    }

    fn read_len(state: &ItemsState) -> usize {
        state.items.len()
    }

    fn read_item(state: &ItemsState, index: usize) -> String {
        state.items[index].clone()
    }

    #[test]
    fn fixed_list_returns_items_immediately_without_sync() {
        let list: ListView<ItemsState> = ListView::fixed(
            WidgetId::from_raw(1),
            vec!["a".to_string(), "b".to_string()],
        );
        assert_eq!(list.items(), ["a".to_string(), "b".to_string()]);

        let commands = list.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(
            commands,
            vec![
                PaintCommand::Text("a".to_string()),
                PaintCommand::Text("b".to_string())
            ]
        );
    }

    #[test]
    fn bound_list_is_empty_before_first_sync() {
        let list: ListView<ItemsState> =
            ListView::bound(WidgetId::from_raw(2), read_len, read_item);
        assert!(list.items().is_empty());
    }

    #[test]
    fn bound_list_reflects_state_after_sync() {
        let mut list: ListView<ItemsState> =
            ListView::bound(WidgetId::from_raw(3), read_len, read_item);
        list.sync_from_state(&ItemsState {
            items: vec!["x".to_string(), "y".to_string(), "z".to_string()],
        });
        assert_eq!(
            list.items(),
            ["x".to_string(), "y".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn bound_list_cache_follows_state_changes_across_syncs() {
        let mut list: ListView<ItemsState> =
            ListView::bound(WidgetId::from_raw(4), read_len, read_item);

        list.sync_from_state(&ItemsState {
            items: vec!["1".to_string()],
        });
        assert_eq!(list.items(), ["1".to_string()]);

        list.sync_from_state(&ItemsState {
            items: vec!["2".to_string(), "3".to_string()],
        });
        assert_eq!(list.items(), ["2".to_string(), "3".to_string()]);
    }

    #[test]
    fn measure_scales_with_longest_item_and_item_count_and_clamps_to_available() {
        let list: ListView<ItemsState> = ListView::fixed(
            WidgetId::from_raw(5),
            vec!["abcdefghij".to_string(), "short".to_string()],
        );
        let unclamped = list.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        assert_eq!(unclamped.width, 10.0 * APPROX_CHAR_WIDTH);
        assert_eq!(unclamped.height, 2.0 * APPROX_LINE_HEIGHT);

        let clamped = list.measure(Measurement {
            width: 5.0,
            height: 10.0,
        });
        assert_eq!(clamped.width, 5.0);
        assert_eq!(clamped.height, 10.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        // 「こんにちは」は 5 文字（Unicode スカラ値）だが、UTF-8 では 1 文字 3 バイトの
        // ため 15 バイト。`chars().count()` 基準であることを固定化する回帰テスト。
        let list: ListView<ItemsState> =
            ListView::fixed(WidgetId::from_raw(6), vec!["こんにちは".to_string()]);
        let measurement = list.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        assert_eq!(measurement.width, 5.0 * APPROX_CHAR_WIDTH);
    }

    #[test]
    fn measure_is_zero_when_no_items() {
        let list: ListView<ItemsState> = ListView::fixed(WidgetId::from_raw(7), Vec::new());
        let measurement = list.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn paint_returns_empty_vec_when_no_items() {
        let list: ListView<ItemsState> = ListView::fixed(WidgetId::from_raw(8), Vec::new());
        let commands = list.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert!(commands.is_empty());
    }

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let list: ListView<ItemsState> =
            ListView::fixed(WidgetId::from_raw(9), vec!["a".to_string()]);
        let size = Measurement {
            width: 50.0,
            height: 20.0,
        };
        assert!(list.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_returns_false_on_bottom_right_edge() {
        let list: ListView<ItemsState> =
            ListView::fixed(WidgetId::from_raw(9), vec!["a".to_string()]);
        let size = Measurement {
            width: 50.0,
            height: 20.0,
        };
        assert!(!list.hit_test((size.width, size.height), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_returns_false_when_disabled() {
        let mut list: ListView<ItemsState> =
            ListView::fixed(WidgetId::from_raw(9), vec!["a".to_string()]);
        list.set_disabled(true);
        let size = Measurement {
            width: 50.0,
            height: 20.0,
        };
        assert!(!list.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn accessibility_role_is_list() {
        let list: ListView<ItemsState> =
            ListView::fixed(WidgetId::from_raw(10), vec!["a".to_string()]);
        assert_eq!(list.accessibility_role(), AccessibilityRole::List);
    }

    #[test]
    fn id_and_default_interaction_state_are_reported() {
        let list: ListView<ItemsState> =
            ListView::fixed(WidgetId::from_raw(11), vec!["a".to_string()]);
        assert_eq!(list.id(), WidgetId::from_raw(11));
        assert_eq!(list.interaction_state(), InteractionState::default());
    }

    #[test]
    fn clone_box_preserves_cached_items_and_id() {
        let mut list: ListView<ItemsState> =
            ListView::bound(WidgetId::from_raw(12), read_len, read_item);
        list.sync_from_state(&ItemsState {
            items: vec!["p".to_string(), "q".to_string()],
        });

        let boxed: Box<dyn Widget<ItemsState>> = Box::new(list);
        let cloned = boxed.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(12));
        let commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(
            commands,
            vec![
                PaintCommand::Text("p".to_string()),
                PaintCommand::Text("q".to_string())
            ]
        );
    }

    #[test]
    fn debug_output_does_not_panic() {
        let list: ListView<ItemsState> =
            ListView::fixed(WidgetId::from_raw(13), vec!["a".to_string()]);
        let output = format!("{list:?}");
        assert!(output.contains("ListView"));
    }

    #[test]
    fn list_view_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<ItemsState>> = Box::new(ListView::fixed(
            WidgetId::from_raw(14),
            vec!["a".to_string()],
        ));
        let commands = widget.paint(
            (0.0, 0.0),
            Measurement {
                width: 1000.0,
                height: 1000.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Text("a".to_string())]);
    }

    /// `Widget<S>: Send + Sync` 境界を `ListView<S>` が満たすことの静的アサーション
    /// （PR #357 レビュー指摘対応と同じ回帰防止パターン、widget.rs / label.rs 参照）。
    #[test]
    fn list_view_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ListView<ItemsState>>();
    }

    // 以下、TASK-8.3-S17（Issue #112）の状態・イベント処理テスト。

    fn full_size() -> Measurement {
        Measurement {
            width: 100.0,
            height: 100.0,
        }
    }

    fn three_item_list() -> ListView<ItemsState> {
        ListView::fixed(
            WidgetId::from_raw(20),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        )
    }

    fn focus(list: &mut ListView<ItemsState>) {
        list.apply_input(InteractionInput::FocusGained);
    }

    // (a) item_at

    #[test]
    fn item_at_resolves_row_by_vertical_position() {
        let list = three_item_list();
        let size = full_size();

        assert_eq!(list.item_at((5.0, 0.0), (0.0, 0.0), size), Some(0));
        assert_eq!(
            list.item_at((5.0, APPROX_LINE_HEIGHT), (0.0, 0.0), size),
            Some(1)
        );
        assert_eq!(
            list.item_at((5.0, APPROX_LINE_HEIGHT * 2.0), (0.0, 0.0), size),
            Some(2)
        );
    }

    #[test]
    fn item_at_returns_none_outside_rect() {
        let list = three_item_list();
        let size = full_size();

        assert_eq!(list.item_at((-1.0, 5.0), (0.0, 0.0), size), None);
        assert_eq!(list.item_at((5.0, -1.0), (0.0, 0.0), size), None);
        assert_eq!(list.item_at((size.width, 5.0), (0.0, 0.0), size), None);
        assert_eq!(list.item_at((5.0, size.height), (0.0, 0.0), size), None);
    }

    #[test]
    fn item_at_returns_none_beyond_item_count() {
        let list = three_item_list();
        let size = Measurement {
            width: 100.0,
            height: 1000.0,
        };

        // 3 項目のみだが領域は十分に大きいため、行 3 以降は項目が存在せず None。
        assert_eq!(
            list.item_at((5.0, APPROX_LINE_HEIGHT * 3.0), (0.0, 0.0), size),
            None
        );
    }

    #[test]
    fn item_at_returns_none_for_nan_or_inf_coordinates() {
        let list = three_item_list();
        let size = full_size();

        assert_eq!(
            list.item_at((f32::NAN, 5.0), (0.0, 0.0), size),
            None,
            "NaN な x 座標は矩形内判定に失敗し None を返す"
        );
        assert_eq!(
            list.item_at((5.0, f32::INFINITY), (0.0, 0.0), size),
            None,
            "+inf な y 座標は矩形内判定に失敗し None を返す"
        );
    }

    #[test]
    fn item_at_returns_none_on_empty_list() {
        let list: ListView<ItemsState> = ListView::fixed(WidgetId::from_raw(21), Vec::new());
        let size = full_size();

        assert_eq!(list.item_at((5.0, 5.0), (0.0, 0.0), size), None);
    }

    // (b) ポインタ確定

    #[test]
    fn press_then_release_on_same_row_fires_event() {
        let mut list = three_item_list();
        let size = full_size();
        let point = (5.0, APPROX_LINE_HEIGHT);

        list.press(point, (0.0, 0.0), size);
        let event = list.release(point, (0.0, 0.0), size);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "b".to_string(),
            })
        );
        assert_eq!(list.focused_index(), Some(1));
    }

    #[test]
    fn release_on_different_row_does_not_fire() {
        let mut list = three_item_list();
        let size = full_size();

        list.press((5.0, 0.0), (0.0, 0.0), size);
        let event = list.release((5.0, APPROX_LINE_HEIGHT), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn drag_away_then_release_does_not_fire() {
        let mut list = three_item_list();
        let size = full_size();

        list.press((5.0, 0.0), (0.0, 0.0), size);
        list.apply_input(InteractionInput::PointerExited);
        let event = list.release((5.0, 0.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn press_sets_pressed_visual_state_and_release_clears_it() {
        let mut list = three_item_list();
        let size = full_size();

        list.press((5.0, 0.0), (0.0, 0.0), size);
        assert_eq!(list.visual_state(), VisualState::Pressed);

        list.release((5.0, 0.0), (0.0, 0.0), size);
        assert_eq!(list.visual_state(), VisualState::Idle);
    }

    #[test]
    fn disabled_list_ignores_press_and_release() {
        let mut list = three_item_list();
        list.set_disabled(true);
        let size = full_size();

        list.press((5.0, 0.0), (0.0, 0.0), size);
        let event = list.release((5.0, 0.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn set_disabled_true_clears_pressed_item() {
        let mut list = three_item_list();
        let size = full_size();

        list.press((5.0, 0.0), (0.0, 0.0), size);
        list.set_disabled(true);
        list.set_disabled(false);
        let event = list.release((5.0, 0.0), (0.0, 0.0), size);

        assert_eq!(
            event, None,
            "set_disabled(true) で pressed_item が解除されているため release しても発火しない"
        );
    }

    // (c) キーボード

    #[test]
    fn focus_next_wraps_around_without_firing_event() {
        let mut list = three_item_list();
        focus(&mut list);

        list.handle_input(ListViewInput::FocusNext);
        list.handle_input(ListViewInput::FocusNext);
        list.handle_input(ListViewInput::FocusNext);
        let event = list.handle_input(ListViewInput::FocusNext);

        assert_eq!(list.focused_index(), Some(0));
        assert_eq!(
            event, None,
            "FocusNext/FocusPrev は selection-follows-focus を採らないため常に None"
        );
    }

    #[test]
    fn focus_prev_wraps_around_without_firing_event() {
        let mut list = three_item_list();
        focus(&mut list);

        let event = list.handle_input(ListViewInput::FocusPrev);

        assert_eq!(list.focused_index(), Some(2));
        assert_eq!(event, None);
    }

    #[test]
    fn activate_fires_event_for_focused_row() {
        let mut list = three_item_list();
        focus(&mut list);
        list.handle_input(ListViewInput::FocusNext);

        let event = list.handle_input(ListViewInput::Activate);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "a".to_string(),
            })
        );
    }

    #[test]
    fn activate_without_focus_returns_none() {
        let mut list = three_item_list();

        let event = list.handle_input(ListViewInput::Activate);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_input_ignored_when_disabled() {
        let mut list = three_item_list();
        focus(&mut list);
        list.handle_input(ListViewInput::FocusNext);
        list.set_disabled(true);

        let event = list.handle_input(ListViewInput::Activate);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_input_ignored_when_not_focused() {
        // Cursor Bugbot 指摘の回帰テスト（PR #406）: handle_input が
        // interaction.focused を確認していないと、フォーカス外でも FocusNext /
        // FocusPrev が focused_index を動かし、Activate が非フォーカスでも
        // Event::ButtonClicked を発火してしまう。set_disabled(true) の後
        // set_disabled(false) を呼ぶとフォーカスは解除されるが focused_index は
        // 残ったままになる再現手順を検証する。
        let mut list = three_item_list();
        focus(&mut list);
        list.handle_input(ListViewInput::FocusNext);
        list.set_disabled(true);
        list.set_disabled(false);
        assert_eq!(
            list.focused_index(),
            Some(0),
            "set_disabled(false) 後も focused_index は残るが interaction.focused は解除されている前提"
        );

        let focus_next_event = list.handle_input(ListViewInput::FocusNext);
        let focused_after_focus_next = list.focused_index();
        let activate_event = list.handle_input(ListViewInput::Activate);

        assert_eq!(
            focus_next_event, None,
            "非フォーカス時は FocusNext を無視し None を返す"
        );
        assert_eq!(
            focused_after_focus_next,
            Some(0),
            "非フォーカス時は focused_index を移動させない（Button 等と同様の focused チェック）"
        );
        assert_eq!(
            activate_event, None,
            "非フォーカス時は Activate を無視し Event::ButtonClicked を発火しない"
        );
    }

    #[test]
    fn keyboard_input_on_empty_list_is_safe_none() {
        let mut list: ListView<ItemsState> = ListView::fixed(WidgetId::from_raw(22), Vec::new());
        focus(&mut list);

        assert_eq!(list.handle_input(ListViewInput::FocusNext), None);
        assert_eq!(list.handle_input(ListViewInput::FocusPrev), None);
        assert_eq!(list.handle_input(ListViewInput::Activate), None);
    }

    // (d) disabled / visual_state

    #[test]
    fn visual_state_priority_is_delegated_to_interaction_state() {
        let mut list = three_item_list();
        assert_eq!(list.visual_state(), VisualState::Idle);

        list.apply_input(InteractionInput::PointerEntered);
        assert_eq!(list.visual_state(), VisualState::Hovered);

        list.set_disabled(true);
        assert_eq!(list.visual_state(), VisualState::Disabled);
    }

    // (e) sync_from_state のクランプ

    #[test]
    fn sync_from_state_clamps_focused_index_when_items_shrink() {
        let mut list: ListView<ItemsState> =
            ListView::bound(WidgetId::from_raw(23), read_len, read_item);
        list.sync_from_state(&ItemsState {
            items: vec!["x".to_string(), "y".to_string(), "z".to_string()],
        });
        focus(&mut list);
        list.handle_input(ListViewInput::FocusPrev); // index 2 へ移動

        list.sync_from_state(&ItemsState {
            items: vec!["x".to_string()],
        });

        assert_eq!(
            list.focused_index(),
            None,
            "範囲外に残った focused_index は sync_from_state で None へクランプされる"
        );
    }

    #[test]
    fn sync_from_state_clamps_pressed_item_when_items_shrink() {
        let mut list: ListView<ItemsState> =
            ListView::bound(WidgetId::from_raw(24), read_len, read_item);
        list.sync_from_state(&ItemsState {
            items: vec!["x".to_string(), "y".to_string(), "z".to_string()],
        });
        list.press((5.0, APPROX_LINE_HEIGHT * 2.0), (0.0, 0.0), full_size());

        list.sync_from_state(&ItemsState {
            items: vec!["x".to_string()],
        });
        let event = list.release((5.0, 0.0), (0.0, 0.0), full_size());

        assert_eq!(
            event, None,
            "範囲外に残った pressed_item は sync_from_state でクランプされ、release しても発火しない"
        );
    }

    // (f) StateStore::dispatch 経由の往復統合テスト

    #[test]
    fn state_store_dispatch_round_trip_fires_button_clicked() {
        use crate::state::StateStore;
        use crate::ui::{Screen, WidgetId};

        struct AppState {
            items: Vec<String>,
            clicked: Option<String>,
        }

        fn record_a(state: &mut AppState) {
            state.clicked = Some("a".to_string());
        }
        fn record_b(state: &mut AppState) {
            state.clicked = Some("b".to_string());
        }

        let screen: Screen<AppState> = Screen::new("list")
            .button(WidgetId::from_raw(1), "a", record_a)
            .button(WidgetId::from_raw(2), "b", record_b);
        let mut store = StateStore::new(AppState {
            items: vec!["a".to_string(), "b".to_string()],
            clicked: None,
        });

        fn read_len(state: &AppState) -> usize {
            state.items.len()
        }
        fn read_item(state: &AppState, index: usize) -> String {
            state.items[index].clone()
        }

        let mut list: ListView<AppState> =
            ListView::bound(WidgetId::from_raw(25), read_len, read_item);
        list.sync_from_state(store.state());

        let size = full_size();
        let point = (5.0, APPROX_LINE_HEIGHT);
        list.press(point, (0.0, 0.0), size);
        let event = list
            .release(point, (0.0, 0.0), size)
            .expect("2 行目（\"b\"）の press/release は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert_eq!(store.state().clicked, Some("b".to_string()));
    }
}
