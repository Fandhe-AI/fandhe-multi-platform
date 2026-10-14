//! TV フォーカスナビゲーション実機検証用サンプル画面（TASK-17.2 / REQ-17）。
//!
//! `docs/architecture/tv-focus-navigation-architecture.md` §8「後続タスクで確認すべき
//! こと」がカバーする確認事項のうち、単一画面で機械的に再現できるもの（上下左右移動・
//! 決定操作・`Axis::Grid` の行境界・大規模 `VirtualList` での性能）を検証できる構成にする。
//! 実際の入力駆動・フォーカスハイライト・reveal 配線は [`crate::tv_focus`] が担い、
//! 本モジュールは `Screen<TvState>` の構築（画面の「内容」）のみを持つ。
//!
//! 画面構成（`docs/architecture/tv-focus-navigation-architecture.md` §4.4「机上検証」の
//! ツリー例に対応）:
//! - 見出しテキスト（フォーカス不可）
//! - 棚 1・棚 2（`Node::List`、`Axis::Horizontal`、各 3 項目）: 縦に 2 段積んだ棚を
//!   `Down`/`Up` で棚ごと移動できることを検証する（Cursor Bugbot 指摘の実機検証）
//! - グリッド（`Node::VirtualList`、`Axis::Grid { columns: 3 }`）: 行境界での
//!   `Left`/`Right` no-op・`Up`/`Down` の行間ジャンプ・grid 最上行から棚へのエスケープを
//!   検証する
//! - ネストしたリスト（`Node::VirtualList` の項目がさらに `Node::VirtualList` を返す
//!   1 段ネスト構造、`Axis::Vertical`）: §4.6 が定める多段 reveal（`FocusEntry.path` 上の
//!   すべての `VirtualList` 祖先を外側→内側の順に最小移動で更新する契約）を検証する
//!   （reviewer 指摘 medium。実運用ではスクロール可能領域を入れ子にする UI は避けるのが
//!   一般的だが、§4.6 の契約自体は「ネストした `VirtualList`」を明示的に扱うため、
//!   検証用にあえて用意する）
//! - 大規模リスト（`Node::VirtualList`、`Axis::Vertical`、1200 項目）: 大規模
//!   `VirtualList` でのフォーカス移動性能（§4.5・8 節確認事項 8）と reveal（§4.6・
//!   確認事項 9）を検証する
//!
//! **`WidgetId` の採番について（reviewer 指摘 low）**: 本ファイルの各項目 id は
//! `1 + i`（棚 1）・`11 + i`（棚 2）・`21 + index`（グリッド）・`1000 + index`
//! （大規模リスト）・`NESTED_ID_BASE + index`（ネストしたリスト）といずれも
//! **index 由来**であり、これは設計文書 §3.1 が警告するアンチパターン（「並び順ではなく
//! 項目が表すデータの永続的な同一性から採番することを推奨する」）と同型である。
//! 本サンプル画面は項目の挿入・削除・並べ替えを一切行わない固定順の構成のため実害はない
//! （id と論理項目の対応がフレームをまたいでずれることがない）が、実画面で
//! `List`/`VirtualList` の項目が動的に増減・並べ替わる場合は、項目固有の永続 ID
//! （例: `WidgetId::from_raw(item.persistent_id)`）を使うこと。

use fandhe_core::{Axis, Node, ReadIndexedFn, Screen, WidgetId};

