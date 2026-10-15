# fandhe-core AI 向けメタ情報（SCHEMA.md）

## 目的

本ファイルは REQ-13（AI 開発への最適化、Must）/ TASK-13.2「AI 向けメタ情報（`SCHEMA.md` 相当）整備」
（`docs/spec/06-roadmap.md` M2）に基づき、AI エージェントが `fandhe-core` の公開 API を用いて
コードを生成する際に参照する入力ファイルである（受け入れ基準: 「メタ情報ありで代表 2 画面の
コンパイル成功率 70% 以上」）。公開 API・型制約・使用パターン・誤りやすい点の 4 部構成とする。

## 基準

本ファイルは `main` の公開 API に同期する（Issue #641）。`cargo test -p fandhe-core --doc` が
本ファイル中の Rust コード例を doctest としてコンパイルするため（`crates/fandhe-core/src/lib.rs` の
`#[cfg(doctest)] #[doc = include_str!(...)] mod schema_md {}`）、公開 API の変更で本ファイルが
陳腐化した場合は `cargo test` が失敗して検知できる。

本ファイルは AI がプロンプト・コンテキストへ丸ごと読み込む前提のため、肥大化させない
（現在サイズ確認: `wc -c docs/ai/SCHEMA.md`）。増補時は以下の順で圧縮を検討する。

1. 冗長な記述を圧縮する
2. 重複するコード例を統合する
3. それでも収まらない場合はセクション分割・タスク種別ごとの同梱を別 Issue で検討する

## 公開 API シグネチャ一覧

### クレートルート再エクスポート（`crates/fandhe-core/src/lib.rs`）

生成タスクで頻出する主要型のみを抜粋する。`token` モジュール（デザイントークン）・
`Widget<S>` トレイト実装の個別ウィジェット型（`Label` / `TextInput` / `Checkbox` 等）・
`state::Event` / `StateStore` は生成対象外として本ファイルでは省略する。

```rust
// クレートルートから import 可能であることを doctest で検証する（コンパイル失敗時に
// 本ファイルの陳腐化を検知する）。
use fandhe_core::{
    Screen, Node, Axis, WidgetId, RenderMode,
    Stack, Grid, Card, Separator,
    click_button, click_button_at, activate_focus_at,
    sample_screen, SampleState,
    ReadFn, WriteFn, ReadIndexedFn, ReadAccessor, WriteAccessor,
    ScreenStack, request_back,
};

fn main() {}
```

### `ui::Node<S>`（`crates/fandhe-core/src/ui/node.rs`）

画面を構成する 1 要素を表す型。状態型 `S` に対してジェネリックであり、異なる状態型のノードを
同一 `Screen<S>` に混在させるとコンパイルエラーになる。`Debug` / `Clone` は手動実装（`S` に
`Debug` / `Clone` 境界を課さないため）。

```rust
use fandhe_core::{Axis, Node, ReadFn, ReadIndexedFn, RgbaPixels, TextRole, WidgetId, Widget, WriteFn};

// `Node<S>`（node.rs）の実バリアント・実フィールド構成を、全バリアント・全フィールドを
// 束縛する網羅 match（`_` を使わない）で検証する。フィールドの追加・削除・型変更が
// あればここでコンパイルエラーになる。
fn describe<S>(node: &Node<S>) -> &'static str {
    match node {
        Node::Text(text) => {
            let _: &String = text;
            "Text"
        }
        Node::TextField { label, get, set } => {
            let _: &String = label;
            let _: &ReadFn<S, String> = get;
            let _: &WriteFn<S, String> = set;
            "TextField"
        }
        Node::Button { label, on_click, id } => {
            let _: &String = label;
            let _: &fn(&mut S) = on_click;
            let _: &WidgetId = id;
            "Button"
        }
        Node::DynamicText { get } => {
            let _: &ReadFn<S, String> = get;
            "DynamicText"
        }
        Node::StyledText { role, get } => {
            let _: &TextRole = role;
            let _: &ReadFn<S, String> = get;
            "StyledText"
        }
        Node::List { expand, axis } => {
            let _: &ReadFn<S, Vec<Node<S>>> = expand;
            let _: &Axis = axis;
            "List"
        }
        Node::VirtualList {
            len,
            item,
            item_extent,
            axis,
        } => {
            let _: &ReadFn<S, usize> = len;
            let _: &ReadIndexedFn<S, Node<S>> = item;
            let _: &f32 = item_extent;
            let _: &Axis = axis;
            "VirtualList"
        }
        Node::IndexedButton {
            label,
            index,
            on_click,
            id,
        } => {
            let _: &String = label;
            let _: &usize = index;
            let _: &WriteFn<S, usize> = on_click;
            let _: &WidgetId = id;
            "IndexedButton"
        }
        Node::Widget(w) => {
            let _: &Box<dyn Widget<S>> = w;
            "Widget"
        }
        Node::RgbaImage { pixels, alt } => {
            let _: &RgbaPixels = pixels;
            let _: &String = alt;
            "RgbaImage"
        }
    }
}

// `Node::expand_children(&self, state: &S) -> Vec<Node<S>>` のシグネチャ検証。
fn assert_expand_children<S>() {
    let _: fn(&Node<S>, &S) -> Vec<Node<S>> = Node::<S>::expand_children;
}

fn main() {
    let node: Node<()> = Node::Text("x".to_string());
    assert_eq!(describe(&node), "Text");
    assert_expand_children::<()>();
}
```

- `Text(String)`: 静的テキスト表示。状態 `S` に依存しない
- `TextField { label, get, set }`: `get` で現在値取得、`set` でユーザー入力を状態へ反映
- `Button { label, on_click, id }`: 押下時に状態 `S` を変更するコールバック。`id` はフォーカス識別・
  決定操作の安定識別子（TV フォーカスナビゲーション等が参照する）
- `DynamicText { get }`: 状態 `S` から都度文字列を読み取る動的テキスト。`Text` との違いは
  「都度評価するかどうか」のみ。単一の動的テキストを `List`（1 件だけの `Text` 展開）で代用しない
- `StyledText { role, get }`: 役割 `TextRole`（`Body` / `Error`）付きの動的テキスト。DSL に色は
  書かせず、色への解決はプラットフォームレイヤが担う。エラー文は `.error_text(get)` を使い、
  `Text` に「エラー:」等の接頭辞を付けない
- `RgbaImage { pixels, alt }`: 実画素（RGBA8）の画像表示。`pixels` は検証済みの `RgbaPixels`
  （`RgbaPixels::new(width, height, rgba)` が長さ不一致・寸法 0・一辺 2048 超で `None`）、`alt` は
  代替テキスト。ビルダーは `.rgba_image(pixels, alt)`
- `List { expand, axis }`: 状態 `S` から子ノード一覧を都度生成する関数ポインタ。子ノードの型は
  `Node<S>` 自身に固定され、ネストした `List` を持てる。`axis` は子要素の配置軸（既定 `Axis::Vertical`）
- `VirtualList { len, item, item_extent, axis }`: index ベースで項目を遅延展開する `List` の
  仮想化版。`len` で総項目数、`item` で index から 1 項目分を生成する
- `IndexedButton { label, index, on_click, id }`: `List::expand` / `VirtualList::item` 内で
  生成する、index を埋め込んだボタン。`click_button` の非展開走査では解決できないため
  `click_button_at` で path 解決して呼び出す
- `Widget(Box<dyn Widget<S>>)`: 共通トレイト `Widget<S>` を実装するウィジェット実体の受け皿
  （`Screen` のウィジェット系ビルダー経由で使う）
- `expand_children(&self, state: &S) -> Vec<Node<S>>`: `List` / `VirtualList` は子ノード一覧を、
  それ以外は空の `Vec` を返す（panic を API 契約にしない）

`Axis`（`crates/fandhe-core/src/ui/node.rs`）は `List` / `VirtualList` の子要素配置軸を表す列挙体。

```rust
use fandhe_core::Axis;

// 全バリアント・全フィールドを束縛する網羅 match で、フィールド構成の一致を検証する。
fn describe(axis: &Axis) -> &'static str {
    match axis {
        Axis::Vertical => "Vertical",
        Axis::Horizontal => "Horizontal",
        Axis::Grid { columns } => {
            let _: &usize = columns;
            "Grid"
        }
    }
}

fn main() {
    assert_eq!(describe(&Axis::default()), "Vertical");
    assert_eq!(describe(&Axis::Grid { columns: 2 }), "Grid");
}
```

### `ui::WidgetId`（`crates/fandhe-core/src/ui/widget.rs`）

ウィジェット実体を画面ツリー上で一意に特定する識別子。`label`（表示文言）とは独立させる。

```rust
use fandhe_core::WidgetId;

fn main() {
    let _: fn(u64) -> WidgetId = WidgetId::from_raw;
    let _: fn(WidgetId) -> u64 = WidgetId::raw;
}
```

- コンストラクタは `WidgetId::from_raw(u64)` のみ。**`WidgetId::new(...)` は存在しない**
- `Screen::button` / `.checkbox()` 等のウィジェット系ビルダーは第 1 引数に `id: WidgetId` を取る。
  1 画面内で重複しない値を割り当てる慣例（同一値の重複自体はコンパイルエラーにはならない）

### `ui::Screen<S>`（`crates/fandhe-core/src/ui/screen.rs`）

1 画面の宣言的 UI 記述。状態型 `S` を固定した `Node<S>` のフラットな並びを保持する。
`Debug` / `Clone` は手動実装。ウィジェット系ビルダー（`label` 以降）は共通して `where S: 'static`
を要求する（借用を持つ状態型は使えない）。

