//! 一覧→詳細の画面遷移スタック（[`crate::ui::ScreenStack`]）を用いた型サンプル
//! 「一覧→詳細」（Issue #730。#726 の最小サンプルを実アプリ型へ拡張）。
//!
//! 文言はダミー、画面レジストリ `crates/fandhe-egui-proxy/src/screens.rs` の
//! `nav "list_detail"` から `FANDHE_PROXY_APP=list_detail` で起動できる。
//!
//! 商品 20 件の一覧（[`crate::ui::Node::VirtualList`] + 行タップで詳細へ遷移）と、
//! 詳細画面でのお気に入りトグルが一覧の表示へ反映されること（状態共有）を示す
//! `docs/ai/SCHEMA.md` パターン 7（`VirtualList`）・パターン 8
//! （[`crate::ui::ScreenStack`]）の実装例。状態型 `ListDetailState` は
//! `nav: ScreenStack<Self>` フィールドを持ち、行ボタンの `on_click` が
//! `nav.push(detail_screen)`、詳細画面の「戻る」ボタン（および `.on_back()`）が
//! `nav.pop()` を呼ぶ（`fn` ポインタのみで遷移・状態更新を表現する、REQ-13）。
//!
//! `ListDetailState::items: Vec<String>`（#726 の最小サンプル）に代えて
//! `products: Vec<Product>` を持つ（互換フィールドは維持しない）。
//! `screens/` は「AI が生成した画面モジュールの置き場」（CLAUDE.md、Issue #653）で
//! `fandhe-core` は `publish = false`（crates.io 未公開）のため外部利用者は存在せず、
//! 本クレート内の呼び出し側（`crates/fandhe-core/tests/list_detail.rs`・
//! `crates/fandhe-egui-proxy/tests/screen_stack_render.rs`）は `products` を参照する。
//! `docs/ai/SCHEMA.md` パターン 8 の `rust,ignore` コード例（`items` のまま）は
//! コンパイル対象外の最小形説明であり、拡張後の実装は本ファイルを参照する旨を記載している。

use crate::ui::{Node, Screen, ScreenStack, WidgetId};

/// 一覧に並ぶダミー商品の件数。
pub const PRODUCT_COUNT: usize = 20;

/// `VirtualList` の 1 行あたりの固定高さ（`docs/architecture/list-virtualization-architecture.md`
/// §3.3 の固定項目高さ契約）。
const ROW_EXTENT: f32 = 32.0;

/// 行ボタンの [`WidgetId`] 採番の基底値。詳細画面のボタン（`WidgetId::from_raw(1)` /
/// `from_raw(2)`）と重ならないよう、行 index とは別区画をあてる
/// （`docs/architecture/tv-focus-navigation-architecture.md` §3.1 の id 運用に合わせる）。
const ROW_ID_BASE: u64 = 1000;

/// ダミー商品 1 件分のデータ。
pub struct Product {
    /// 商品名（ダミー、「商品 01」〜「商品 20」）。
    pub name: String,
    /// 価格（ダミー、100 円刻み）。
    pub price: u32,
    /// 商品説明（ダミー定型文）。
    pub description: String,
    /// お気に入り登録済みかどうか。詳細画面のトグルが書き換え、一覧の行表示へ
    /// 反映される（状態共有の本丸、Issue #730）。
    pub favorite: bool,
}

/// 一覧・詳細の 2 画面が共有する状態。`nav` が現在表示中の画面を管理する
/// （[`crate::ui::ScreenStack`] 参照）。
pub struct ListDetailState {
    /// 画面遷移スタック。既定は一覧画面のみを積んだ状態。
    pub nav: ScreenStack<Self>,
    /// 一覧に表示するダミー商品 20 件。
    pub products: Vec<Product>,
    /// 詳細表示中の商品 index（`products` の範囲内）。
    pub selected: usize,
}

/// ダミー商品 20 件を生成する（`ListDetailState::default` から呼ばれる）。
fn dummy_products() -> Vec<Product> {
    (1..=PRODUCT_COUNT)
        .map(|n| Product {
            name: format!("商品 {n:02}"),
            price: 100 * n as u32,
            description: format!("商品 {n:02} の説明文です。"),
            favorite: false,
        })
        .collect()
}

impl Default for ListDetailState {
    /// `ScreenStack::new` が `fn() -> Screen<Self>` を要求するため、`derive(Default)`
    /// では `nav` を初期化できない（手動実装が必要、screen_stack.rs のモジュール doc
    /// 参照）。
    fn default() -> Self {
        Self {
            nav: ScreenStack::new(list_screen),
            products: dummy_products(),
            selected: 0,
        }
    }
}

/// 商品の行表示ラベルを組み立てる（お気に入り印・商品名・価格）。一覧の行と
/// `render_headless`・結合テストの双方が同じ書式を参照する単一ソース。
pub fn row_label(product: &Product) -> String {
    let mark = if product.favorite { "★" } else { "☆" };
    format!("{mark} {} ¥{}", product.name, product.price)
}