/// 棚（`Axis::Horizontal` な `List`）1 段あたりの項目数。
const SHELF_LEN: usize = 3;
/// グリッドの列数（`Axis::Grid { columns }`）。
const GRID_COLUMNS: usize = 3;
/// グリッドの総項目数（2 行 + 端数 1 行 = 7 項目、設計文書 §4.4「机上検証」と同型の
/// 不完全最終行を含む構成にし、境界条件の検証を厚くする）。
const GRID_LEN: usize = 7;
/// 大規模 `VirtualList` の総項目数（「1000 件級」の要求、8 節確認事項 8）。
const BIG_LIST_LEN: usize = 1200;
/// ネストしたリスト（外側）のグループ数。各グループの項目がさらに `VirtualList`
/// （内側）を持つ（§4.6 多段 reveal 検証、reviewer 指摘 medium）。
const NESTED_GROUP_COUNT: usize = 3;
/// ネストしたリスト（内側）1 グループあたりの項目数。既定の可視ウィンドウ
/// （overscan 込みで数件〜十数件程度）を超える件数にし、reveal の実地検証を意味あるものにする。
const NESTED_ITEMS_PER_GROUP: usize = 30;
/// ネストしたリスト（外側）1 行の高さ。内側 `VirtualList` 自身の `ScrollArea` が
/// この高さの中に収まる（`tv_focus.rs::render_virtual_list` の行 `Ui` は
/// `max_rect` で高さを固定するため、内側の `ScrollArea` はその範囲内でのみ
/// スクロールする）。内側の項目が数行分見える高さを確保する。
const NESTED_OUTER_ROW_EXTENT: f32 = 130.0;
/// ネストしたリスト（内側）1 項目あたりの高さ。
const NESTED_INNER_ITEM_EXTENT: f32 = 26.0;
/// ネストした葉ノードの id 採番の起点（グループごとに [`NESTED_ITEMS_PER_GROUP`] より
/// 十分広い間隔を空け、既存の棚・グリッド・大規模リストの id 範囲（1..=13、21..=27、
/// 1000..=2199）と重複しない値を選ぶ）。
const NESTED_ID_BASE: u64 = 5000;
/// グループごとの id レンジ幅（[`NESTED_ITEMS_PER_GROUP`] に余裕を持たせた値）。
const NESTED_ID_STRIDE: u64 = 1000;

/// TV サンプル画面が保持する状態。各項目の決定ボタン押下回数のみを持つ最小構成
/// （on_click で状態が変わることを確認できれば十分、タスクの最低要件）。
#[derive(Debug, Clone)]
pub struct TvState {
    /// 棚 1 の各項目の押下回数。
    pub shelf1_counts: Vec<u32>,
    /// 棚 2 の各項目の押下回数。
    pub shelf2_counts: Vec<u32>,
    /// グリッドの各項目の押下回数。
    pub grid_counts: Vec<u32>,
    /// 大規模リストの各項目の押下回数。
    pub big_list_counts: Vec<u32>,
}

impl Default for TvState {
    fn default() -> Self {
        Self {
            shelf1_counts: vec![0; SHELF_LEN],
            shelf2_counts: vec![0; SHELF_LEN],
            grid_counts: vec![0; GRID_LEN],
            big_list_counts: vec![0; BIG_LIST_LEN],
        }
    }
}

fn shelf1_expand(state: &TvState) -> Vec<Node<TvState>> {
    (0..SHELF_LEN)
        .map(|i| Node::IndexedButton {
            label: format!("棚1-{} ({})", i + 1, state.shelf1_counts[i]),
            index: i,
            on_click: shelf1_on_click,
            // WidgetId はツリー全体で一意（設計文書 §3.1）。棚 1 は 1..=SHELF_LEN を使う。
            id: WidgetId::from_raw(1 + i as u64),
        })
        .collect()
}

fn shelf1_on_click(state: &mut TvState, index: usize) {
    if let Some(count) = state.shelf1_counts.get_mut(index) {
        *count = count.saturating_add(1);
    }
}

fn shelf2_expand(state: &TvState) -> Vec<Node<TvState>> {
    (0..SHELF_LEN)
        .map(|i| Node::IndexedButton {
            label: format!("棚2-{} ({})", i + 1, state.shelf2_counts[i]),
            index: i,
            on_click: shelf2_on_click,
            // 棚 2 は 11..=(10+SHELF_LEN) を使う（棚 1 の id 範囲と重複しない）。
            id: WidgetId::from_raw(11 + i as u64),
        })
        .collect()
}

fn shelf2_on_click(state: &mut TvState, index: usize) {
    if let Some(count) = state.shelf2_counts.get_mut(index) {
        *count = count.saturating_add(1);
    }
}

fn grid_len(_state: &TvState) -> usize {
    GRID_LEN
}

fn grid_item(state: &TvState, index: usize) -> Node<TvState> {
    Node::IndexedButton {
        label: format!("grid-{index} ({})", state.grid_counts[index]),
        index,
        on_click: grid_on_click,
        // グリッドは 21..=(20+GRID_LEN) を使う。
        id: WidgetId::from_raw(21 + index as u64),
    }
}

