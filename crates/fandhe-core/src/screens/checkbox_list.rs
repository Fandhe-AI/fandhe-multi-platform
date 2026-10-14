//! Phase 11 の拡充ループ型サンプル「チェックボックス一覧」（親 #1036、Issue #1037）。
//! `Node::List` の各行に `Checkbox` を 1 つずつ置き、辞書 5 件のオン・オフを扱う。
//! myIme 設定画面が要する「辞書を複数選んでオン・オフする」形の実証。
//!
//! `FANDHE_PROXY_APP=checkbox-list` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` から呼び出されるほか、
//! `crates/fandhe-web-dom/src/demo.rs` の `mount_checkbox_list_screen` からも
//! 同じ [`checkbox_list_screen`] を使う（`screens/mod.rs` の `pub mod checkbox_list;` と対）。
//!
//! `render_headless` は `Node::Widget`（`Checkbox` を含む）の行を出力しない契約
//! （`ui/render.rs` 参照）のため、各行の切り替え結果は画面下部の集約テキスト
//! （[`summary`]）でヘッドレス結合テストから検証する（`settings.rs` と同じ方針）。
//!
//! 行数を定数 5 に固定している理由: [`crate::ui::Checkbox::bound`] は
//! index を持たない `fn` ポインタ（[`crate::state::ReadFn`] / [`crate::state::WriteFn`]）
//! しか受け取らず、`Node::IndexedButton` に相当する index 付き Checkbox は
//! 未提供（REQ-13、環境キャプチャクロージャを型レベルで拒否する設計のため）。
//! 可変行数化は `Node<S>` の設計変更にあたるためスコープ外とする
//! （`.claude/rules/out-of-scope-tracking.md` 準拠）。
//!
//! スクロールの天井: `Node::List` は自前でスクロールしない（`ProxyApp` は
//! `CentralPanel` 直描画でトップレベルに `ScrollArea` を持たない、
//! `list_detail.rs` モジュール doc 参照）。本画面は 5 行 + ボタン 2 + 集約の
//! 固定件数のため画面内に収まる想定で、新しい仕組みは足さない。

use crate::ui::{Checkbox, Node};
use crate::{Screen, WidgetId};

/// 一覧に並ぶ辞書名（固定 5 件）。
pub const DICTIONARY_NAMES: [&str; DICTIONARY_COUNT] = [
    "基本辞書",
    "人名辞書",
    "地名辞書",
    "顔文字辞書",
    "郵便番号辞書",
];

/// 辞書の件数（行数の固定値、モジュール doc 参照）。
pub const DICTIONARY_COUNT: usize = 5;

/// 行の [`WidgetId`] 採番の基底値。「すべて選択」(1) / 「すべて解除」(2) と
/// 区画が重ならないようにする（`list_detail.rs` の `ROW_ID_BASE` と同じ流儀）。
const ROW_ID_BASE: u64 = 100;

/// チェックボックス一覧画面が保持する状態。表示値の SSOT（契約表 6「嘘の成功」対策）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckboxListState {
    /// 辞書ごとのオン・オフ（`DICTIONARY_NAMES` と同じ並び）。
    pub enabled: [bool; DICTIONARY_COUNT],
}

impl Default for CheckboxListState {
    /// 初期値は先頭 3 件（基本辞書・人名辞書・地名辞書）をオンにする。
    fn default() -> Self {
        CheckboxListState {
            enabled: [true, true, true, false, false],
        }
    }
}