`impl Into<String>` / `impl Widget<S> + 'static` を引数に取るメソッド（`new` / `text` /
`text_field` / `button` / `text_input` / `checkbox` / `radio_button` / `toggle_switch` /
`slider` / `progress_bar` / `image(_bound)` / `icon(_bound)` / `scroll_view` / `dialog` /
`menu` / `tooltip`）は、`impl Trait` 引数が生む匿名ジェネリック型が fn ポインタ coercion と
衝突し `let _: fn(...) = Screen::<S>::method;` へ代入できない（turbofish でも明示不可）ため、
本節は 2 部構成とする: (1) `impl Trait` を取らないメソッド（`title` 等）は fn ポインタ代入、
(2) `impl Trait` を取るメソッドは具体型（`&str` / 実在の `Widget<S>` 実装
`fandhe_core::Label<S>`）を渡す呼び出し形式で検証する。

`visit` / `visit_expanded` / `visit_windowed` / `iter` はジェネリック引数
（`F: FnMut(&Node<S>)`・`impl Iterator`）を取るため fn ポインタ化できず、後述の呼び出し形式で検証する。

```rust
use fandhe_core::{
    Axis, Card, Grid, Label, Node, ReadFn, ReadIndexedFn, RenderMode, Screen, Separator, Stack,
    WidgetId, WriteFn,
};

// 1. impl Trait を取らないメソッド（fn ポインタ代入）。
fn assert_screen_signatures<S: 'static>() {
    let _: fn(&Screen<S>) -> &str = Screen::<S>::title;
    let _: fn(Screen<S>, RenderMode) -> Screen<S> = Screen::<S>::with_render_mode;
    let _: fn(&Screen<S>) -> RenderMode = Screen::<S>::render_mode;
    let _: fn(&Screen<S>) -> usize = Screen::<S>::len;
    let _: fn(&Screen<S>) -> bool = Screen::<S>::is_empty;
    let _: fn(Screen<S>, Node<S>) -> Screen<S> = Screen::<S>::with_node;
    let _: fn(&mut Screen<S>, Node<S>) = Screen::<S>::push;

    // 既存 4 バリアント + DynamicText + StyledText のビルダー（S: 'static 不要。
    // 下記「型制約」節参照）。
    let _: fn(Screen<S>, ReadFn<S, String>) -> Screen<S> = Screen::<S>::dynamic_text;
    let _: fn(Screen<S>, ReadFn<S, String>) -> Screen<S> = Screen::<S>::error_text;
    let _: fn(Screen<S>, ReadFn<S, Vec<Node<S>>>) -> Screen<S> = Screen::<S>::list;
    let _: fn(Screen<S>, ReadFn<S, Vec<Node<S>>>, Axis) -> Screen<S> = Screen::<S>::list_with_axis;
    let _: fn(Screen<S>, ReadFn<S, usize>, ReadIndexedFn<S, Node<S>>, f32) -> Screen<S> =
        Screen::<S>::virtual_list;
    let _: fn(Screen<S>, ReadFn<S, usize>, ReadIndexedFn<S, Node<S>>, f32, Axis) -> Screen<S> =
        Screen::<S>::virtual_list_with_axis;

    // ウィジェット系ビルダー（Node::Widget 経由。いずれも where S: 'static）。
    let _: fn(Screen<S>, WidgetId, ReadFn<S, String>) -> Screen<S> = Screen::<S>::label;
    let _: fn(Screen<S>, WidgetId, ReadFn<S, usize>, ReadIndexedFn<S, String>) -> Screen<S> =
        Screen::<S>::list_view;
    let _: fn(Screen<S>, WidgetId) -> Screen<S> = Screen::<S>::spinner;
    let _: fn(Screen<S>, Stack<S>) -> Screen<S> = Screen::<S>::stack;
    let _: fn(Screen<S>, Grid<S>) -> Screen<S> = Screen::<S>::grid;
    let _: fn(Screen<S>, WidgetId, Vec<String>, ReadFn<S, usize>) -> Screen<S> = Screen::<S>::tabs;
    let _: fn(Screen<S>, Card<S>) -> Screen<S> = Screen::<S>::card;
    let _: fn(Screen<S>, Separator<S>) -> Screen<S> = Screen::<S>::separator;
}

// 2. impl Trait を取るメソッド（呼び出し形式）。状態型は () に固定し、具体型の
// ReadFn<(), T> / WriteFn<(), T> / fn(&mut ()) は本節専用の最小関数で用意する。
fn get_string(_state: &()) -> String {
    String::new()
}
fn set_string(_state: &mut (), _value: String) {}
fn get_bool(_state: &()) -> bool {
    false
}
fn set_bool(_state: &mut (), _value: bool) {}
fn get_usize(_state: &()) -> usize {
    0
}
fn set_usize(_state: &mut (), _value: usize) {}
fn get_f32(_state: &()) -> f32 {
    0.0
}
fn set_f32(_state: &mut (), _value: f32) {}
fn get_option_usize(_state: &()) -> Option<usize> {
    None
}
fn on_click(_state: &mut ()) {}

fn build_screen_via_impl_trait_builders() -> Screen<()> {
    let anchor: Label<()> = Label::fixed(WidgetId::from_raw(0), "anchor");
    Screen::new("t")
        .text("見出し")
        .text_field("label", get_string, set_string)
        .button(WidgetId::from_raw(1), "label", on_click)
        .text_input(WidgetId::from_raw(2), "label", get_string, set_string)
        .checkbox(WidgetId::from_raw(3), "label", get_bool, set_bool)
        .radio_button(WidgetId::from_raw(4), "label", 0, get_usize, set_usize)
        .toggle_switch(WidgetId::from_raw(5), "label", get_bool, set_bool)
        .slider(WidgetId::from_raw(6), "label", 0.0, 1.0, get_f32, set_f32)
        .progress_bar(WidgetId::from_raw(7), "label", get_f32)
        .image(WidgetId::from_raw(8), "src", "alt")
        .image_bound(WidgetId::from_raw(9), get_string, "alt")
        .icon(WidgetId::from_raw(10), "name", "label")
        .icon_bound(WidgetId::from_raw(11), get_string, "label")
        .scroll_view(WidgetId::from_raw(12), anchor.clone())
        .dialog(WidgetId::from_raw(13), "title", "body", get_bool)
        .menu(
            WidgetId::from_raw(14),
            "label",
            vec!["a".to_string()],
            get_bool,
            get_option_usize,
        )
        .tooltip(WidgetId::from_raw(15), "text", anchor)
}

fn main() {
    assert_screen_signatures::<()>();
    assert_eq!(build_screen_via_impl_trait_builders().len(), 17);
}
```

走査 API（`visit` / `visit_expanded` / `visit_windowed` / `iter`）はジェネリック引数
（`F: FnMut(&Node<S>)` / `impl Iterator`）を取るため fn ポインタ化できず、呼び出し形式で検証する。
`VisibleRange` は `std::ops::Range<usize>` の型エイリアス。

```rust
use fandhe_core::{Node, Screen, VisibleRange};

fn items(_state: &()) -> Vec<Node<()>> {
    vec![Node::Text("a".into()), Node::Text("b".into())]
}

fn assert_iter<'a, S: 'a>(it: impl Iterator<Item = &'a Node<S>>) -> usize {
    it.count()
}

let screen: Screen<()> = Screen::new("t").text("head").list(items);
let mut n = 0;
screen.visit(|_node: &Node<()>| n += 1);
assert_eq!(n, 2); // List は展開しない
let mut n = 0;
screen.visit_expanded(&(), |_node: &Node<()>| n += 1);
assert_eq!(n, 4); // List + 子 2 件
let range: VisibleRange = 0..1;
let mut n = 0;
screen.visit_windowed(&(), &range, |_node: &Node<()>| n += 1);
assert_eq!(n, 4); // VirtualList 以外は visit_expanded と同じ
assert_eq!(assert_iter(screen.iter()), 2);
```

- `new` / `with_node`（self 消費のビルダー形式）/ `push`（可変参照版）: ノードの追加
- `text` / `dynamic_text` / `error_text` / `text_field` / `button` / `list` / `virtual_list`
  （`_with_axis` 版含む）: 既存 4 バリアント + `DynamicText` + `StyledText` の糖衣構文。
  `S: 'static` は不要。`error_text(get)` は `TextRole::Error` を持つ `StyledText` を追加する
  （色は書かない。色解決はプラットフォームレイヤの契約。「エラー:」等の接頭辞を `Text` に
  付けない）
- `rgba_image(pixels, alt)`: `Node::RgbaImage` の糖衣構文（`S: 'static` 不要）
- `label` 以降のウィジェット系ビルダーは `Node::Widget(Box<dyn Widget<S>>)` を組み立てる糖衣構文で、
  いずれも `where S: 'static` を要求する。第 1 引数は 2 系統に分かれる
  - `id: WidgetId` を直接取るもの: `label` / `text_input` / `list_view` / `checkbox` /
    `radio_button` / `toggle_switch` / `slider` / `progress_bar` / `spinner` / `image(_bound)` /
    `icon(_bound)` / `scroll_view` / `dialog` / `menu` / `tabs` / `tooltip`
  - 組み立て済みのコンテナ型を取るもの（`WidgetId` はコンストラクタ側へ渡す）:
    `stack(Stack<S>)` / `grid(Grid<S>)` / `card(Card<S>)` / `separator(Separator<S>)`
- `visit`: 挿入順の単純走査。`List` / `VirtualList` の中身は展開しない
- `visit_expanded`: `List` / `VirtualList` を `state` で展開して深さ優先に走査する
  （プラットフォームレイヤの描画走査が使う想定）
- `visit_windowed`: `VirtualList` の可視範囲 `range` 内の index に限定して `item` を呼ぶ
  性能経路（300 項目規模のリスト等）