fn grid_on_click(state: &mut TvState, index: usize) {
    if let Some(count) = state.grid_counts.get_mut(index) {
        *count = count.saturating_add(1);
    }
}

fn big_list_len(_state: &TvState) -> usize {
    BIG_LIST_LEN
}

fn big_list_item(state: &TvState, index: usize) -> Node<TvState> {
    Node::IndexedButton {
        label: format!("項目 {index} ({})", state.big_list_counts[index]),
        index,
        on_click: big_list_on_click,
        // 大規模リストは 1000 以降を使う（BIG_LIST_LEN=1200 でも 21..=27 の
        // グリッド id 範囲と衝突しない）。
        id: WidgetId::from_raw(1000 + index as u64),
    }
}

fn big_list_on_click(state: &mut TvState, index: usize) {
    if let Some(count) = state.big_list_counts.get_mut(index) {
        *count = count.saturating_add(1);
    }
}

fn nested_group_len(_state: &TvState) -> usize {
    NESTED_GROUP_COUNT
}

/// 外側リストの `group_index` 番目の項目を構築する。返す値自体が
/// `Node::VirtualList`（1 段ネスト）であり、`item`（内側の項目生成関数）は
/// グループごとに専用の関数ポインタへ切り替える。`Node::VirtualList::item` は
/// 環境キャプチャ不可の素の関数ポインタに限定されるため（`node.rs` の AI 型安全 DSL
/// 方針）、`group_index` をクロージャで捕まえることはできない。代わりに
/// `group_index` ごとに固定 id オフセットを埋め込んだ専用関数
/// （[`nested_group0_item`]/[`nested_group1_item`]/[`nested_group2_item`]）を用意し、
/// この関数（プレーンな `fn`、状態キャプチャなし）が `group_index` の値で選択するだけに
/// 留める。
fn nested_group_item(_state: &TvState, group_index: usize) -> Node<TvState> {
    let item: ReadIndexedFn<TvState, Node<TvState>> = match group_index {
        0 => nested_group0_item,
        1 => nested_group1_item,
        _ => nested_group2_item,
    };
    Node::VirtualList {
        len: nested_inner_len,
        item,
        item_extent: NESTED_INNER_ITEM_EXTENT,
        axis: Axis::Vertical,
    }
}

fn nested_inner_len(_state: &TvState) -> usize {
    NESTED_ITEMS_PER_GROUP
}

/// `id_base` を起点に内側リストの `index` 番目の `IndexedButton` を組み立てる共通処理。
/// `id_base` はグループごとに固定の定数（呼び出し元 `nested_group{N}_item` が
/// リテラルで渡す）であり、状態からのキャプチャではないため関数ポインタ制約に抵触
/// しない。
fn nested_leaf(index: usize, id_base: u64) -> Node<TvState> {
    Node::IndexedButton {
        label: format!("nested-{index}"),
        index,
        on_click: nested_on_click,
        // index 由来の id: 本サンプルは項目の並びが固定（挿入・削除・並べ替えを
        // 行わない）ため実害はないが、実画面では §3.1 が推奨するとおり項目固有の
        // 永続 ID（データの同一性由来）を使うこと。index をそのまま使うと、並べ替え・
        // 挿入時にフォーカス対象の id が意図せず変わり得る（reviewer 指摘 low）。
        id: WidgetId::from_raw(id_base + index as u64),
    }
}

fn nested_group0_item(_state: &TvState, index: usize) -> Node<TvState> {
    nested_leaf(index, NESTED_ID_BASE)
}

fn nested_group1_item(_state: &TvState, index: usize) -> Node<TvState> {
    nested_leaf(index, NESTED_ID_BASE + NESTED_ID_STRIDE)
}

fn nested_group2_item(_state: &TvState, index: usize) -> Node<TvState> {
    nested_leaf(index, NESTED_ID_BASE + NESTED_ID_STRIDE * 2)
}

/// ネストした葉ノードの決定操作ハンドラ。本サンプルは reveal・多段構造の検証が
/// 目的であり、状態更新の確認は棚・グリッド・大規模リストで既に満たしているため
/// no-op とする。
fn nested_on_click(_state: &mut TvState, _index: usize) {}

