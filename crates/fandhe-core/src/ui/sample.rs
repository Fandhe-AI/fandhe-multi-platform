//! 代表 1 画面サンプル（TASK-1.1-S6 / REQ-1）。
//!
//! `Screen<S>` / `Node<S>` の DSL 一式（S1: 型定義・走査基盤、S3: ボタンイベント）を
//! 用いて、テキスト・テキスト入力・ボタン・リストの 4 種の要素を含む 1 画面を
//! 組み立てる公開関数を提供する。REQ-1 の受け入れ基準（「ボタン・リスト・テキストを
//! 含む 1 画面以上」の宣言的記述）を満たすための土台として、`examples/sample_screen.rs`
//! （ヘッドレス動作確認）・`tests/sample_screen.rs`（結合テスト）から利用される。
//!
//! S2（#18）マージにより `.text()` / `.text_field()` ビルダーが追加されたため、
//! 本モジュールは `with_node` + `Node` バリアント直接構築を用いず、`.text()` /
//! `.text_field()` / `.button()` / `.list()` ビルダーと `click_button` のみで
//! 画面を組み立てる（S5: #21 の状態管理・イベント配線は引き続き未対応）。

use super::node::Node;
use super::screen::Screen;
use super::widget::WidgetId;

/// サンプル画面が保持する状態。テキスト入力欄の値（`memo`）・ボタン押下で増加する
/// カウンタ（`count`）・ボタン押下でリストへ増える項目（`items`）を持つ。
///
/// 状態アクセスはすべて関数ポインタ（`fn(&S) -> T` / `fn(&mut S, T)`）経由とし、
/// クロージャによる環境キャプチャを許さない（REQ-13 / NFR-9、node.rs の設計判断を踏襲）。
#[derive(Default)]
pub struct SampleState {
    /// テキスト入力欄が保持する値。
    pub memo: String,
    /// ボタン押下で増加するカウンタ。
    pub count: u32,
    /// ボタン押下で項目が追加されるリスト。
    pub items: Vec<String>,
}

/// テキスト入力欄の現在値を取得する（`Node::TextField::get`）。
pub fn get_memo(state: &SampleState) -> String {
    state.memo.clone()
}

/// テキスト入力欄への入力を状態へ反映する（`Node::TextField::set`）。
pub fn set_memo(state: &mut SampleState, value: String) {
    state.memo = value;
}

/// 「カウント +1」ボタンのハンドラ（`Node::Button::on_click`）。
///
/// panic を API 契約にしない方針（coding-rust.md）に従い、`u32` の桁あふれでも
/// panic せず上限で飽和させる（`event.rs` のテストヘルパーが `saturating_sub`
/// を用いているのと様式を揃える）。
pub fn increment_count(state: &mut SampleState) {
    state.count = state.count.saturating_add(1);
}

/// 「項目を追加」ボタンのハンドラ（`Node::Button::on_click`）。
///
/// 追加する項目名は現在のカウント値から機械的に導出し、外部入力（引数・環境変数・
/// ネットワーク）には一切依存しない（A03 インジェクション対策・security.md 準拠）。
pub fn add_item(state: &mut SampleState) {
    let label = format!("item-{}", state.items.len() + 1);
    state.items.push(label);
}

/// カウント値の表示文字列を取得する（`Node::DynamicText::get`、Issue #581）。
///
/// `.text()` は静的文字列のみを受け付け状態 `S` に追随しないため動的表示には使えない
/// （screen.rs 参照）。以前は 1 件の `Node::Text` を返す `Node::List` で代用していたが、
/// `List` は「動的な子要素一覧」の契約を各バックエンド（`fandhe-web-dom` の
/// `<ul><li>`・`fandhe-swiftui-bridge` の `SnapshotNode::List`）が前提にしており、
/// 単一ラベルがリストとして誤って公開されていた（codex レビュー P1 指摘）。
/// `Node::DynamicText` へ置き換え、状態非依存の `.text()` と対をなす形にする。
pub fn count_label(state: &SampleState) -> String {
    format!("カウント: {}", state.count)
}

/// 状態からリスト表示用の子ノード一覧を生成する（`Node::List::expand`）。
///
/// S4（#20）マージにより `List` の定義は `items: fn(&S) -> Vec<String>` から
/// `expand: fn(&S) -> Vec<Node<S>>` へ変わった。各項目は静的な `Node::Text` へ
/// 変換して返す（子ノードの型は `Node<S>` 自身に固定される、node.rs 参照）。
pub fn expand_items(state: &SampleState) -> Vec<Node<SampleState>> {
    state.items.iter().cloned().map(Node::Text).collect()
}

/// 代表 1 画面サンプルを構築する。
///
/// テキスト（静的見出し）・テキスト入力（メモ欄）・ボタン 2 種（カウント増加・
/// 項目追加）・リスト（状態由来の項目一覧）・動的テキスト（カウント値表示）の
/// 5 種を含む 1 画面を返す。`examples/sample_screen.rs`（ヘッドレス動作確認）・
/// `tests/sample_screen.rs`（結合テスト）の双方から参照される、TASK-1.1-S6 の
/// 中心的な公開 API。
///
/// カウント値表示（`.dynamic_text(count_label)`）は末尾に追加する（Issue #581）。
///
/// 他クレート（`fandhe-swiftui-bridge` / `fandhe-web-dom`）のテストや `docs/ai/SCHEMA.md`
/// の doctest は、本関数の構造（件数・添字・文言）を独立にハードコードせず、戻り値の
/// `len()` / `title()` / 各 `Node` の `get` から都度導出すること。構造を変えたときに
/// 更新する場所は本関数と `tests/sample_screen.rs` の 2 か所に一本化する（Issue #694）。
pub fn sample_screen() -> Screen<SampleState> {
    Screen::new("サンプル画面")
        .text("代表 1 画面サンプル")
        .text_field("メモ", get_memo, set_memo)
        .button(WidgetId::from_raw(1), "カウントを増やす", increment_count)
        .button(WidgetId::from_raw(2), "項目を追加", add_item)
        .list(expand_items)
        .dynamic_text(count_label)
}

// `sample_screen` の要素構成・件数・順序の検証は `tests/sample_screen.rs`
// （`sample_screen_includes_text_button_and_list`）に統合済み。以前は本モジュール
// にもほぼ同内容のユニットテストがあり重複していたため、公開 API のみを用いる
// 結合テスト側に一本化した（Review 指摘対応）。