- `iter`: 挿入順のイテレータ（内部実装型を露出しない `impl Iterator`）
- `on_back(handler: fn(&mut S)) -> Self` / `back_handler() -> Option<fn(&mut S)>`
  （Issue #726）: OS の戻る操作を受けたときのハンドラ。`state::Event::BackRequested` →
  `ui::request_back` 経由で発火する（パターン 8 参照）。未設定（既定）の画面では
  戻る操作に応答しない
- **`.dialog()`（`Dialog<S>`、S46）は AX ノード公開のみで、描画・開閉イベントは未配線**
  （egui-proxy の実描画・`render_headless` のどちらからも呼ばれない）。実際に描画される
  ダイアログは、`DialogWidget::new_dynamic(..).with_content(expand_fn)`（`DialogWidget`
  S47、`ui::dialog_widget`）を `Node::Widget` として `.with_node()` で置いたものに限る。
  egui-proxy はこれを前面パネル + スクリムで描画する。外側タップは、`with_content` の
  子ノード列の中に `dismiss_label` と同名の `Node::Button` があるときに限り、その
  `Button` の path へ `Event::ButtonClickedAt` を dispatch する（Issue #739）。同名の
  `Button` が子ノード列に無ければ外側タップは何もしない（トップレベルの `.button()` への
  フォールバックは無い）。参照実装: `crates/fandhe-core/src/screens/confirm_dialog.rs`

### `Stack<S>` / `Grid<S>` / `Card<S>` / `Separator<S>`（`ui/stack_grid.rs`・`ui/card.rs`）

複数子・複雑な構成を持つコンテナ型。`Screen::stack` / `.grid` / `.card` / `.separator` へ渡す前に
組み立てる。いずれも `impl<S: 'static>` の下で定義され、コンストラクタ + 主要ビルダーのみ記載する
（`Widget<S>` トレイト実装の詳細は本ファイルの対象外）。

`FlexDirection` はクレートルートへ再エクスポートされておらず `fandhe_core::ui::FlexDirection`
を使う（`Stack` の主軸方向）。

```rust
use fandhe_core::ui::FlexDirection;
use fandhe_core::{Card, Grid, Label, Separator, SeparatorOrientation, Stack, WidgetId};

fn assert_stack_signatures<S: 'static>() {
    let _: fn(WidgetId) -> Stack<S> = Stack::<S>::new;
    let _: fn(Stack<S>, FlexDirection) -> Stack<S> = Stack::<S>::with_direction;
    let _: fn(Stack<S>, f32) -> Stack<S> = Stack::<S>::with_spacing;
    let _: fn(Stack<S>, Label<S>) -> Stack<S> = Stack::<S>::with_child;
}

fn assert_grid_signatures<S: 'static>() {
    let _: fn(WidgetId) -> Grid<S> = Grid::<S>::new;
    let _: fn(Grid<S>, usize) -> Grid<S> = Grid::<S>::with_columns;
    let _: fn(Grid<S>, f32) -> Grid<S> = Grid::<S>::with_spacing;
    let _: fn(Grid<S>, Label<S>) -> Grid<S> = Grid::<S>::with_child;
}

fn assert_card_signatures<S: 'static>() {
    let _: fn(WidgetId) -> Card<S> = Card::<S>::new;
    let _: fn(Card<S>, Label<S>) -> Card<S> = Card::<S>::with_child;
    let _: fn(Card<S>, f32) -> Card<S> = Card::<S>::with_padding;
    // `impl Into<String>` は turbofish で明示できないため、具体型 `&str` を渡す呼び出し
    // 形式でシグネチャを検証する。
    let card: Card<S> = Card::new(WidgetId::from_raw(0)).with_tap_label("tap");
    let _ = card;
}

fn assert_separator_signatures<S: 'static>() {
    let _: fn(WidgetId) -> Separator<S> = Separator::<S>::new;
    let _: fn(Separator<S>, SeparatorOrientation) -> Separator<S> = Separator::<S>::with_orientation;
    let _: fn(Separator<S>, f32) -> Separator<S> = Separator::<S>::with_thickness;
}

fn main() {
    assert_stack_signatures::<()>();
    assert_grid_signatures::<()>();
    assert_card_signatures::<()>();
    assert_separator_signatures::<()>();
}
```

### `ui::event`（`crates/fandhe-core/src/ui/event.rs`）

```rust
use fandhe_core::{
    Screen, WidgetId, activate_focus_at, click_button, click_button_at, request_back,
};

fn assert_event_signatures<S>() {
    let _: fn(&Screen<S>, &str, &mut S) -> bool = click_button;
    let _: fn(&Screen<S>, &[usize], &str, &mut S) -> bool = click_button_at;
    let _: fn(&Screen<S>, &[usize], WidgetId, &mut S) -> bool = activate_focus_at;
    let _: fn(&Screen<S>, &mut S) -> bool = request_back;
}

fn main() {
    assert_event_signatures::<()>();
}
```

- `click_button`: ラベル一致で `Button` / `IndexedButton` を探索し `on_click` を呼ぶヘルパー。
  `Screen::iter`（非展開走査）経由のため `List` / `VirtualList` の中身は辿らない。見つからなければ
  `false`。同一ラベル複数時は先頭の 1 件のみ発火
- `click_button_at`: `path`（トップレベル挿入順 index → 展開順 index）を降下して
  `List` / `VirtualList` 中の `Button` / `IndexedButton` を解決し、ラベル一致時のみ発火する
  （動的生成子内のボタンを解決する経路）
- `activate_focus_at`: `click_button_at` の `id`（`WidgetId`）版。ラベルの代わりに `id` 一致で判定
- `request_back`（Issue #726）: `screen.back_handler()`（`.on_back()` で設定したハンドラ）を
  発火する。未設定なら状態を変更せず `false`

### `ui::sample`（`crates/fandhe-core/src/ui/sample.rs`）

`Screen<S>` / `Node<S>` DSL の使用実例（代表 1 画面サンプル）を提供する公開関数群。
`SampleState` / `sample_screen` はクレートルートへ再エクスポートされているが、
`get_memo` / `set_memo` / `increment_count` / `add_item` / `expand_items` はルートには
再エクスポートされておらず `fandhe_core::ui::` 経由（`ui/mod.rs` の `pub use sample::{..}`）
で import する。**`count_label` は非公開のためクレート外から import できない**
（`sample_screen()` 内部専用の実装詳細。詳細後述）。

```rust
use fandhe_core::ui::{add_item, expand_items, get_memo, increment_count, set_memo};
use fandhe_core::{Node, SampleState, Screen, sample_screen};

fn main() {
    let _: fn() -> SampleState = SampleState::default;
    let _: fn(&SampleState) -> String = get_memo;
    let _: fn(&mut SampleState, String) = set_memo;
    let _: fn(&mut SampleState) = increment_count;
    let _: fn(&mut SampleState) = add_item;
    let _: fn(&SampleState) -> Vec<Node<SampleState>> = expand_items;
    let _: fn() -> Screen<SampleState> = sample_screen;
}
```

`sample_screen()` と同じ構成で 1 画面を組み立て `render_headless`（パターン 7）で検証する
自己完結の例（要素数と照合しない。構造変更の更新先は `sample.rs`/`tests/sample_screen.rs`
に一本化、#694）。`count_label` は非公開のため同一シグネチャのクロージャで代用する。

```rust
use fandhe_core::ui::{add_item, expand_items, get_memo, increment_count, set_memo};
use fandhe_core::{Screen, SampleState, WidgetId, render_headless};

fn main() {
    let built: Screen<SampleState> = Screen::new("サンプル画面")
        .text("代表 1 画面サンプル")
        .text_field("メモ", get_memo, set_memo)
        .button(WidgetId::from_raw(1), "カウントを増やす", increment_count)
        .button(WidgetId::from_raw(2), "項目を追加", add_item)
        .list(expand_items)
        .dynamic_text(|s: &SampleState| format!("カウント: {}", s.count));
    let lines = render_headless(&built, &SampleState::default());
    assert_eq!(lines[0], "# サンプル画面");
    assert!(lines.contains(&"カウント: 0".to_string()));
}
```

- `get_memo` / `set_memo`: `Node::TextField` の `get` / `set` の実例
- `increment_count`: `Node::Button::on_click` の実例。`u32` の桁あふれは panic せず飽和させる
- `add_item`: 追加項目名は現在の項目数（`items.len() + 1`）から機械的に導出する
- `count_label`: `Node::DynamicText::get` の実例。非公開のため AI 生成コードから直接利用できない。
  動的テキストが必要な場合は同シグネチャ（`fn(&S) -> String`）の関数を自前で定義する
- `expand_items`: `Node::List::expand` の実例。状態の `items` を `Node::Text` へ変換して返す

### `state`（`crates/fandhe-core/src/state/{read,write}.rs`。`store.rs` の `Event` / `StateStore` は対象外）

`ui` モジュールの状態アクセス用関数ポインタ表記を集約する状態管理レイヤ共通の型 SSOT。