/// TV フォーカスナビゲーション検証用の 1 画面を構築する。
///
/// トップレベル（`Screen` 自身）は設計文書 §4.4「コンテナと階層」が定める暗黙の
/// `Axis::Vertical` コンテナとして扱われるため、棚・グリッド・大規模リストを
/// 直接トップレベルへ並べるだけで「縦に積んだ構成」になる（ラッパー `List` は不要）。
pub fn tv_sample_screen() -> Screen<TvState> {
    Screen::new("tv-focus-sample")
        .text("TV フォーカスサンプル: 矢印キー/D-pad で移動、Enter/Space/決定ボタンで選択")
        .list_with_axis(shelf1_expand, Axis::Horizontal)
        .list_with_axis(shelf2_expand, Axis::Horizontal)
        .virtual_list_with_axis(
            nested_group_len,
            nested_group_item,
            NESTED_OUTER_ROW_EXTENT,
            Axis::Vertical,
        )
        .virtual_list_with_axis(
            grid_len,
            grid_item,
            48.0,
            Axis::Grid {
                columns: GRID_COLUMNS,
            },
        )
        .virtual_list_with_axis(big_list_len, big_list_item, 32.0, Axis::Vertical)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 画面構築が panic せず、期待するトップレベルノード数（見出し + 棚 2 段 +
    /// ネストしたリスト + グリッド + 大規模リスト = 6）を持つことを確認する
    /// （レンダラ非依存の最小検証）。
    #[test]
    fn tv_sample_screen_has_expected_top_level_node_count() {
        let screen = tv_sample_screen();
        assert_eq!(screen.len(), 6);
    }

    /// 各コンテナの id がツリー全体で重複しないことを確認する（設計文書 §3.1 が
    /// 開発者へ課す一意性契約を、このサンプル画面自身が満たしていることの検証）。
    #[test]
    fn tv_sample_screen_widget_ids_are_unique() {
        let state = TvState::default();
        let mut ids = std::collections::HashSet::new();

        for node in shelf1_expand(&state) {
            let Node::IndexedButton { id, .. } = node else {
                panic!("IndexedButton であるべき");
            };
            assert!(ids.insert(id), "id 重複: {id:?}");
        }
        for node in shelf2_expand(&state) {
            let Node::IndexedButton { id, .. } = node else {
                panic!("IndexedButton であるべき");
            };
            assert!(ids.insert(id), "id 重複: {id:?}");
        }
        for index in 0..GRID_LEN {
            let Node::IndexedButton { id, .. } = grid_item(&state, index) else {
                panic!("IndexedButton であるべき");
            };
            assert!(ids.insert(id), "id 重複: {id:?}");
        }
        // 大規模リストは代表点のみ検証する（全件検証は不要な計算コスト）。
        for index in [0usize, 1, BIG_LIST_LEN - 1] {
            let Node::IndexedButton { id, .. } = big_list_item(&state, index) else {
                panic!("IndexedButton であるべき");
            };
            assert!(ids.insert(id), "id 重複: {id:?}");
        }
        // ネストしたリストは各グループの代表点のみ検証する（全件検証は不要な計算コスト）。
        for group_index in 0..NESTED_GROUP_COUNT {
            let Node::VirtualList { item, .. } = nested_group_item(&state, group_index) else {
                panic!("VirtualList であるべき（1 段ネスト構造）");
            };
            for index in [0usize, NESTED_ITEMS_PER_GROUP - 1] {
                let Node::IndexedButton { id, .. } = item(&state, index) else {
                    panic!("IndexedButton であるべき");
                };
                assert!(ids.insert(id), "id 重複: {id:?}");
            }
        }
    }

    /// 決定操作相当の直接呼び出しで状態が更新されることを確認する
    /// （on_click で状態が変わることの最小検証）。
    #[test]
    fn on_click_updates_state() {
        let mut state = TvState::default();
        shelf1_on_click(&mut state, 0);
        assert_eq!(state.shelf1_counts[0], 1);
        grid_on_click(&mut state, 3);
        assert_eq!(state.grid_counts[3], 1);
        big_list_on_click(&mut state, 999);
        assert_eq!(state.big_list_counts[999], 1);
    }
}