/// 行 0（基本辞書）の表示値を返す。
fn get_row_0(state: &CheckboxListState) -> bool {
    state.enabled[0]
}
/// 行 0（基本辞書）の変更を反映する。
fn set_row_0(state: &mut CheckboxListState, value: bool) {
    state.enabled[0] = value;
}
/// 行 1（人名辞書）の表示値を返す。
fn get_row_1(state: &CheckboxListState) -> bool {
    state.enabled[1]
}
/// 行 1（人名辞書）の変更を反映する。
fn set_row_1(state: &mut CheckboxListState, value: bool) {
    state.enabled[1] = value;
}
/// 行 2（地名辞書）の表示値を返す。
fn get_row_2(state: &CheckboxListState) -> bool {
    state.enabled[2]
}
/// 行 2（地名辞書）の変更を反映する。
fn set_row_2(state: &mut CheckboxListState, value: bool) {
    state.enabled[2] = value;
}
/// 行 3（顔文字辞書）の表示値を返す。
fn get_row_3(state: &CheckboxListState) -> bool {
    state.enabled[3]
}
/// 行 3（顔文字辞書）の変更を反映する。
fn set_row_3(state: &mut CheckboxListState, value: bool) {
    state.enabled[3] = value;
}
/// 行 4（郵便番号辞書）の表示値を返す。
fn get_row_4(state: &CheckboxListState) -> bool {
    state.enabled[4]
}
/// 行 4（郵便番号辞書）の変更を反映する。
fn set_row_4(state: &mut CheckboxListState, value: bool) {
    state.enabled[4] = value;
}

/// 行ごとの getter/setter の組（`DICTIONARY_NAMES` と同じ並び・件数）。
/// [`Checkbox::bound`] が index を持たない `fn` ポインタしか受け取らないため、
/// 行ごとに固定の関数を用意して配列にまとめる（モジュール doc 参照）。
type RowAccessor = (
    crate::state::ReadFn<CheckboxListState, bool>,
    crate::state::WriteFn<CheckboxListState, bool>,
);
const ROW_ACCESSORS: [RowAccessor; DICTIONARY_COUNT] = [
    (get_row_0, set_row_0),
    (get_row_1, set_row_1),
    (get_row_2, set_row_2),
    (get_row_3, set_row_3),
    (get_row_4, set_row_4),
];

/// `Node::List::expand`。辞書 5 件ぶんの `Node::Widget(Checkbox)` を組み立てる。
/// 行 path は `[list の位置, i]` で [`crate::ui::set_widget_value_at`] へ届く
/// （モジュール冒頭「使う部品」参照、`event.rs` の `Node::List` 展開契約）。
fn rows(_state: &CheckboxListState) -> Vec<Node<CheckboxListState>> {
    DICTIONARY_NAMES
        .iter()
        .zip(ROW_ACCESSORS.iter())
        .enumerate()
        .map(|(i, (name, (get, set)))| {
            Node::Widget(Box::new(Checkbox::bound(
                WidgetId::from_raw(ROW_ID_BASE + i as u64),
                *name,
                *get,
                *set,
            )))
        })
        .collect()
}

/// 「すべて選択」ボタンのハンドラ。全行をオンにする。
pub fn select_all(state: &mut CheckboxListState) {
    state.enabled = [true; DICTIONARY_COUNT];
}

/// 「すべて解除」ボタンのハンドラ。全行をオフにする。
pub fn clear_all(state: &mut CheckboxListState) {
    state.enabled = [false; DICTIONARY_COUNT];
}

/// 画面下部に表示する集約テキスト（「有効: N / 5 件（名前, …）」形式）。
/// `render_headless` が `Node::Widget` の行を出力しないため、ヘッドレス結合テストは
/// 本関数の出力で各行・一括操作の結果を検証する。
pub fn summary(state: &CheckboxListState) -> String {
    let names: Vec<&str> = DICTIONARY_NAMES
        .iter()
        .zip(state.enabled.iter())
        .filter(|(_, enabled)| **enabled)
        .map(|(name, _)| *name)
        .collect();
    let count = names.len();
    let joined = if names.is_empty() {
        "なし".to_string()
    } else {
        names.join(", ")
    };
    format!("有効: {count} / {DICTIONARY_COUNT} 件（{joined}）")
}

/// チェックボックス一覧画面を組み立てる。`crates/fandhe-egui-proxy/src/screens/registry.rs`
/// から `FANDHE_PROXY_APP=checkbox-list` で、`fandhe-web-dom` の
/// `mount_checkbox_list_screen` から Web で、それぞれ呼び出される公開エントリポイント。
pub fn checkbox_list_screen() -> Screen<CheckboxListState> {
    Screen::new("チェックボックス一覧")
        .text("変換に使う辞書")
        .list(rows)
        .button(WidgetId::from_raw(1), "すべて選択", select_all)
        .button(WidgetId::from_raw(2), "すべて解除", clear_all)
        .text("選択状態")
        .dynamic_text(summary)
}