```rust
use fandhe_core::{ReadAccessor, ReadFn, ReadIndexedFn, WriteAccessor, WriteFn};

// 型エイリアスの実体（`fn(&S) -> T` 等）と一致することを、恒等関数的な型検査で確認する
// （引数の型を経由して型エイリアス ⇔ 具体 fn シグネチャの一致をコンパイラに検証させる）。
fn assert_read_fn_shape<S, T>(f: ReadFn<S, T>) -> fn(&S) -> T {
    f
}
fn assert_read_indexed_fn_shape<S, T>(f: ReadIndexedFn<S, T>) -> fn(&S, usize) -> T {
    f
}
fn assert_write_fn_shape<S, T>(f: WriteFn<S, T>) -> fn(&mut S, T) {
    f
}

fn assert_accessor_signatures<S, T>() {
    let _: fn(ReadFn<S, T>) -> ReadAccessor<S, T> = ReadAccessor::<S, T>::new;
    let _: fn(&ReadAccessor<S, T>, &S) -> T = ReadAccessor::<S, T>::get;
    let _: fn(&ReadAccessor<S, T>) -> ReadFn<S, T> = ReadAccessor::<S, T>::as_fn;

    let _: fn(WriteFn<S, T>) -> WriteAccessor<S, T> = WriteAccessor::<S, T>::new;
    let _: fn(&WriteAccessor<S, T>, &mut S, T) = WriteAccessor::<S, T>::set;
    let _: fn(&WriteAccessor<S, T>) -> WriteFn<S, T> = WriteAccessor::<S, T>::as_fn;
}

fn main() {
    let _ = assert_read_fn_shape::<(), ()>;
    let _ = assert_read_indexed_fn_shape::<(), ()>;
    let _ = assert_write_fn_shape::<(), ()>;
    assert_accessor_signatures::<(), ()>();
}
```

`ReadAccessor` / `WriteAccessor` の `Debug` / `Clone` / `Copy` は手動実装（`S` に境界を課さない）。

- `ReadFn<S, T>` / `WriteFn<S, T>`: 状態アクセスの型表記の SSOT。`Node::TextField::get`
  （`fn(&S) -> String`）・`set`（`fn(&mut S, String)`）と同一表記
- `ReadIndexedFn<S, T>`: `fn(&S, usize) -> T`。`Node::VirtualList::item` の型表記
- `ReadAccessor` / `WriteAccessor`: 上記関数ポインタを保持する名前付きアクセサ。`get` / `set`
  で読み書きし、`as_fn` で生の関数ポインタへ橋渡しする

## 型制約（AI が守るべきルール）

- **状態アクセスは関数ポインタ限定**: `fn(&S) -> T` / `fn(&mut S, T)` / `fn(&mut S)` のみを
  受け付ける。環境をキャプチャするクロージャは `fn` へ coerce できずコンパイルエラーになる
  （REQ-13 / NFR-9）。非キャプチャクロージャは coerce 可能なため受理される
- **`Screen<S>` は 1 画面 1 状態型**: 異なる状態型 `S` のノードを同一 `Screen<S>` に混在させると
  型不一致でコンパイルエラーになる
- **ビルダーは self 消費のメソッドチェーン**: `Screen::new` → `.button(id, ..)` / `.list(..)` /
  `.with_node(..)` の順で連結する。可変参照版が必要な場合のみ `push` を使う
- **`id: WidgetId` を第 1 引数に取るビルダーのコンストラクタは `from_raw` のみ**: `button`
  （`Node::Button` の DSL 系）・`checkbox` / `slider` 等（ウィジェット系）はいずれも
  `WidgetId::from_raw(n)` を第 1 引数に渡す。`WidgetId::new(...)` は存在しない
- **`S: 'static` が必須なのはウィジェット系ビルダーのみ**: `label` 以降のウィジェット系ビルダー
  （Screen 節の一覧参照）は `where S: 'static` を要求する。既存 4 バリアント系ビルダー（`text` /
  `dynamic_text` / `error_text` / `text_field` / `button` / `list(_with_axis)` /
  `virtual_list(_with_axis)`）は要求しない。借用を持つ状態型（ライフタイムパラメータを含む `S`）
  はウィジェット系ビルダーへ
  渡せない
- **`List` の `expand` は `Vec<Node<S>>` を返す**: 子ノードの型は `Node<S>` 自身に固定されるため、
  ネストした `List` を子要素に持てる。展開は静的な一度きりの計算ではなく、走査
  （`visit_expanded` / `expand_children`）のたびに `expand(state)` が再評価される
- **型表記の SSOT は `state::ReadFn` / `state::WriteFn` / `state::ReadIndexedFn`**: `ui` モジュール
  の生 `fn` ポインタ（`Node::TextField::get` 等）とシグネチャは同一であり、新規コードで型注釈が
  必要な場合はこれらを用いる
- **panic を API 契約にしない**: `Node::expand_children` は `List` / `VirtualList` 以外の
  バリアントに対して空の `Vec` を返す。`click_button` / `click_button_at` / `activate_focus_at`
  はいずれも不一致・範囲外時に `false` を返す。いずれも panic しない
- **`StateStore::apply` は「状態アクセスは関数ポインタ限定」ルールの対象外**（Issue #745）:
  `apply` は外部 I/O 完了の反映専用の別経路で、`&mut S` を直接キャプチャするクロージャを
  受け付ける。`get` / `set` / `on_click` / `expand`（`Node` フィールド）が関数ポインタ限定
  であることの根拠には一切ならない。`apply` の存在を理由に `Node` 側へクロージャを渡そうと
  しないこと（パターン 9 参照）

## 使用パターン・コード例集

各パターンは `use fandhe_core::...` から始まる、模倣すればコンパイルが通る完結スニペット。

### パターン 1: 最小画面（カウンタ）

状態 struct を定義し `Screen::new` → `.button(id, label, fn)` で組み立て、`click_button` で
押下を発火して状態を確認する最小構成。

```rust
use fandhe_core::{Screen, WidgetId, click_button};

#[derive(Default)]
struct CounterState {
    count: u32,
}

fn increment(state: &mut CounterState) {
    state.count += 1;
}

fn main() {
    let screen: Screen<CounterState> =
        Screen::new("カウンタ画面").button(WidgetId::from_raw(1), "+1", increment);
    let mut state = CounterState::default();

    assert!(click_button(&screen, "+1", &mut state));
    assert_eq!(state.count, 1);
}
```

### パターン 2: テキスト入力（TextField）・動的テキスト（DynamicText）

`.text_field(label, get, set)` はフィールド名 `label` / `get` / `set` を過不足なく指定する。
`.dynamic_text(get)` は状態変化に追随する表示に使う（`.text()` は静的文字列専用）。

```rust
use fandhe_core::{Node, Screen};

#[derive(Default)]
struct FormState {
    memo: String,
}

fn get_memo(state: &FormState) -> String {
    state.memo.clone()
}

fn set_memo(state: &mut FormState, value: String) {
    state.memo = value;
}

fn main() {
    let screen: Screen<FormState> = Screen::new("入力画面")
        .text_field("メモ", get_memo, set_memo)
        .dynamic_text(get_memo);

    let mut state = FormState::default();
    set_memo(&mut state, "更新後".to_string());

    let mut seen_dynamic = false;
    screen.visit(|node| match node {
        Node::TextField { label, get, .. } => {
            assert_eq!(label, "メモ");
            assert_eq!(get(&state), "更新後");
        }
        Node::DynamicText { get } => {
            assert_eq!(get(&state), "更新後");
            seen_dynamic = true;
        }
        _ => {}
    });
    assert!(seen_dynamic);
}
```

エラー文は `.error_text(get)` を使う。色は書かない・`Text` に「エラー:」等の接頭辞を付けない。

```rust
use fandhe_core::{Node, Screen, TextRole};

fn get_error(_state: &()) -> String {
    "入力してください".to_string()
}

fn main() {
    let screen: Screen<()> = Screen::new("入力画面").error_text(get_error);

    assert_eq!(
        fandhe_core::render_headless(&screen, &()),
        vec!["# 入力画面".to_string(), "入力してください".to_string()]
    );

    screen.visit(|node| {
        if let Node::StyledText { role, .. } = node {
            assert_eq!(*role, TextRole::Error);
        }
    });
}
```

### パターン 3: リスト展開（`expand: fn(&S) -> Vec<Node<S>>`）

状態から `Node::Text` の列を生成する基本形と `Node::List` 自身を子要素とするネスト例。
展開は `expand(state)` の呼び出しごとに再評価される。

```rust
use fandhe_core::{Axis, Node, Screen};

#[derive(Default)]
struct ListState {
    items: Vec<String>,
}

fn expand_items(state: &ListState) -> Vec<Node<ListState>> {
    state.items.iter().cloned().map(Node::Text).collect()
}

// ネスト例: 子要素として Node::List を返すこともできる。
fn expand_nested(_state: &ListState) -> Vec<Node<ListState>> {
    vec![
        Node::Text("見出し".to_string()),
        Node::List {
            expand: expand_items,
            axis: Axis::default(),
        },
    ]
}

fn main() {
    let screen: Screen<ListState> = Screen::new("リスト画面").list(expand_items);
    let state = ListState {
        items: vec!["a".to_string(), "b".to_string()],
    };

    let mut visited = Vec::new();
    screen.visit_expanded(&state, |node| {
        if let Node::Text(text) = node {
            visited.push(text.clone());
        }
    });
    assert_eq!(visited, vec!["a", "b"]);
    // expand_nested を使う場合は screen.list(expand_nested) のように差し替える。
    let _ = expand_nested;
}
```

### パターン 4: 画面の組み立てスタイル

`Screen<S>` は self 消費のメソッドチェーンと可変参照版 `push` の 2 通りの組み立て方を提供する。

```rust
use fandhe_core::{Axis, Node, Screen, WidgetId};

#[derive(Default)]
struct AppState {
    count: u32,
}

fn increment(state: &mut AppState) {
    state.count += 1;
}

fn expand_none(_state: &AppState) -> Vec<Node<AppState>> {
    Vec::new()
}

fn main() {
    // self 消費のメソッドチェーン: new → with_node → button → list
    let chained: Screen<AppState> = Screen::new("チェーン画面")
        .with_node(Node::Text("見出し".to_string()))
        .button(WidgetId::from_raw(1), "+1", increment)
        .list(expand_none);
    assert_eq!(chained.len(), 3);

    // 可変参照版 push を使う組み立て（途中で条件分岐を挟みたい場合に向く）。
    let mut pushed: Screen<AppState> = Screen::new("push 画面");
    pushed.push(Node::Text("見出し".to_string()));
    pushed.push(Node::Button {
        label: "+1".to_string(),
        on_click: increment,
        id: WidgetId::from_raw(2),
    });
    assert_eq!(pushed.len(), 2);
    let _: Axis = Axis::default();
}
```