/// `Node::VirtualList::len`（`ReadFn<S, usize>`）。総項目数は常に `products.len()`。
fn product_count(state: &ListDetailState) -> usize {
    state.products.len()
}

/// `Node::VirtualList::item`（`ReadIndexedFn<S, Node<S>>`）。index から 1 行分の
/// `Node::IndexedButton` を生成する。範囲外 index は panic せず案内テキストを返す
/// （coding-rust.md のエラーハンドリング方針。`VirtualList` は `len` の範囲内でのみ
/// `item` を呼ぶ契約だが、呼び出し側の契約違反があっても安全側に倒す）。
fn product_row(state: &ListDetailState, index: usize) -> Node<ListDetailState> {
    match state.products.get(index) {
        Some(product) => Node::IndexedButton {
            label: row_label(product),
            index,
            on_click: open_detail_at,
            id: WidgetId::from_raw(ROW_ID_BASE + index as u64),
        },
        None => Node::Text("(項目なし)".to_string()),
    }
}

/// 行タップで詳細画面へ遷移する（[`ScreenStack::push`]）。`index` が範囲内のときのみ
/// `selected` を更新する（範囲外は状態を変えず据え置き、panic しない）。
///
/// `Node::IndexedButton::on_click` として一覧画面の各行から呼ばれる。
fn open_detail_at(state: &mut ListDetailState, index: usize) {
    if index >= state.products.len() {
        return;
    }
    state.selected = index;
    state.nav.push(detail_screen);
}

/// 一覧画面へ戻る（[`ScreenStack::pop`]）。root（一覧）で呼ばれても panic せず
/// 何もしない（[`ScreenStack::pop`] の契約）。
///
/// 詳細画面の「戻る」ボタン・[`Screen::on_back`]（OS の戻る操作）の双方から
/// 同一ハンドラとして呼ばれる。
fn go_back(state: &mut ListDetailState) {
    state.nav.pop();
}

/// 詳細表示中の商品への参照を取得する。範囲外 index でも panic しない
/// （`get` による据え置き、coding-rust.md のエラーハンドリング方針）。
fn selected_product(state: &ListDetailState) -> Option<&Product> {
    state.products.get(state.selected)
}

/// 詳細画面の商品名表示（`Node::DynamicText::get`）。
fn detail_name(state: &ListDetailState) -> String {
    selected_product(state)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "(商品なし)".to_string())
}

/// 詳細画面の価格表示（`Node::DynamicText::get`）。
fn detail_price(state: &ListDetailState) -> String {
    selected_product(state)
        .map(|p| format!("¥{}", p.price))
        .unwrap_or_default()
}

/// 詳細画面の説明文表示（`Node::DynamicText::get`）。
fn detail_description(state: &ListDetailState) -> String {
    selected_product(state)
        .map(|p| p.description.clone())
        .unwrap_or_default()
}

/// お気に入り状態の表示（`Node::DynamicText::get`）。
fn favorite_label(state: &ListDetailState) -> String {
    match selected_product(state) {
        Some(p) if p.favorite => "お気に入り: ★ 登録済み".to_string(),
        Some(_) => "お気に入り: ☆ 未登録".to_string(),
        None => "お気に入り: -".to_string(),
    }
}

/// 詳細画面の「お気に入りを切り替える」ボタンから呼ばれる。`selected` が範囲外の
/// 場合は何もしない（`get_mut` による据え置き）。一覧へ戻った際、この変更が
/// [`product_row`] の [`row_label`] を通じて即座に反映される（状態共有、Issue #730）。
fn toggle_favorite(state: &mut ListDetailState) {
    if let Some(product) = state.products.get_mut(state.selected) {
        product.favorite = !product.favorite;
    }
}

/// 一覧画面（root）。商品 20 件を `VirtualList`（`Node::List` は egui 側で
/// スクロールしないため使わない。モジュール冒頭参照）で描画する。
pub fn list_screen() -> Screen<ListDetailState> {
    Screen::new("一覧").text("行をタップで詳細へ").virtual_list(
        product_count,
        product_row,
        ROW_EXTENT,
    )
}

/// 詳細画面。商品情報・お気に入りトグル・一覧への「戻る」を持つ。OS の戻る操作
/// （Esc・Android バックキー）も `.on_back(go_back)` で同じハンドラに配線する。
pub fn detail_screen() -> Screen<ListDetailState> {
    Screen::new("詳細")
        .dynamic_text(detail_name)
        .dynamic_text(detail_price)
        .dynamic_text(detail_description)
        .dynamic_text(favorite_label)
        .button(
            WidgetId::from_raw(1),
            "お気に入りを切り替える",
            toggle_favorite,
        )
        .button(WidgetId::from_raw(2), "戻る", go_back)
        .on_back(go_back)
}

/// `state.nav` の現在画面を返す（レジストリ・プラットフォームレイヤの入口、
/// `fn(&S) -> Screen<S>` として毎フレーム呼び直す契約。
/// `docs/ai/SCHEMA.md` パターン 8 参照）。
pub fn current_screen(state: &ListDetailState) -> Screen<ListDetailState> {
    state.nav.current()
}