### パターン 5: 走査（`visit` / `visit_expanded` / `iter`）

- `visit`: `List` を展開しない単純走査（トップレベルのノードのみ）
- `visit_expanded(state, f)`: `List` を展開し、深さ優先で子ノードまで辿る
- `iter()`: 挿入順のイテレータ（`List` は展開しない）
- 大量項目（300 件規模）のリストで可視範囲のみを走査したい性能経路には `Screen::virtual_list` +
  `Screen::visit_windowed` を使う（`VisibleRange` の算出は `ui::compute_visible_range` を参照）
- 描画確認はパターン 7 の `render_headless` を使う（走査 API で件数だけ数えない）

### パターン 6: `state` アクセサとの接続

`ReadAccessor::new` / `WriteAccessor::new` で名前付き関数から読み書きアクセサを作り、
`as_fn()` で `Node::TextField` が要求する生の `fn` ポインタへ橋渡しする。

```rust
use fandhe_core::{Node, ReadAccessor, ReadFn, WriteAccessor};

struct FormState {
    memo: String,
}

fn read_memo(state: &FormState) -> String {
    state.memo.clone()
}

fn write_memo(state: &mut FormState, value: String) {
    state.memo = value;
}

fn main() {
    let read_accessor: ReadAccessor<FormState, String> = ReadAccessor::new(read_memo);
    let write_accessor: WriteAccessor<FormState, String> = WriteAccessor::new(write_memo);

    let mut state = FormState {
        memo: "初期値".to_string(),
    };
    assert_eq!(read_accessor.get(&state), "初期値");
    write_accessor.set(&mut state, "更新後".to_string());
    assert_eq!(read_accessor.get(&state), "更新後");

    // as_fn() で Node::TextField の生 fn ポインタへ橋渡しする。
    let node: Node<FormState> = Node::TextField {
        label: "メモ".to_string(),
        get: read_accessor.as_fn(),
        set: write_accessor.as_fn(),
    };
    if let Node::TextField { get, .. } = node {
        assert_eq!(get(&state), "更新後");
    }

    // 型注釈が必要な場合の SSOT: state::ReadFn<S, T> を使う（WriteFn<S, T> も同様）。
    let get_fn: ReadFn<FormState, String> = read_memo;
    assert_eq!(get_fn(&state), "更新後");
}
```

### パターン 7: 結合テスト方針（`render_headless` 出力で assert）と `click_button_at`

- 結合テストは `render_headless(&screen, &state)` の `Vec<String>`（`# タイトル` /
  `[ラベル: 値]` / `(ボタン)` / 行テキスト）に期待行を含むか assert する。件数だけ数えない。
  操作後も再度呼び assert する（Issue #705）
- `List::expand` / `VirtualList::item` の `Node::IndexedButton` は `click_button` では解決できない。
  `click_button_at(&screen, &[list の挿入順, 展開順], label, &mut state)` を使う
- 画面実装は `src/screens/<name>.rs`（クレート内部なので `use crate::{...}`。`fandhe_core::` は
  `extern crate self` 未宣言のため解決不可）、結合テストは `tests/<name>.rs`（別クレートなので
  `use fandhe_core::screens::<name>::{...}`）に分け、`fn main()` ではなく `#[test] fn` で書く

```rust,ignore
// src/screens/task.rs（doctest 不可）
use crate::{Node, Screen, WidgetId};

#[derive(Default)]
pub struct TaskState {
    pub input: String,
    pub tasks: Vec<(String, bool)>,
}

pub fn get_input(state: &TaskState) -> String { state.input.clone() }
pub fn set_input(state: &mut TaskState, value: String) { state.input = value; }
pub fn add_task(state: &mut TaskState) {
    if !state.input.is_empty() {
        state.tasks.push((std::mem::take(&mut state.input), false));
    }
}
pub fn toggle_done(state: &mut TaskState, index: usize) {
    if let Some(task) = state.tasks.get_mut(index) {
        task.1 = !task.1;
    }
}
const DYNAMIC_ID_BASE: u64 = 1000;

pub fn expand_tasks(state: &TaskState) -> Vec<Node<TaskState>> {
    let items = state.tasks.iter().enumerate().flat_map(|(i, (title, done))| {
        [
            Node::Text(format!("[{}] {title}", if *done { "x" } else { " " })),
            Node::IndexedButton {
                label: "完了".to_string(),
                index: i,
                on_click: toggle_done,
                id: WidgetId::from_raw(DYNAMIC_ID_BASE + i as u64),
            },
        ]
    });
    items.collect()
}

pub fn task_screen() -> Screen<TaskState> {
    Screen::new("タスク管理")
        .text_field("タスク", get_input, set_input)
        .button(WidgetId::from_raw(1), "追加", add_task)
        .list(expand_tasks)
}
```

```rust,ignore
// tests/task.rs
use fandhe_core::screens::task::{TaskState, set_input, task_screen};
use fandhe_core::{click_button, click_button_at, render_headless};

#[test]
fn task_screen_renders_and_handles_actions() {
    let screen = task_screen();
    let mut state = TaskState::default();

    let lines = render_headless(&screen, &state);
    assert_eq!(lines[0], "# タスク管理");
    assert!(lines.contains(&"[タスク: ]".to_string()));

    set_input(&mut state, "牛乳を買う".to_string());
    assert!(click_button(&screen, "追加", &mut state));
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"[ ] 牛乳を買う".to_string()));
    // 追加後の入力欄クリアも検証
    assert!(lines.contains(&"[タスク: ]".to_string()));

    assert!(click_button_at(&screen, &[2, 1], "完了", &mut state));
    assert!(render_headless(&screen, &state).contains(&"[x] 牛乳を買う".to_string()));
}
```

### パターン 8: 画面遷移スタック（積む/戻る、Issue #726）

複数 `Screen<S>` を重ねて戻る画面遷移は `ScreenStack<S>` を状態型 `S` のフィールドとして
持たせて表現する。状態は単一共有のまま、画面ごとに `fn() -> Screen<S>` を `push`/`pop` する
（REQ-13: 状態書き込みは `fn(&mut S)` 限定のため、遷移そのものもこの制約内で表現する）。

- `ScreenStack::new(root: fn() -> Screen<S>) -> Self`: root を 1 枚積んだ状態で作る
- `.push(screen: fn() -> Screen<S>)`: 画面を積む
- `.pop() -> bool`: 直前の画面へ戻る。root では取り除かず `false`（panic しない）
- `.current() -> Screen<S>`: 末尾のビルダーを呼んで現在画面を構築する（呼び出し元は
  これだけを描く契約。スタック全体を意識しない）
- `#[derive(Default)]` では `ScreenStack::new(root)` を組み立てられないため、`nav` フィールドを
  持つ状態型は **`Default` を手動実装**すること（`ScreenStack::new(root_screen)` を渡す）
- 「戻る」ボタンと OS の戻る操作（Esc・Android バックキー）には**同じハンドラ**を渡す:
  ボタンでは `.button(id, "戻る", go_back)`、OS の戻るでは `.on_back(go_back)`
  （`ui::request_back` / `state::Event::BackRequested` 経由でプラットフォームレイヤが発火する）

```rust,ignore
// src/screens/list_detail.rs（doctest 不可。`crate::` を使うため）
use crate::{Screen, ScreenStack, WidgetId};

pub struct ListDetailState {
    pub nav: ScreenStack<Self>,
    pub items: Vec<String>,
    pub selected: usize,
}

impl Default for ListDetailState {
    fn default() -> Self {
        Self {
            nav: ScreenStack::new(list_screen),
            items: vec!["項目 A".to_string(), "項目 B".to_string()],
            selected: 0,
        }
    }
}

fn selected_label(state: &ListDetailState) -> String {
    state.items.get(state.selected).cloned().unwrap_or_default()
}

fn open_detail(state: &mut ListDetailState) {
    state.nav.push(detail_screen);
}

fn go_back(state: &mut ListDetailState) {
    state.nav.pop();
}

pub fn list_screen() -> Screen<ListDetailState> {
    Screen::new("一覧")
        .dynamic_text(selected_label)
        .button(WidgetId::from_raw(1), "詳細を開く", open_detail)
}

pub fn detail_screen() -> Screen<ListDetailState> {
    Screen::new("詳細")
        .dynamic_text(selected_label)
        .button(WidgetId::from_raw(1), "戻る", go_back)
        .on_back(go_back)
}

// レジストリ・プラットフォームレイヤはこの関数を毎フレーム呼び直す（`fn(&S) -> Screen<S>`）。
pub fn current_screen(state: &ListDetailState) -> Screen<ListDetailState> {
    state.nav.current()
}
```

参照実装は `crates/fandhe-core/src/screens/list_detail.rs`（結合テストは
`crates/fandhe-core/tests/list_detail.rs`）。Issue #730 で商品 20 件の一覧（`VirtualList` +
行タップ）→詳細（お気に入りトグル・状態共有）へ拡張済みで、上記コード例（2 画面・3
項目の最小形）より広い構成になっている。レジストリへの登録方法（`nav` 行の書式）は
「新規画面ファイルの置き場」節を参照。

### パターン 9: 外部 I/O 完了の反映（`StateStore::apply`、Issue #745）

別スレッドで完了する外部 I/O（保存・HTTP・カメラ等、Phase 8。`fandhe-device` は未実装）の
結果を状態 `S` へ反映するには、`dispatch`（`Event` 経由・関数ポインタ限定）ではなく
`StateStore::apply` を使う。完了結果はその場でしか値が定まらずクロージャによる
キャプチャが本質的に必要なため、`apply` だけが「状態アクセスは関数ポインタ限定」
（型制約節）の対象外になっている。`apply` は無条件に `f` を実行し `version` を進める
（`dispatch` のような「一致しなければ何もしない」判定は持たない）ため、呼び出し元は
「反映すべき完了である」ことを保証する契約になる。

```rust
use fandhe_core::StateStore;

#[derive(Default)]
struct DownloadState {
    body: String,
}

let mut store = StateStore::new(DownloadState::default());

// 実際には HTTP クライアント等の完了コールバックが呼ぶ想定（ここでは直接呼ぶ）。
store.apply(|state: &mut DownloadState| {
    state.body = "pong".to_string();
});

assert_eq!(store.state().body, "pong");
assert_eq!(store.version(), 1);
```

`fandhe-egui-proxy::ProxyApp` はフレーム先頭で完了キューを drain して `apply` する
受け口を持つ。別スレッドから完了を送るには `ProxyApp::completion_sender` を使う:

```rust,ignore
// main.rs 等（doctest 不可。`fandhe_egui_proxy::` のクレート外 API を使うため）
use fandhe_egui_proxy::{Completion, ProxyApp};

let sender = app.completion_sender(); // CompletionSender<S>（再描画要求を組み込んだラッパー）

std::thread::spawn(move || {
    // ここで実際の I/O（HTTP リクエスト等、未実装）を行い、完了結果を state へ書き込む
    // クロージャを組み立てて送る。
    let body = "pong".to_string();
    let completion: Completion<S> = Box::new(move |state: &mut S| {
        state.body = body;
    });
    let _ = sender.send(completion);
});
```

参照実装は `crates/fandhe-core/src/state/store.rs` の `StateStore::apply`、
`crates/fandhe-egui-proxy/src/app.rs` の `drain_completions` / `ProxyApp::completion_sender`。

画面のボタン → 端末 I/O 起動を配線するには、`registry.rs` の `effect` 区画
（`generated_screens!` マクロ、Issue #748）を使う。`nav "<name>" => current_screen,` と同じ
状態駆動画面に、`driver: fn(&S, &CompletionSender<S>)`（`EffectFn<S>`）を追加で紐付け、
`ProxyApp::update` のフレーム末尾から毎フレーム `driver(store.state(), &completion_sender())`
として呼ぶ。`driver` は `&S` の借用のみを受け取り状態を直接変更できないため（変更は必ず
`CompletionSender::send` → 次フレームの `drain_completions` 経由）、「起動が必要か」を
状態側のフラグで表現する。`driver` は関数ポインタで状態を持てないため、以下の二段階の
フェーズ遷移が定石になる:

1. 画面のボタンハンドラが `Idle` 等 → `Requested` へ遷移させる
2. `driver` が `Requested` を観測した瞬間、`sender.send` で「起動済みマーカー」への
   遷移（例: `Loading`）を同期的に送り、別スレッドで実際の I/O を開始する
3. I/O 完了時に別スレッドから最終結果（`Loaded`/`Failed` 等）を `sender.send` する
4. `Loading` 以外で届いた最終結果は古い完了とみなして無視する（呼び出し元の判定）

この二段階により、`driver` 自身が「起動済みか」を覚えていなくても、同一フレーム内で
`Requested` が複数回観測されて二重起動することはない（フレーム内順序「drain →
描画/dispatch → driver 呼び出し」が契約）。参照実装は
`crates/fandhe-core/src/screens/http_fetch.rs`（状態機械）と
`crates/fandhe-egui-proxy/src/device/http_fetch.rs`（`driver` 本体）、
`crates/fandhe-egui-proxy/src/screens/registry.rs`（`effect` 行の書式）。

取得先はビルド時 `FANDHE_ITEMS_URL` で差し替え可能（実機テスト用、Issue #660）。
`fandhe-device` の `Http` 実装が許可する平文 http はループバックと私設 IPv4
リテラルのみで、それ以外の宛先は https 必須（`crates/fandhe-device/src/http.rs`）。

端末機能（`fandhe-device` の `Storage`/`Http`/`Camera`）を使う `effect` 行は
`#[cfg(all(feature = "device", not(target_arch = "wasm32")))]` を行頭に付ける（Issue #746）。
`device` feature は既定 on で wasm32 でも有効になるが、`fandhe-device` 自体は
`fandhe-egui-proxy/Cargo.toml` の非 wasm32 分岐にのみ optional dependency として
置かれているため、`feature = "device"` 単体では wasm32 ビルドで未解決の crate 参照に
なる（`not(target_arch = "wasm32")` を併記して防ぐ）。

### パターン 10: 端末保存の型サンプル「メモ保存」（Issue #747）

画面 ↔ 端末実装の接続方式（上記「新規画面ファイルの置き場」節）の参照実装。
`crates/fandhe-core/src/screens/storage_note.rs`（結合テストは
`crates/fandhe-core/tests/storage_note.rs`）が画面本体と最小トレイト `NoteStorage` を持ち、
`crates/fandhe-egui-proxy/src/device.rs` が `fandhe_device::storage::FsStorage` への橋渡しと
`registry.rs` の `init` 行が呼ぶ状態ファクトリを持つ。`FANDHE_PROXY_APP=storage_note` で
起動でき、保存したメモは `$HOME/.fandhe` 配下に残るため再起動後も一覧に残る。

「ボタン → 端末 I/O 起動 → 完了反映」型の画面は、上記の素の `Completion<S>` 送信に加えて
`crates/fandhe-egui-proxy/src/screens.rs` の `generated_screens!` マクロが持つ `effect` 区画
（`app::EffectFn<S> = fn(&S, &CompletionSender<S>)`、`ProxyApp::with_effect` 経由で注入）を
使う。「起動要求が来たか」を状態側のフラグ（例:
`fandhe_core::screens::camera_capture::CapturePhase::Requested`）で表現し、`effect`
ドライバがフレーム末尾で毎回それを見て起動する二段階遷移（`Requested` → ドライバ検知
→ `Capturing`）にするのが契約（同一フレーム内の二重起動観測を避けるため）。
参照実装は `crates/fandhe-core/src/screens/camera_capture.rs`（状態機械・
`CameraCaptureState`）と `crates/fandhe-egui-proxy/src/device/camera_capture.rs`
（`camera_capture_effect`、`fandhe-device` の `Camera` トレイトを呼ぶ側）。

**Android の配線先（Issue #750）**: `DataDir`（「メモ保存」の保存先）は
`crates/fandhe-egui-proxy/src/device/android.rs` の `data_dir()` が
`AndroidApp::internal_data_path()`（`getFilesDir()` 相当）を返す。カメラは
`AndroidCamera`（同ファイル）が JNI 経由で `MainActivity.requestCameraCapture()` を呼び、
Java 側の `ActivityResultContracts.TakePicturePreview` が撮影した `Bitmap` を
`nativeOnCameraResult` へ渡す往復（`platforms/android/app/.../MainActivity.java`）で
`Camera::capture` を実現する。新規画面が Android 固有の端末実装を要る場合はこの
`device/android.rs` 配下に閉じ、`fandhe-core` へは持ち込まない。

**画像表示（Issue #5）**: 撮影結果等の実画素は `Screen::rgba_image(RgbaPixels, alt)`
（`Node::RgbaImage`）で出す。`Screen::image` / `image_bound`（`ui/image.rs`）は識別子
（パス・URL の不透明文字列）を保持するだけで画素を運べないため、画素を見せたい画面には使わない。
状態に生の RGBA を持たせず、`camera_capture.rs` の `thumbnail_from_rgba` のように長辺 160px 程度へ
縮小してから `RgbaPixels` にする。`fandhe-egui-proxy` は GPU テクスチャ、`fandhe-web-dom` は
`<img src="data:image/png;base64,…">` で描く。`fandhe-swiftui-bridge` は未対応で、alt を
`SnapshotNode::Text` として渡す。

iOS 固有の端末実装（Issue #751）は `crates/fandhe-egui-proxy/src/device/ios.rs`（Rust、
`Camera`/`Storage` トレイトの橋渡し）+ `crates/fandhe-egui-proxy/ios/*.m`（ObjC、UIKit /
AVFoundation の実処理。`build.rs` が iOS ターゲットのみコンパイルする）に置く。

### 参照実装への誘導

フルサンプル（5 要素を含む代表 1 画面）は、重複掲載による陳腐化を避けるため以下を直接参照する。

- `crates/fandhe-core/src/ui/sample.rs`（`SampleState` / `sample_screen` / `get_memo` /
  `set_memo` / `increment_count` / `add_item` / `count_label` / `expand_items`）
- `crates/fandhe-core/examples/sample_screen.rs`（ヘッドレス動作確認）
- `crates/fandhe-core/tests/sample_screen.rs`（結合テスト。構造変更時の期待値の一本化先）

### 新規画面ファイルの置き場と `mod` 追加の作法（Issue #653）

AI が新規画面を追加する場合、既存ファイルへの追記ではなく
以下の 3 か所へ**新規ファイル**として置く。

```text
crates/fandhe-core/src/screens/<name>.rs       # 画面本体（状態型・ビルダー・ハンドラ）
crates/fandhe-core/examples/<name>.rs          # ヘッドレス動作確認（任意）
crates/fandhe-core/tests/<name>.rs             # 結合テスト（パターン 7 の方針で書く）
```

`crates/fandhe-core/src/ui/mod.rs` は 600 行超で大半がドキュメンテーションコメントのため、
**手を入れない**。代わりに新設した `crates/fandhe-core/src/screens/mod.rs`
（登録した画面を切り替えて起動するための置き場、冒頭にモジュールの役割を説明する
ドキュメンテーションコメントを持つ）へ `pub mod <name>;` を 1 行足す。既存内容を一切
省略・変更せず、末尾に追加した `pub mod` 行を加えたファイル全文を出力する。

端末機能（保存・HTTP・カメラ、Phase 8 / Issue #746）を使う画面は `fandhe-device` の
トレイト（`Storage`/`Http`/`Camera`）を直接 import しない。依存方向は
`fandhe-egui-proxy` → `fandhe-device` の一方向のみで、`fandhe-core` からは参照できない
（coding-rust.md の二層アーキテクチャ原則）。画面 ↔ 端末実装の接続は次の 2 形にする:

- 構築時に副作用を伴う状態初期化（端末保存領域からの読み込み等、メモ保存等）は
  次の形にする（Issue #747 で確定。参照実装は下記「パターン 10」）:
  1. `fandhe-core` 側に `fandhe_device` の対象トレイトと同形の最小トレイト
     （エラーは表示用文字列に単純化）を画面ファイル内に定義する。純粋なトレイト定義
     のみで I/O を持たないため「端末依存」には当たらず、画面ファイルと一緒に削除できる
  2. `fandhe-egui-proxy` 側の `device.rs` が `fandhe-device` の実装を newtype で
     そのトレイトへ橋渡しし、状態ファクトリ（`fn() -> S`）を提供する
  3. `crates/fandhe-egui-proxy/src/screens/registry.rs` の `init "<name>" => 画面ビルダー,
     状態ファクトリ,` 行（`generated_screens!` マクロの `init` 区画、下記参照）が
     `Default::default()` の代わりにその状態ファクトリで状態を構築する。構築時に
     端末保存領域から読み込む等の副作用を伴う初期化を、`fandhe-core` に
     `fandhe-device` を参照させずに配線できる
- フレーム末尾での非同期な端末 I/O 起動（HTTP 取得等）は `registry.rs` の `effect` 区画
  （パターン 9、Issue #748）で行う

新規画面を追加する場合は、`crates/fandhe-egui-proxy/src/screens/registry.rs`（画面レジストリ、
Issue #715）へも 1 行足す。`generated_screens!` マクロは `"<name>" => <path>,` の並びを
先に、`nav "<name>" => <path>,` の並びをその後にしか受け付けない（順序が逆になるとマクロが
パターンマッチできずビルドが壊れる）。そのため `"<name>" => fandhe_core::screens::<name>::
<画面ビルダー関数>,` は、ブロック内に既に `nav` 行がある場合は**その最初の行の直前**に、
`nav` 行が 1 つも無い場合はブロック末尾（閉じ括弧の前）に追加し、既存行は一切省略・変更せず
そのまま残した全文を出力する。この 1 行がある画面は `FANDHE_PROXY_APP=<name>` で起動した
egui プロキシ端末に表示される。変更はコミットしてから次の作業に進む。

端末機能を使う画面の行には `#[cfg(all(feature = "device", not(target_arch = "wasm32")))]` を
行頭に付ける（Issue #746/#747。`fandhe-device` への依存自体が `Cargo.toml` の native 分岐に
しか無いため、feature `device` 単独では wasm32 ビルドが壊れる）。`generated_screens!` マクロは
各行に属性を 0 個以上付けられる（非 `nav` 行は属性が `"名前"` の直前、`nav` 行は `nav`
キーワードの直後、`init` 行（下記参照）は `init` キーワードの直後、`effect` 行（パターン 9
参照）は `effect` キーワードの直後に属性が来る点が異なる。マクロの repetition 境界解決の
制約、`screens.rs` のマクロ定義 doc 参照）。

状態を `Default::default()` ではなく状態ファクトリ（`fn() -> S`）から構築したい画面
（上記の端末保存領域からの読み込みなど）は、`"<name>" => 関数,` / `nav "<name>" => 関数,`
の代わりに `init "<name>" => 画面ビルダー, 状態ファクトリ,`（`generated_screens!` マクロの
`init` 区画）を使う。区画順は 非 `nav` → `nav` → `init` → `effect` の固定のみを受け付ける
（他の区画と同じ理由でこの順以外はパターンマッチに失敗する）。

`ScreenStack<S>`（パターン 8）を使う画面のように、状態の変化に応じて表示画面を毎フレーム
引き直す必要がある場合は `"<name>" => 関数,` の代わりに
`nav "<name>" => fandhe_core::screens::<name>::current_screen,`（`generated_screens!` マクロの
`nav` 区画、`current_screen: fn(&S) -> Screen<S>`）を使う。固定 1 画面（`fn() -> Screen<S>`）の
既存 `"<name>" => 関数,` 形式と併記でき、区画を跨いで名前が重複しなければどちらも登録できる。

画面本体は `ui::{Screen, Node}` と `state` の公開 API のみを使い、状態型は画面ファイル内で
`#[derive(Default)]` を付けて定義する（パターン 1〜7 と同じ構成）。画面を組み立てる関数・
状態型・ハンドラはすべて `pub` にする（`examples/<name>.rs`・`tests/<name>.rs` は別クレートと
してコンパイルされ公開 API しか参照できないため）。以下は `screens/todo.rs` に置く最小画面の例。
この配置先はクレート内部（`fandhe-core` 自身のモジュールツリー内）であり、`fandhe-core` は
`extern crate self as fandhe_core;` を宣言していないため、他パターンと異なり `fandhe_core::`
ではなく `crate::` で公開 API を参照する。本ブロックは `crate::` を使うため doctest としては
コンパイルできず `rust,ignore` とする。参照実装は
`crates/fandhe-core/src/screens/schema_example_fixture.rs`（`examples/`・`tests/` 双方から
公開 API 経由で呼び出して固定化）。ファイル名・モジュール名を例示の画面名 `todo` と変えているのは、
AI がこの例をそのまま画面名 `todo` として生成した場合に参照実装モジュールと二重定義するのを
避けるため。

```rust,ignore
use crate::{Screen, WidgetId};

#[derive(Default)]
pub struct TodoScreenState {
    pub items: Vec<String>,
}

pub fn add_todo(state: &mut TodoScreenState) {
    state.items.push(format!("todo-{}", state.items.len() + 1));
}

pub fn todo_screen() -> Screen<TodoScreenState> {
    Screen::new("タスク一覧")
        .text("タスク一覧")
        .button(WidgetId::from_raw(1), "追加", add_todo)
}
```

## 既知の誤りやすい点・アンチパターン

AI エージェントが `fandhe-core` でコードを生成する際に陥りやすい誤りを、
「誤り（NG）→ 症状 → 正しい書き方（OK）→ 根拠」の形式で列挙する。1〜5・9・11〜13 は
`compile_fail` / 通常ブロックの doctest で NG=コンパイル失敗・OK=コンパイル成功を個別検証する。
6〜8 はコンパイルは通るが挙動が意図と異なる誤りで、実行時アサーションで確認する（8 は NG 例なし。
10 は誤解の否定であり NG 例は存在しない）。

### 1. 環境キャプチャするクロージャを `get` / `set` / `on_click` / `expand` に渡す

- **症状**: `expected fn pointer, found closure` でコンパイルエラーになる
- **NG**:

  ```rust,compile_fail
  use fandhe_core::{Screen, WidgetId};

  #[derive(Default)]
  struct AppState {
      count: u32,
  }

  fn main() {
      let bonus = 10;
      let _screen: Screen<AppState> = Screen::new("t").button(WidgetId::from_raw(1), "+n", move |state: &mut AppState| {
          state.count += bonus; // `bonus` をキャプチャしており fn へ coerce できない
      });
  }
  ```

- **OK**: 名前付き関数、または環境を参照しない（非キャプチャの）クロージャ式を渡す

  ```rust
  use fandhe_core::{Screen, WidgetId};

  #[derive(Default)]
  struct AppState {
      count: u32,
  }

  fn incr(state: &mut AppState) {
      state.count += 1;
  }

  fn main() {
      let _screen: Screen<AppState> = Screen::new("t").button(WidgetId::from_raw(1), "+1", incr);
  }
  ```

- **根拠**: 型制約「状態アクセスは関数ポインタ限定」（REQ-13 / NFR-9）。意図した安全網であり
  回避しない

### 2. `Node` バリアントのフィールド名の類推誤り

- **症状**: `variant Node<S>::TextField has no field named "value"` 等、`E0559` でコンパイル
  エラーになる
- **NG**: `TextField { label, value: get_memo, callback: set_memo }`（慣用的な命名から
  類推した誤ったフィールド名）
- **OK**: `TextField { label, get: get_memo, set: set_memo }`
- **根拠**: 「公開 API シグネチャ一覧」の `Node<S>` 定義。正しいフィールド名は
  `label` / `get` / `set`（`TextField`）、`label` / `on_click` / `id`（`Button`）、
  `expand` / `axis`（`List`）に固定

### 3. `List::expand` に `Vec<String>` を返す関数を渡す

- **症状**: `expected fn pointer ... Vec<Node<S>>, found fn item ... Vec<String>` で
  コンパイルエラーになる
- **NG**:

  ```rust,compile_fail
  use fandhe_core::{Node, Screen};

  #[derive(Default)]
  struct AppState {
      items: Vec<String>,
  }

  fn expand_items(state: &AppState) -> Vec<String> {
      state.items.clone()
  }

  fn main() {
      let _screen: Screen<AppState> = Screen::new("t").list(expand_items);
  }
  ```

- **OK**: 文字列項目を `Node::Text` へ変換して返す（`ui::sample::expand_items` が実例）

  ```rust
  use fandhe_core::{Node, Screen};

  #[derive(Default)]
  struct AppState {
      items: Vec<String>,
  }

  fn expand_items(state: &AppState) -> Vec<Node<AppState>> {
      state.items.iter().cloned().map(Node::Text).collect()
  }

  fn main() {
      let _screen: Screen<AppState> = Screen::new("t").list(expand_items);
  }
  ```

- **根拠**: `Node::List::expand` は `fn(&S) -> Vec<Node<S>>` であり `Vec<String>` ではない

### 4. 異なる状態型 `S` のノード・画面の混在

- **症状**: `expected fn pointer fn(&A) -> ..., found fn item fn(&B) -> ...` で
  コンパイルエラーになる
- **NG**: 状態型 `A` 用に定義したはずの `Vec<Node<A>>` へ、状態型 `B` 用の `get` / `set` /
  `on_click` を持つノードを混在させる
- **OK**: 1 つの `Screen<S>` / ノード列は単一の状態型 `S` に固定する
- **根拠**: 型制約「`Screen<S>` は 1 画面 1 状態型」。異なる状態型のノードを混在させると
  型不一致でコンパイルエラーになる（意図した型安全性）

### 5. self 消費ビルダーの戻り値を捨てる

- **症状**: コンパイル自体は多くの場合通るが、追加した要素が反映されない。加えて元の
  変数をその後も使おうとすると `value borrowed here after move`（`E0382`）でコンパイル
  エラーになる（`Screen` は `Copy` を実装しないため）
- **NG**:

  ```rust,compile_fail
  use fandhe_core::{Screen, WidgetId};

  #[derive(Default)]
  struct AppState {
      count: u32,
  }

  fn incr(state: &mut AppState) {
      state.count += 1;
  }

  fn main() {
      let screen: Screen<AppState> = Screen::new("t");
      screen.button(WidgetId::from_raw(1), "+1", incr); // 戻り値を捨てている（screen はムーブ済み）
      println!("{}", screen.len()); // ここでコンパイルエラー
  }
  ```

- **OK**: 戻り値を受け取って再代入する（メソッドチェーン）

  ```rust
  use fandhe_core::{Screen, WidgetId};

  #[derive(Default)]
  struct AppState {
      count: u32,
  }

  fn incr(state: &mut AppState) {
      state.count += 1;
  }

  fn main() {
      let screen: Screen<AppState> = Screen::new("t").button(WidgetId::from_raw(1), "+1", incr);
      assert_eq!(screen.len(), 1);
  }
  ```

- **根拠**: 型制約「ビルダーは self 消費のメソッドチェーン」。可変参照が必要な場合のみ
  `push` を使う

### 6. `click_button` が `List` 内のボタンにも届くという誤解

- **症状**: コンパイルは通るが、`List` の `expand` が返す子ノード中の `Button` を押下した
  つもりでも状態が変化しない（`click_button` が `false` を返す）
- **誤り**: `List` の子要素に同名ラベルの `Button` を配置し、`click_button(&screen, "+1", ..)`
  で発火すると期待する
- **正しい理解**: `click_button` は `List` を展開しない線形走査のためラベル一致検索でも
  `List` の中身までは辿らない。動的生成子内のボタンは `click_button_at`（パターン 7 参照）を使う
- **根拠**: 「公開 API シグネチャ一覧」の `ui::event::click_button` の説明

### 7. `visit` と `visit_expanded` の混同

- **症状**: `List` 配下の子ノードを含めて数えたい場合に `visit` を使うと件数が不足する
  （`List` 自身の 1 件のみ）
- **誤り**: 全ノード走査の目的で `Screen::visit` を使う
- **正しい理解**: `visit` は `List` を展開しない単純走査。全ノード走査には
  `visit_expanded(state, f)` を使う
- **根拠**: 「公開 API シグネチャ一覧」の `Screen::visit` / `Screen::visit_expanded` の説明

### 8. panic 前提のコードを書く・`unwrap()` / `expect()` の使用

- **症状**: 想定外の入力（ラベル不一致・非 `List` ノードへの展開要求）で panic し、
  アプリ全体が落ちる
- **誤り**: 「ボタンは必ず見つかる」といった前提で `unwrap()` 的にエラーケースを無視したコードを書く
- **正しい理解**: `fandhe-core` は panic を API 契約にしない設計で、`expand_children` は
  非対応バリアントに空の `Vec` を、`click_button` 系は不一致・範囲外時に `false` を返す。
  呼び出し側はこの戻り値を確認して分岐し、生成コードで `unwrap()` / `expect()` を使わない
- **根拠**: 型制約「panic を API 契約にしない」・`coding-rust.md`

### 9. コールバックのシグネチャ誤り

- **症状**: `expected fn pointer fn(&mut S) -> (), found fn item fn(&mut S, i32) -> bool`
  （引数・戻り値の数や型が異なる）でコンパイルエラーになる
- **NG**:

  ```rust,compile_fail
  use fandhe_core::{Screen, WidgetId};

  #[derive(Default)]
  struct AppState {
      count: i32,
  }

  fn incr_with_amount(state: &mut AppState, amount: i32) -> bool {
      state.count += amount;
      true
  }

  fn main() {
      let _screen: Screen<AppState> = Screen::new("t").button(WidgetId::from_raw(1), "+n", incr_with_amount);
  }
  ```

- **OK**: `on_click` は `fn(&mut S)` 固定（引数・戻り値の追加は不可）。1 の OK と同一コード
- **根拠**: 「公開 API シグネチャ一覧」の `Node::Button::on_click` / `Screen::button` の
  シグネチャ。なお `Node::TextField::get` は `String` を値で返す（`&str` / `&String` では
  ない）

### 10. `S` に `Debug` / `Clone` の derive が必須という誤解

- **誤解**: `Screen<S>` / `Node<S>` を使うために状態型 `S` へ `#[derive(Debug, Clone)]` を
  付けなければならないと考える
- **正しい理解**: `Node<S>` / `Screen<S>` の `Debug` / `Clone` はいずれも手動実装であり、
  `S` に対して `Debug` / `Clone` 境界を課さない。`S` に derive を付けなくても
  `Screen<S>` / `Node<S>` は問題なく使える
- **根拠**: `ui::Node<S>` / `ui::Screen<S>` の型定義コメント（「`Debug` / `Clone` は
  手動実装」）

### 11. `ReadAccessor` / `WriteAccessor` を `Node` フィールドへ直接渡す

- **症状**: `expected fn pointer fn(&S) -> String, found struct ReadAccessor<S, String>` で
  コンパイルエラーになる
- **NG**:

  ```rust,compile_fail
  use fandhe_core::{Node, ReadAccessor, WriteAccessor};

  struct FormState {
      memo: String,
  }

  fn get_memo(state: &FormState) -> String {
      state.memo.clone()
  }
  fn set_memo(state: &mut FormState, value: String) {
      state.memo = value;
  }

  fn main() {
      let accessor: ReadAccessor<FormState, String> = ReadAccessor::new(get_memo);
      let write_accessor: WriteAccessor<FormState, String> = WriteAccessor::new(set_memo);
      let _node: Node<FormState> = Node::TextField {
          label: "メモ".to_string(),
          get: accessor,
          set: write_accessor,
      };
  }
  ```

- **OK**: `Node` は生の関数ポインタを受けるため `as_fn()` で橋渡しする

  ```rust
  use fandhe_core::{Node, ReadAccessor, WriteAccessor};

  struct FormState {
      memo: String,
  }

  fn get_memo(state: &FormState) -> String {
      state.memo.clone()
  }
  fn set_memo(state: &mut FormState, value: String) {
      state.memo = value;
  }

  fn main() {
      let accessor: ReadAccessor<FormState, String> = ReadAccessor::new(get_memo);
      let write_accessor: WriteAccessor<FormState, String> = WriteAccessor::new(set_memo);
      let _node: Node<FormState> = Node::TextField {
          label: "メモ".to_string(),
          get: accessor.as_fn(),
          set: write_accessor.as_fn(),
      };
  }
  ```

- **根拠**: 「公開 API シグネチャ一覧」の `state::ReadAccessor` / `Node::TextField` の説明

### 12. `button` / ウィジェット系ビルダーに `id` を渡し忘れる

- **症状**: `this function takes 3 arguments but 2 arguments were supplied`（`E0061`）で
  コンパイルエラーになる
- **NG**:

  ```rust,compile_fail
  use fandhe_core::Screen;

  #[derive(Default)]
  struct AppState {
      count: u32,
  }

  fn incr(state: &mut AppState) {
      state.count += 1;
  }

  fn main() {
      let _screen: Screen<AppState> = Screen::new("t").button("+1", incr);
  }
  ```

- **OK**: `id: WidgetId` を第 1 引数に渡す。1 の OK と同一コード
- **根拠**: 「公開 API シグネチャ一覧」の `Screen::button` / ウィジェット系ビルダーのシグネチャ

### 13. `WidgetId::new(...)` を呼ぶ・借用を持つ状態型をウィジェット系ビルダーへ渡す

- **症状**: `WidgetId::new` は存在しないため `no function or associated item named 'new'
  found`（`E0599`）でコンパイルエラーになる。借用を持つ状態型（ライフタイムパラメータを
  含む `S`）をウィジェット系ビルダーへ渡すと `` `S` may not live long enough ``（`S: 'static`
  境界違反）でコンパイルエラーになる
- **NG**: `WidgetId::new(1)` / ライフタイム `'a` を持つ `struct AppState<'a> { .. }` を
  `Screen<AppState<'a>>::checkbox(..)` へ渡す
- **OK**: `WidgetId::from_raw(1)` を使い、ウィジェット系ビルダーへ渡す状態型は所有型
  （ライフタイムパラメータなし）にする
- **根拠**: 「公開 API シグネチャ一覧」の `WidgetId` / `Screen` ウィジェット系ビルダーの
  シグネチャ（`where S: 'static`）
