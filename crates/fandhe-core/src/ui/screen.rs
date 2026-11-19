//! 1 画面の宣言的 UI 記述（`Screen<S>`）とツリー構築・走査 API（TASK-1.1-S1 / S2 / S3 / S4）。
//!
//! `Screen<S>` は状態型 `S` を固定した [`super::node::Node`] のフラットな並びを
//! 保持する最小のツリー構造である。汎用の `push` / `with_node` によるノード追加、
//! `visit` による単純走査に加え、`.text()` / `.text_field()`（TASK-1.1-S2 / #18）・
//! `.button()` / `.list()` ビルダーと `List` の子要素展開を伴う
//! `visit_expanded`（TASK-1.1-S4 / REQ-1）を提供する。
//! 描画マッピングは [`super::render::render_headless`] が本モジュールの `visit_expanded`
//! を走査基盤として構築されている。イベント配線（S5: #21）も同様にこの走査基盤の
//! 上に構築することを想定している。

use super::card::{Card, Separator};
use super::checkbox::Checkbox;
use super::dialog_widget::DialogWidget;
use super::icon::Icon;
use super::image_widget::ImageWidget;
use super::menu::Menu;
use super::node::Node;
use super::progress::{ProgressBar, Spinner};
use super::radio_button::RadioButton;
use super::render_mode::RenderMode;
use super::slider::Slider;
use super::stack_grid::{Grid, Stack};
use super::tabs::Tabs;
use super::text_input::TextInput;
use super::toggle_switch::ToggleSwitch;
use super::tooltip::Tooltip;
use super::widget::{Widget, WidgetId};

/// 1 画面の宣言的 UI 記述。状態型 `S` はアプリ（呼び出し元）ごとに固定する。
///
/// ジェネリック `S` により、異なる状態型のノードを同一 `Screen` に混在させると
/// コンパイルエラーになる（PoC-6 で実証済みの型安全性を維持）。
///
/// `Debug` / `Clone` は手動実装する（`Node<S>` と同様、状態型 `S` は所有していないため
/// `derive` マクロが自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける）。
pub struct Screen<S> {
    title: String,
    nodes: Vec<Node<S>>,
    /// 画面単位の出力方式（REQ-4 / TASK-4.4-S3）。既定は [`RenderMode::Canvas`]
    /// （既存挙動を変えない安全側の既定）。Web 以外のプラットフォームレイヤは無視してよい。
    render_mode: RenderMode,
    /// OS の戻る操作（Esc・Android バックキー等）を受けたときのハンドラ（Issue #726）。
    /// `None`（既定）の画面は「戻る」操作に応答しない。詳細は [`Self::on_back`] を参照。
    on_back: Option<fn(&mut S)>,
}

impl<S> std::fmt::Debug for Screen<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen")
            .field("title", &self.title)
            .field("nodes", &self.nodes)
            .field("render_mode", &self.render_mode)
            .field("on_back_is_some", &self.on_back.is_some())
            .finish()
    }
}

impl<S> Clone for Screen<S> {
    fn clone(&self) -> Self {
        Self {
            title: self.title.clone(),
            nodes: self.nodes.clone(),
            render_mode: self.render_mode,
            on_back: self.on_back,
        }
    }
}

impl<S> Screen<S> {
    /// タイトルのみを持つ空の `Screen` を作る。出力方式は既定の [`RenderMode::Canvas`]。
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            nodes: Vec::new(),
            render_mode: RenderMode::default(),
            on_back: None,
        }
    }

    /// 画面タイトルを返す。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 画面単位の出力方式を指定する（ビルダー形式、REQ-4 / TASK-4.4-S3）。
    ///
    /// アプリ・プロトタイプ層が画面登録時に呼ぶ想定（設計書
    /// `docs/architecture/dom-output-architecture.md` §2.5）。Web ターゲットの
    /// 切替調停（[`super::render_mode::ModeSwitcher`]）がこの値を評価して
    /// DOM 出力／独自レンダラ（canvas）を切り替える。
    pub fn with_render_mode(mut self, mode: RenderMode) -> Self {
        self.render_mode = mode;
        self
    }

    /// 画面単位の出力方式を返す（既定 [`RenderMode::Canvas`]）。
    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    /// OS の戻る操作（Esc・Android バックキー等）を受けたときのハンドラを指定する
    /// （ビルダー形式、Issue #726）。
    ///
    /// プラットフォームレイヤが OS の戻る操作を変換し
    /// [`super::event::request_back`] / [`crate::state::Event::BackRequested`] 経由で
    /// 発火する契約（`state/store.rs` のモジュール doc 参照）。典型的には
    /// [`super::screen_stack::ScreenStack::pop`] を呼ぶハンドラを渡し、画面内の
    /// 「戻る」ボタンと同じハンドラを共有する（`docs/ai/SCHEMA.md` パターン 8 参照）。
    pub fn on_back(mut self, handler: fn(&mut S)) -> Self {
        self.on_back = Some(handler);
        self
    }

    /// [`Self::on_back`] で設定したハンドラを返す（未設定なら `None`）。
    pub fn back_handler(&self) -> Option<fn(&mut S)> {
        self.on_back
    }

    /// 現在保持しているノード数を返す。
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// ノードを 1 つも持たないかどうかを返す。
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// ノードを末尾に追加する（所有権を取り、`self` を返すビルダー形式）。
    ///
    /// 要素別のビルダーメソッド（`.text()` 等）は S2 以降で本メソッドを介して
    /// 実装される想定の、最小の汎用追加 API。
    pub fn with_node(mut self, node: Node<S>) -> Self {
        self.nodes.push(node);
        self
    }

    /// ノードを末尾に追加する（可変参照版）。
    pub fn push(&mut self, node: Node<S>) {
        self.nodes.push(node);
    }

    /// 静的テキストを追加する（ビルダー形式、TASK-1.1-S2 / #18）。
    ///
    /// `Node::Text` を組み立てて [`Self::with_node`] に委譲する糖衣構文。
    /// `.button()` / `.list()` と同様、状態 `S` には依存しない。
    pub fn text(self, s: impl Into<String>) -> Self {
        self.with_node(Node::Text(s.into()))
    }

    /// 状態 `S` から都度文字列を読み取る動的テキストを追加する（ビルダー形式、
    /// Issue #581 / REQ-1）。
    ///
    /// `Node::DynamicText` を組み立てて [`Self::with_node`] に委譲する糖衣構文。
    /// **`.text()` との使い分け**: `.text()` は状態 `S` に依存しない静的文字列
    /// （`Node::Text`）を追加するのに対し、本メソッドは `get` が返す値を描画・走査の
    /// たびに再評価する（`.text_field()` の `get` と同じ都度評価の設計）。表示内容が
    /// カウンタ等の状態変化に追随する必要がある場合に使う。単一の動的テキストを
    /// `.list()`（1 件だけの `Node::Text` 展開）で代用しないこと（node.rs の
    /// Issue #581 コメント参照。`List` は各バックエンドで「動的な子要素一覧」として
    /// 解釈される契約のため、意味論が誤って公開される）。
    pub fn dynamic_text(self, get: crate::state::ReadFn<S, String>) -> Self {
        self.with_node(Node::DynamicText { get })
    }

    /// 役割 [`crate::ui::TextRole::Error`] を持つ動的テキストを追加する
    /// （ビルダー形式、Issue #812、親 #811 の 1/3）。
    ///
    /// `Node::StyledText { role: TextRole::Error, get }` を組み立てて
    /// [`Self::with_node`] に委譲する糖衣構文。DSL には色を書かせない方針
    /// （node.rs の `TextRole` doc 参照）のため、色は本メソッドの引数に取らない。
    /// 実際の色解決はプラットフォームレイヤの `TokenResolver` が担う（#813）。
    /// form 等の「エラー:」接頭辞による代替表現の置き換え先となる（#814）。
    pub fn error_text(self, get: crate::state::ReadFn<S, String>) -> Self {
        self.with_node(Node::StyledText {
            role: crate::ui::TextRole::Error,
            get,
        })
    }

    /// ラベル付きテキスト入力欄を追加する（ビルダー形式、TASK-1.1-S2 / #18）。
    ///
    /// `Node::TextField` を組み立てて [`Self::with_node`] に委譲する糖衣構文。
    /// `get` は状態 `S` から現在値を取得する関数ポインタ、`set` はユーザー入力を
    /// 状態 `S` へ反映する関数ポインタで、いずれも [`crate::state`] の SSOT 型
    /// エイリアス（[`crate::state::ReadFn`] / [`crate::state::WriteFn`]）と代入互換
    /// である（クロージャの環境キャプチャを拒否する設計は [`super::node::Node`] を参照）。
    pub fn text_field(
        self,
        label: impl Into<String>,
        get: crate::state::ReadFn<S, String>,
        set: crate::state::WriteFn<S, String>,
    ) -> Self {
        self.with_node(Node::TextField {
            label: label.into(),
            get,
            set,
        })
    }

    /// ボタンを追加する（ビルダー形式）。
    ///
    /// `Node::Button` を組み立てて [`Self::with_node`] に委譲する糖衣構文。
    /// `on_click` を関数ポインタ `fn(&mut S)` に限定する理由は [`super::node::Node`]
    /// の設計判断を参照（TASK-1.1-S3 由来、S1 マージ後は `Node::Button` バリアント
    /// を用いる本メソッドへ統合）。
    ///
    /// `id` はフォーカス識別・決定操作の安定識別子（TASK-17.2 の前提移行、
    /// `docs/architecture/tv-focus-navigation-architecture.md` §3.1）。他の
    /// ウィジェット系ビルダー（`.checkbox(id, ..)` 等）と対称になるよう、
    /// `id` を第 1 引数に取る破壊的シグネチャ変更を行った（同節）。
    pub fn button(self, id: WidgetId, label: impl Into<String>, on_click: fn(&mut S)) -> Self {
        self.with_node(Node::Button {
            label: label.into(),
            on_click,
            id,
        })
    }

    /// `List` の子要素展開を伴うリスト要素を末尾に追加する（TASK-1.1-S4 / REQ-1）。
    ///
    /// `expand` は状態 `S` から子ノード一覧を生成する関数ポインタで、`Node::List` として
    /// 保持する。実際の展開（子ノード生成）は描画・走査側が [`Screen::visit_expanded`] を
    /// 通じて都度行う。引数の型は [`crate::state::ReadFn`]（`Node::List::expand` と同じ
    /// SSOT 型エイリアス）と代入互換である（TASK-13.1-S4）。
    ///
    /// 配置軸は既定の [`super::node::Axis::Vertical`] を使う非破壊な糖衣構文。軸を
    /// 明示したい場合は [`Self::list_with_axis`] を使う（TASK-17.2 の前提移行、
    /// `docs/architecture/tv-focus-navigation-architecture.md` §4.3）。
    pub fn list(self, expand: crate::state::ReadFn<S, Vec<Node<S>>>) -> Self {
        self.list_with_axis(expand, super::node::Axis::default())
    }

    /// [`Self::list`] の配置軸明示版（TASK-17.2 の前提移行、
    /// `docs/architecture/tv-focus-navigation-architecture.md` §4.3）。
    ///
    /// 加法的な API 拡張であり、既存の `.list()` 呼び出し元は無修正で動く
    /// （coding-rust.md の feature 加法的設計方針に準拠）。
    pub fn list_with_axis(
        self,
        expand: crate::state::ReadFn<S, Vec<Node<S>>>,
        axis: super::node::Axis,
    ) -> Self {
        self.with_node(Node::List { expand, axis })
    }

    /// index ベースで項目を遅延展開する `VirtualList` を末尾に追加する
    /// （TASK-5.5-S2 / REQ-5、Issue #85）。`.list()` と対称の糖衣構文。
    ///
    /// `len` は状態 `S` から総項目数を取得する関数ポインタ、`item` は状態 `S` と
    /// index から 1 項目分の `Node<S>` を生成する関数ポインタ、`item_extent` は
    /// 固定項目高さ（設計文書
    /// `docs/architecture/list-virtualization-architecture.md` §3.3）。
    /// 実際の可視範囲限定走査は [`Self::visit_windowed`] が担う。
    ///
    /// 配置軸は既定の [`super::node::Axis::Vertical`] を使う非破壊な糖衣構文。軸を
    /// 明示したい場合は [`Self::virtual_list_with_axis`] を使う（TASK-17.2 の
    /// 前提移行、`docs/architecture/tv-focus-navigation-architecture.md` §4.3）。
    pub fn virtual_list(
        self,
        len: crate::state::ReadFn<S, usize>,
        item: crate::state::ReadIndexedFn<S, Node<S>>,
        item_extent: f32,
    ) -> Self {
        self.virtual_list_with_axis(len, item, item_extent, super::node::Axis::default())
    }

    /// [`Self::virtual_list`] の配置軸明示版（TASK-17.2 の前提移行、
    /// `docs/architecture/tv-focus-navigation-architecture.md` §4.3）。
    ///
    /// 加法的な API 拡張であり、既存の `.virtual_list()` 呼び出し元は無修正で動く
    /// （coding-rust.md の feature 加法的設計方針に準拠）。
    pub fn virtual_list_with_axis(
        self,
        len: crate::state::ReadFn<S, usize>,
        item: crate::state::ReadIndexedFn<S, Node<S>>,
        item_extent: f32,
        axis: super::node::Axis,
    ) -> Self {
        self.with_node(Node::VirtualList {
            len,
            item,
            item_extent,
            axis,
        })
    }

    /// ラベル付きテキスト入力欄ウィジェットを追加する（ビルダー形式、TASK-8.3-S13 /
    /// Issue #108）。
    ///
    /// `Node::Widget(Box::new(TextInput::bound(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する糖衣構文。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）、
    /// `get` / `set` は [`crate::state`] の SSOT 型エイリアス（[`crate::state::ReadFn`] /
    /// [`crate::state::WriteFn`]）と代入互換の関数ポインタで、環境キャプチャクロージャを
    /// 型レベルで拒否する（REQ-13）。
    ///
    /// **レガシー `.text_field()`（`Node::TextField`）との違い**: `.text_field()` は
    /// TASK-1.1 由来のレガシー enum バリアントであり、設計文書
    /// `docs/architecture/widget-architecture.md` §4.1 の温存方針により変更しない。
    /// 本メソッドは共通トレイト `Widget<S>` を実装する新規ウィジェット型 `TextInput<S>`
    /// を `Node::Widget` として保持するものであり、`.text_field()` の後継ではない
    /// （両者は共存する）。
    ///
    /// **egui・web-dom では描画されない**。画面で入力欄が要るなら `.text_field()` を使う
    /// （扱いは #260）。
    ///
    /// **未配線**: `render_headless` 等は `Node::Widget` を状態非依存のプレースホルダとして
    /// 扱う契約（[`super::widget::Widget::sync_from_state`] のドキュメンテーションコメント
    /// 参照）を維持する。入力イベント処理・`set` の実配線は未接続。`WidgetCatalog`
    /// （`catalog.rs` の `text_field` エントリ）には登録済み。
    pub fn text_input(
        self,
        id: WidgetId,
        label: impl Into<String>,
        get: crate::state::ReadFn<S, String>,
        set: crate::state::WriteFn<S, String>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(TextInput::bound(
            id, label, get, set,
        ))))
    }

    /// チェックボックスウィジェットを追加する（ビルダー形式、TASK-8.3-S19 /
    /// Issue #114）。`.text_input()` と対称の糖衣構文。
    ///
    /// `Node::Widget(Box::new(Checkbox::bound(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）、
    /// `get` / `set` は [`crate::state`] の SSOT 型エイリアス（[`crate::state::ReadFn`] /
    /// [`crate::state::WriteFn`]）と代入互換の関数ポインタで、環境キャプチャクロージャを
    /// 型レベルで拒否する（REQ-13）。
    ///
    /// **実描画・配線**: `fandhe-render-egui` が実描画を担い（Issue #806）、チェック状態変更は
    /// `Event::WidgetValueChanged` → [`super::event::set_widget_value_at`] 経由で `set` へ
    /// 届く（Issue #805）。`render_headless` は `Node::Widget` の行を出力しない契約
    /// （`ui/render.rs` 参照）を維持する。
    pub fn checkbox(
        self,
        id: WidgetId,
        label: impl Into<String>,
        get: crate::state::ReadFn<S, bool>,
        set: crate::state::WriteFn<S, bool>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(Checkbox::bound(id, label, get, set))))
    }

    /// 状態 `S` に束縛されたラジオボタンウィジェット（[`RadioButton`]）を末尾に追加する
    /// （TASK-8.3-S22 / REQ-8 / REQ-13、Issue #117）。`.text_input()` と
    /// 対称の糖衣構文。
    ///
    /// `Node::Widget(Box::new(RadioButton::bound(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）、`value` は
    /// このラジオボタンが表す選択肢値、`get` / `set` は [`crate::state`] の SSOT 型エイリアス
    /// （[`crate::state::ReadFn`] / [`crate::state::WriteFn`]）と代入互換の関数ポインタで、
    /// 環境キャプチャクロージャを型レベルで拒否する（REQ-13）。
    ///
    /// **グループ内排他選択**: 同一グループの複数 `.radio_button()` 呼び出しに同じ `get` /
    /// `set` を渡すことで排他性を表現する（専用コンテナ型は導入しない設計、
    /// `radio_button.rs` モジュール doc 参照）。
    ///
    /// **責務分担**: `InteractionState::apply` の入力処理は `RadioButton<S>` 自体を
    /// 拡張するのではなく、兄弟モジュールの [`super::RadioGroupWidget`] が別型として担う。
    /// 本ビルダーは `RadioButton::bound(..)` によるシグネチャ確定済みの契約を提供する。
    ///
    /// **実描画・配線**: `fandhe-render-egui` が実描画を担い（Issue #806）、選択変更は
    /// `Event::WidgetValueChanged` → [`super::event::set_widget_value_at`] 経由で `set` へ
    /// 届く（Issue #805）。`render_headless` は `Node::Widget` の行を出力しない契約
    /// （`ui/render.rs` 参照）を維持する。
    pub fn radio_button(
        self,
        id: WidgetId,
        label: impl Into<String>,
        value: usize,
        get: crate::state::ReadFn<S, usize>,
        set: crate::state::WriteFn<S, usize>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(RadioButton::bound(
            id, label, value, get, set,
        ))))
    }

    /// トグルスイッチウィジェットを追加する（ビルダー形式、TASK-8.3-S25 /
    /// Issue #120）。`.checkbox()` と対称の糖衣構文。
    ///
    /// `Node::Widget(Box::new(ToggleSwitch::bound(..)))` を組み立てて
    /// [`Self::with_node`] に委譲する。`id` はツリー上で一意な識別子
    /// （[`super::widget::WidgetId`]）、`get` / `set` は [`crate::state`] の SSOT
    /// 型エイリアス（[`crate::state::ReadFn`] / [`crate::state::WriteFn`]）と代入互換の
    /// 関数ポインタで、環境キャプチャクロージャを型レベルで拒否する（REQ-13）。
    ///
    /// **実描画・配線**: `fandhe-render-egui` が実描画を担い（Issue #806）、トグル操作は
    /// `Event::WidgetValueChanged` → [`super::event::set_widget_value_at`] 経由で `set` へ
    /// 届く（Issue #805）。`render_headless` は `Node::Widget` の行を出力しない契約
    /// （`ui/render.rs` 参照）を維持する。
    pub fn toggle_switch(
        self,
        id: WidgetId,
        label: impl Into<String>,
        get: crate::state::ReadFn<S, bool>,
        set: crate::state::WriteFn<S, bool>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(ToggleSwitch::bound(
            id, label, get, set,
        ))))
    }

    /// 状態 `S` に束縛されたスライダーウィジェット（[`Slider`]）を末尾に追加する
    /// （TASK-8.3-S28 / REQ-8 / REQ-13、Issue #123）。`.checkbox()` / `.toggle_switch()` と
    /// 対称の糖衣構文。
    ///
    /// `Node::Widget(Box::new(Slider::bound(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）、
    /// `min` / `max` は値の範囲（`Slider::bound` が正規化する）、`get` / `set` は
    /// [`crate::state`] の SSOT 型エイリアス（[`crate::state::ReadFn`] /
    /// [`crate::state::WriteFn`]）と代入互換の関数ポインタで、環境キャプチャクロージャを
    /// 型レベルで拒否する（REQ-13）。
    ///
    /// **実描画・配線**: `fandhe-render-egui` が実描画を担い（Issue #806）、ドラッグ・キー操作による
    /// 値変更は `Event::WidgetValueChanged` → [`super::event::set_widget_value_at`] 経由で
    /// `set` へ届く（Issue #805）。`render_headless` は `Node::Widget` の行を出力しない契約
    /// （`ui/render.rs` 参照）を維持する。
    pub fn slider(
        self,
        id: WidgetId,
        label: impl Into<String>,
        min: f32,
        max: f32,
        get: crate::state::ReadFn<S, f32>,
        set: crate::state::WriteFn<S, f32>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(Slider::bound(
            id, label, min, max, get, set,
        ))))
    }

    /// プログレスバー（determinate）ウィジェットを追加する（ビルダー形式、
    /// TASK-8.3-S31 / Issue #126）。`.checkbox()` / `.toggle_switch()` と対称の
    /// 糖衣構文。
    ///
    /// `Node::Widget(Box::new(ProgressBar::bound(..)))` を組み立てて
    /// [`Self::with_node`] に委譲する。`id` はツリー上で一意な識別子
    /// （[`super::widget::WidgetId`]）、`get` は [`crate::state`] の SSOT 型エイリアス
    /// （[`crate::state::ReadFn`]）と代入互換の関数ポインタで、環境キャプチャクロージャを
    /// 型レベルで拒否する（REQ-13）。プログレスは表示専用のため `set` を持たない
    /// （`progress` モジュール doc 参照）。
    ///
    /// `fandhe-render-egui` は egui の `ProgressBar` で描画する（Issue #1428）。一方
    /// `render_headless` 等は `Node::Widget` を状態非依存のプレースホルダとして扱う契約
    /// （[`super::widget::Widget::sync_from_state`] のドキュメンテーションコメント参照）を
    /// 維持する。`WidgetCatalog`（`catalog.rs` の `progress_bar` エントリ）には登録済み。
    pub fn progress_bar(
        self,
        id: WidgetId,
        label: impl Into<String>,
        get: crate::state::ReadFn<S, f32>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(ProgressBar::bound(id, label, get))))
    }

    /// スピナー（indeterminate）ウィジェットを追加する（ビルダー形式、
    /// TASK-8.3-S31 / Issue #126）。
    ///
    /// `Node::Widget(Box::new(Spinner::new(id)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）。
    /// 状態バインディングを持たない（`progress` モジュール doc 参照）ため `get` /
    /// `set` 引数はない。
    ///
    /// `fandhe-render-egui` は egui の `Spinner` で描画する（Issue #1428。`render_headless`
    /// 等の契約は `.progress_bar()` と同じ）。`WidgetCatalog`（`catalog.rs` の `spinner` エントリ）には登録済み。
    pub fn spinner(self, id: WidgetId) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(Spinner::new(id))))
    }

    /// 状態 `S` に依存しない固定のアセット識別子を持つ画像（[`ImageWidget`]）を末尾に追加する
    /// （TASK-8.3-S34 / REQ-8 / REQ-9 / REQ-13、Issue #129・#1427）。
    ///
    /// `Node::Widget(Box::new(ImageWidget::fixed(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する糖衣構文。`id` はツリー上で一意な識別子、`source` は画像ソース識別子、`alt` は
    /// 代替テキスト（REQ-9）。
    ///
    /// **画素はこのメソッドだけでは出ない**。`source` はファイルパスや URL を読み込むものではなく、
    /// アプリ（端末機能の effect ドライバ等）が `fandhe_runtime::image_store::put`（入口 crate からは
    /// `fandhe::runtime::image_store::put`）で同じ識別子の画素を置いたときだけ、`fandhe-render-egui` が
    /// 実画素を描く。置けるのは同時に 1 枚。一致する画素が無く `source` が空でないときは、`alt` を
    /// 文字で描く（`source` が空のときは何も描かない）。固定の画像を出すなら [`Self::rgba_image`]。
    ///
    /// Issue #1427 より、以前の [`super::image::Image`]（egui で描かれない別型）ではなく、
    /// 描かれる [`ImageWidget`] を作る。
    pub fn image(self, id: WidgetId, source: impl Into<String>, alt: impl Into<String>) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(ImageWidget::fixed(id, source, alt))))
    }

    /// 実画素（RGBA8）の画像ノード [`Node::RgbaImage`] を末尾に追加する（Issue #5）。
    /// `alt` はアクセシビリティ用の代替テキスト。`pixels` は
    /// [`super::node::RgbaPixels::new`] で検証済みの値のみ受け付ける。
    pub fn rgba_image(self, pixels: super::node::RgbaPixels, alt: impl Into<String>) -> Self {
        self.with_node(Node::RgbaImage {
            pixels,
            alt: alt.into(),
        })
    }

    /// 状態 `S` に束縛されたアセット識別子を持つ画像（[`ImageWidget`]）を末尾に追加する
    /// （TASK-8.3-S34 / REQ-8 / REQ-9 / REQ-13、Issue #129・#1427）。[`Self::image`] の状態束縛版。
    /// `get` は [`crate::state::ReadFn`] と代入互換の関数ポインタ（環境キャプチャクロージャを
    /// 型レベルで拒否する、REQ-13）。`alt` は代替テキスト（REQ-9）。
    /// 描画の条件は [`Self::image`] と同じ。
    pub fn image_bound(
        self,
        id: WidgetId,
        get: crate::state::ReadFn<S, String>,
        alt: impl Into<String>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(ImageWidget::bound(id, get, alt))))
    }

    /// 状態 `S` に依存しない固定のアイコン名を持つアイコンウィジェット（[`Icon`]）を
    /// 末尾に追加する（TASK-8.3-S39 / REQ-8 / REQ-9 / REQ-13、Issue #134）。
    /// `.image()` と対称の糖衣構文（`icon.rs` モジュール doc 参照。既存 main の API のみに
    /// 依存する自己完結実装）。
    ///
    /// `Node::Widget(Box::new(Icon::fixed(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）、`name` は
    /// アイコン名（"settings"・"search" 等のセマンティック識別子。不透明文字列であり、
    /// `fandhe-core` はグリフ解決・フォント・アセット I/O を一切行わない）、`label` は
    /// アクセシビリティラベル（REQ-9）。
    pub fn icon(self, id: WidgetId, name: impl Into<String>, label: impl Into<String>) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(Icon::fixed(id, name, label))))
    }

    /// 状態 `S` に束縛されたアイコン名を持つアイコンウィジェット（[`Icon`]）を
    /// 末尾に追加する（TASK-8.3-S39 / REQ-8 / REQ-9 / REQ-13、Issue #134）。`.icon()` の
    /// 状態束縛版で、`.image_bound()` と対称の糖衣構文。
    ///
    /// `Node::Widget(Box::new(Icon::bound(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`get` は [`crate::state`] の SSOT 型エイリアス（[`crate::state::ReadFn`]）と
    /// 代入互換の関数ポインタで、環境キャプチャクロージャを型レベルで拒否する
    /// （REQ-13）。`label` はアクセシビリティラベル（REQ-9）。
    pub fn icon_bound(
        self,
        id: WidgetId,
        get: crate::state::ReadFn<S, String>,
        label: impl Into<String>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(Icon::bound(id, get, label))))
    }

    /// 構築済みの [`Stack`] を末尾に追加する（TASK-8.3-S43 / REQ-8 / REQ-9、
    /// Issue #138）。本ウィジェットセット初の複数子コンテナであり、複数子は
    /// ビルダー引数で表現しづらいため、`.image()` のような
    /// 引数分解形式ではなく、呼び出し側で組み立て済みの `Stack<S>` を受け取る
    /// 形とする（`stack_grid` モジュール doc §3.3 参照）。
    pub fn stack(self, stack: Stack<S>) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(stack)))
    }

    /// 構築済みの [`Grid`] を末尾に追加する（TASK-8.3-S43 / REQ-8 / REQ-9、
    /// Issue #138）。[`Self::stack`] と同じ理由で、組み立て済みの `Grid<S>` を
    /// 受け取る形とする。
    pub fn grid(self, grid: Grid<S>) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(grid)))
    }

    /// 構築済みの [`DialogWidget`]（ダイアログ・モーダル）を末尾に追加する（Issue #1427）。
    /// [`Self::stack`] と同じく、組み立て済みの型を受け取り `.with_node(Node::Widget(Box::new(..)))` の
    /// 手間を省く。`fandhe-render-egui` は開状態のとき前面パネル + スクリムで描く。
    /// 開閉・子ノード（`with_content`）・外側タップで閉じるための `dismiss_label` と同名の
    /// `Node::Button` の約束は `DialogWidget` と `docs/ai/SCHEMA.md` を参照。
    /// 以前あった `.dialog(id, title, body, open)` は描かれない [`super::dialog::Dialog`] を作っていたため削除した。
    pub fn dialog(self, dialog: DialogWidget<S>) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(dialog)))
    }

    /// メニュー・ドロップダウンを構築して追加する糖衣構文（TASK-8.3-S49 / REQ-8、
    /// Issue #144。[`Self::dialog`] と対称のパターン）。
    ///
    /// `Node::Widget(Box::new(Menu::bound(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）、
    /// `open` は開閉状態、`selected` は選択中項目 index（`Option<usize>`）を読み取る
    /// [`crate::state::ReadFn`]。`set_open` は開閉の、`set_selected` は選んだ項目 index の
    /// 書き戻し先 [`crate::state::WriteFn`]（いずれも環境キャプチャクロージャを型レベルで
    /// 拒否する、REQ-13）。利用者がトリガーを押す・外側を押す・項目を選ぶと、
    /// `fandhe-render-egui` が `Event::WidgetValueChanged`（開閉は `WidgetValue::Bool`、
    /// 選択は `WidgetValue::Index`）を積み、`Widget::apply_value` が書き戻す。項目を選ぶと
    /// `set_selected(i)` に続けて `set_open(false)` も呼ばれる（Issue #1429）。
    ///
    /// `render_headless` 等は `Node::Widget` を状態非依存のプレースホルダとして
    /// 扱う契約（`menu` モジュール doc 参照）を維持する。描画は `fandhe-render-egui` のみ。
    /// `WidgetCatalog`（`catalog.rs` の `menu` エントリ）には登録済み。
    // 糖衣構文が `Menu::bound` の 7 引数に `self` を足すため 8 になる。引数を束ねる型は
    // 呼び出し側の書き味を悪くするだけなので足さない。
    #[allow(clippy::too_many_arguments)]
    pub fn menu(
        self,
        id: WidgetId,
        label: impl Into<String>,
        items: Vec<String>,
        open: crate::state::ReadFn<S, bool>,
        set_open: crate::state::WriteFn<S, bool>,
        selected: crate::state::ReadFn<S, Option<usize>>,
        set_selected: crate::state::WriteFn<S, usize>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(Menu::bound(
            id,
            label,
            items,
            open,
            set_open,
            selected,
            set_selected,
        ))))
    }

    /// タブを構築して追加する糖衣構文（TASK-8.3-S52 / REQ-8、Issue #147。
    /// [`Self::menu`] S49 と対称のパターン）。
    ///
    /// `Node::Widget(Box::new(Tabs::bound(..)))` を組み立てて [`Self::with_node`] に
    /// 委譲する。`id` はツリー上で一意な識別子（[`super::widget::WidgetId`]）、
    /// `labels` は各タブの見出し文言、`selected` は選択中タブ index（`usize`）を
    /// 読み取る [`crate::state::ReadFn`]、`set_selected` は利用者が選んだタブ index の
    /// 書き戻し先 [`crate::state::WriteFn`]（いずれも環境キャプチャクロージャを型レベルで
    /// 拒否する、REQ-13）。タブを押すと `fandhe-render-egui` が `Event::WidgetValueChanged`
    /// （`WidgetValue::Index`）を積み、`Widget::apply_value` が `set_selected` へ書き戻す
    /// （Issue #1429）。選択中タブの中身は、状態を読む `.list()` 等を後ろに置いて出し分ける
    /// （`Tabs::with_content` の中身は egui では描かれない）。
    ///
    /// `render_headless` 等は `Node::Widget` を状態非依存のプレースホルダとして
    /// 扱う契約（`tabs` モジュール doc 参照）を維持する。描画は `fandhe-render-egui` のみ。
    /// `WidgetCatalog`（`catalog.rs` の `tabs` エントリ）には登録済み。
    pub fn tabs(
        self,
        id: WidgetId,
        labels: Vec<String>,
        selected: crate::state::ReadFn<S, usize>,
        set_selected: crate::state::WriteFn<S, usize>,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(Tabs::bound(
            id,
            labels,
            selected,
            set_selected,
        ))))
    }

    /// 固定テキスト・アンカー必須のツールチップを構築して追加する糖衣構文
    /// （TASK-8.3-S57 / REQ-8、Issue #152。[`Self::dialog`] / [`Self::tabs`] と
    /// 対称のパターン）。
    ///
    /// `Node::Widget(Box::new(Tooltip::fixed(id, text).with_anchor(anchor)))` を
    /// 組み立てて [`Self::with_node`] に委譲する。`id` はツリー上で一意な識別子
    /// （[`super::widget::WidgetId`]）、`text` は本文テキスト、`anchor` はツールチップが
    /// 装飾する対象ウィジェットである。状態 `S` に束縛された本文（`Tooltip::bound`）や
    /// アンカーなし構成が必要な場合は、組み立て済み `Tooltip<S>` を [`Self::with_node`] へ
    /// 直接渡すこと（本ビルダーは最小構成のみを提供する）。
    ///
    /// **未配線**（`tooltip` モジュール doc 参照）: `render_headless` は
    /// `Node::Widget` を状態非依存のプレースホルダとして扱う契約を維持する。
    /// hover/focus 連動の表示・非表示は未着手であり、本ビルダーは
    /// `Tooltip::show` / `Tooltip::hide` による明示的な表示制御のみを前提とする。
    pub fn tooltip(
        self,
        id: WidgetId,
        text: impl Into<String>,
        anchor: impl Widget<S> + 'static,
    ) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(
            Tooltip::fixed(id, text).with_anchor(anchor),
        )))
    }

    /// 構築済みの [`Card`] を末尾に追加する（TASK-8.3-S60 / REQ-8、Issue #155。
    /// [`Self::stack`] / [`Self::grid`] と同じ理由で、複数子コンテナは呼び出し側で
    /// 組み立て済みの `Card<S>` を受け取る形とする）。
    ///
    /// **未配線**（`card` モジュール doc 参照）: `render_headless` は
    /// `Node::Widget` を状態非依存のプレースホルダとして扱う契約を維持する。
    /// カードのタップ・hover エレベーション等のイベント配線は未着手。
    pub fn card(self, card: Card<S>) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(card)))
    }

    /// 構築済みの [`Separator`] を末尾に追加する（TASK-8.3-S60 / REQ-8、Issue #155。
    /// [`Self::card`] と対称のパターン）。
    ///
    /// `fandhe-render-egui` は egui の `Separator` で描画する（Issue #1428。縦線は親が横並び
    /// のときだけで、それ以外は横線）。`render_headless` は `Node::Widget` を状態非依存の
    /// プレースホルダとして扱う契約（`card` モジュール doc 参照）を維持する。
    /// セパレータは装飾専用で `hit_test` が常に `false` を返すため、状態・イベント
    /// 処理側の対象にもならない。
    pub fn separator(self, separator: Separator<S>) -> Self
    where
        S: 'static,
    {
        self.with_node(Node::Widget(Box::new(separator)))
    }

    /// 保持しているノードを挿入順に走査し、各ノードへ `f` を適用する。
    ///
    /// 描画マッピング（プラットフォームレイヤ）・イベント配線（S5）は
    /// いずれも本メソッドを共通の走査基盤として利用することを想定している。
    /// `List` の中身は展開しない単純走査であるため、子要素を実際に描く経路には
    /// [`Screen::visit_expanded`] を利用する想定。
    pub fn visit<F: FnMut(&Node<S>)>(&self, mut f: F) {
        for node in &self.nodes {
            f(node);
        }
    }

    /// 保持しているノードを挿入順に走査し、`List` / `VirtualList` ノードは `state` を
    /// 渡して子ノードを展開したうえで再帰的に走査する（ネストにも対応する、
    /// TASK-1.1-S4 / REQ-1。`VirtualList` は TASK-5.5-S2 / REQ-5 の機構）。
    ///
    /// `event::click_button` 等の単純な線形走査（[`Self::visit`] / [`Self::iter`] を介する経路）は
    /// `List` / `VirtualList` の中身を展開しないため、プラットフォームレイヤの描画走査
    /// （子要素を実際に描く経路）は本メソッドを利用する想定。
    ///
    /// `VirtualList` は `Node::expand_children` と同じく `0..len(state)` の全項目を
    /// 展開する（可視範囲に限定した性能経路が必要な呼び出し元は
    /// [`Self::visit_windowed`] を使うこと。本メソッドは既存の描画・スナップショット
    /// 走査経路の意味論的後方互換を優先する）。
    pub fn visit_expanded<F: FnMut(&Node<S>)>(&self, state: &S, mut f: F) {
        for node in &self.nodes {
            Self::visit_node_expanded(node, state, &mut f);
        }
    }

    /// [`Screen::visit_expanded`] の再帰本体。`node` 自身を訪問したうえで
    /// [`Node::expand_children`] の結果を深さ優先で再帰的に訪問する。
    ///
    /// バリアントによるガードは設けない（Issue #739）。`Node::List` /
    /// `Node::VirtualList` 以外（葉ノード・`Node::Widget` 含む）は
    /// `expand_children` が空 `Vec` を返すため、無条件に呼んでも既存の走査結果は
    /// 変わらない。`Node::Widget` は [`super::widget::Widget::children`]
    /// （`DialogWidget::content` 等）経由で状態依存の子ノードを持ち得るため、これにより
    /// `render_headless` 等の共有参照走査でもダイアログ本体に続けてパネル内容が
    /// 出力される。
    fn visit_node_expanded(node: &Node<S>, state: &S, f: &mut dyn FnMut(&Node<S>)) {
        f(node);
        for child in node.expand_children(state) {
            Self::visit_node_expanded(&child, state, f);
        }
    }

    /// 可視範囲 `range` に限定してノードを走査する（TASK-5.5-S2 / REQ-5、Issue #85）。
    ///
    /// `Node::VirtualList` に遭遇した場合のみ `range`（`0..len(state)` でクランプした
    /// 範囲）内の index に `item(state, i)` を呼び出し、性能上の利点（可視範囲外の
    /// ノード生成を避ける）を提供する。`VirtualList` 以外の既存バリアント（`Text` /
    /// `TextField` / `Button` / `List`）は [`Self::visit_expanded`] と同じ全走査を
    /// 維持する（`VirtualList` を使わない既存画面の挙動を変えない後方互換性、
    /// 設計文書 §3.3）。
    ///
    /// ネストした `VirtualList` の座標系変換は扱わない: 遭遇したすべての
    /// `VirtualList` に同一の `range` をそのまま適用する（設計文書 §5 の将来課題）。
    pub fn visit_windowed<F: FnMut(&Node<S>)>(
        &self,
        state: &S,
        range: &super::virtual_list::VisibleRange,
        mut f: F,
    ) {
        for node in &self.nodes {
            Self::visit_node_windowed(node, state, range, &mut f);
        }
    }

    /// [`Screen::visit_windowed`] の再帰本体。`Node::List` / `Node::VirtualList` に加え
    /// `Node::Widget` も [`super::widget::Widget::children`] 経由で再帰的に展開する
    /// （[`Screen::visit_node_expanded`] と同じ理由、Issue #739、PR #792 codex-review
    /// 3 巡目 P2 指摘）。
    fn visit_node_windowed(
        node: &Node<S>,
        state: &S,
        range: &super::virtual_list::VisibleRange,
        f: &mut dyn FnMut(&Node<S>),
    ) {
        f(node);
        match node {
            Node::List { expand, .. } => {
                for child in expand(state) {
                    Self::visit_node_windowed(&child, state, range, f);
                }
            }
            Node::VirtualList { len, item, .. } => {
                let clamped_end = range.end.min(len(state));
                let clamped_start = range.start.min(clamped_end);
                for index in clamped_start..clamped_end {
                    let child = item(state, index);
                    Self::visit_node_windowed(&child, state, range, f);
                }
            }
            Node::Widget(widget) => {
                // `DialogWidget::children`（`with_content` 経由）等、状態依存の子ノードを
                // 持つ `Widget` を `visit_windowed` でも展開する（Issue #739、PR #792
                // codex-review 3 巡目 P2 指摘: `visit_node_expanded` は Widget へ降下するが
                // 本メソッドは List / VirtualList のみだったため、`visit_windowed` 経由の
                // 走査で確認パネルの文言・ボタンが欠落していた）。`range` は
                // VirtualList 専用のため Widget の子へはそのまま伝搬するのみで使わない。
                for child in widget.children(state) {
                    Self::visit_node_windowed(&child, state, range, f);
                }
            }
            // `Row`（Issue #1288）は評価済みの子を持つ。`expand_children` と同じく全部辿る。
            Node::Row { children, .. } => {
                for child in children {
                    Self::visit_node_windowed(child, state, range, f);
                }
            }
            _ => {}
        }
    }

    /// ノードへの挿入順のイテレータを返す。
    ///
    /// 戻り値は内部実装型（`Vec` の `slice::Iter`）を露出させず `impl Iterator` として
    /// 抽象化する。内部表現を将来ツリー構造等へ変更してもこのシグネチャは変わらない。
    pub fn iter(&self) -> impl Iterator<Item = &Node<S>> {
        self.nodes.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::super::node::Node;
    use super::super::render_mode::RenderMode;
    use super::Screen;

    struct CounterState {
        count: i32,
    }

    fn get_note(_state: &CounterState) -> String {
        "note".to_string()
    }
    fn set_note(_state: &mut CounterState, _value: String) {}
    fn on_click(state: &mut CounterState) {
        state.count += 1;
    }
    fn expand_items(_state: &CounterState) -> Vec<Node<CounterState>> {
        vec![Node::Text("a".to_string()), Node::Text("b".to_string())]
    }

    /// 各バリアントを含む `Screen` を構築でき、ノード数・順序が保持されることを検証する。
    #[test]
    fn screen_builds_tree_with_all_variants_preserving_order() {
        let screen: Screen<CounterState> = Screen::new("サンプル画面")
            .with_node(Node::Text("見出し".to_string()))
            .with_node(Node::TextField {
                label: "メモ".to_string(),
                get: get_note,
                set: set_note,
            })
            .with_node(Node::Button {
                label: "+1".to_string(),
                on_click,
                id: super::super::widget::WidgetId::from_raw(1),
            })
            .with_node(Node::List {
                expand: expand_items,
                axis: super::super::node::Axis::default(),
            });

        assert_eq!(screen.title(), "サンプル画面");
        assert_eq!(screen.len(), 4);
        assert!(!screen.is_empty());

        let labels: Vec<&'static str> = screen
            .iter()
            .map(|node| match node {
                Node::Text(_) => "text",
                Node::TextField { .. } => "text_field",
                Node::DynamicText { .. } => "dynamic_text",
                Node::StyledText { .. } => "styled_text",
                Node::Button { .. } => "button",
                Node::IndexedButton { .. } => "indexed_button",
                Node::CellButton { .. } => "cell_button",
                Node::Row { .. } => "row",
                Node::List { .. } => "list",
                Node::VirtualList { .. } => "virtual_list",
                // 本テストは TASK-8.3-S2 時点で `Widget` を使わない
                // （個別ウィジェット実装は S7 以降のスコープ）。到達しないはずの分岐だが、
                // 網羅性のために明示する（Issue #97）。
                Node::Widget(_) => "widget",
                Node::RgbaImage { .. } => "rgba_image",
            })
            .collect();
        assert_eq!(labels, vec!["text", "text_field", "button", "list"]);
    }

    /// `visit` が挿入順に全ノードを訪問することを検証する。
    #[test]
    fn visit_traverses_nodes_in_insertion_order() {
        let mut screen: Screen<CounterState> = Screen::new("走査テスト");
        screen.push(Node::Text("1".to_string()));
        screen.push(Node::Text("2".to_string()));
        screen.push(Node::Text("3".to_string()));

        let mut visited = Vec::new();
        screen.visit(|node| {
            if let Node::Text(s) = node {
                visited.push(s.clone());
            }
        });

        assert_eq!(visited, vec!["1", "2", "3"]);
    }

    /// 空の `Screen` は `is_empty` が真になり、`visit` は何も訪問しない。
    #[test]
    fn empty_screen_visits_nothing() {
        let screen: Screen<CounterState> = Screen::new("空画面");
        assert!(screen.is_empty());

        let mut count = 0;
        screen.visit(|_| count += 1);
        assert_eq!(count, 0);
    }

    /// `text()` ビルダーで `Node::Text` がツリーに追加され、文字列が保持される
    /// ことを検証する（TASK-1.1-S2 / #18）。
    #[test]
    fn text_builder_adds_text_node() {
        let screen: Screen<CounterState> = Screen::new("test").text("見出し");
        assert_eq!(screen.len(), 1);
        match screen.iter().next() {
            Some(Node::Text(text)) => assert_eq!(text, "見出し"),
            _ => panic!("Text バリアントであるべき"),
        }
    }

    /// `dynamic_text()` ビルダーで `Node::DynamicText` がツリーに追加され、`get` が
    /// 状態を経由して都度評価されることを検証する（Issue #581）。
    #[test]
    fn dynamic_text_builder_adds_dynamic_text_node() {
        fn get_count_label(state: &CounterState) -> String {
            format!("カウント: {}", state.count)
        }

        let screen: Screen<CounterState> = Screen::new("test").dynamic_text(get_count_label);
        assert_eq!(screen.len(), 1);

        match screen.iter().next() {
            Some(Node::DynamicText { get }) => {
                let state = CounterState { count: 5 };
                assert_eq!(get(&state), "カウント: 5");
            }
            _ => panic!("DynamicText バリアントであるべき"),
        }
    }

    /// `error_text()` ビルダーで `Node::StyledText { role: TextRole::Error, .. }` が
    /// ツリーに追加され、`get` が状態を経由して都度評価されることを検証する
    /// （Issue #812）。
    #[test]
    fn error_text_builder_adds_styled_text_node_with_error_role() {
        fn get_error_label(state: &CounterState) -> String {
            format!("エラー件数: {}", state.count)
        }

        let screen: Screen<CounterState> = Screen::new("test").error_text(get_error_label);
        assert_eq!(screen.len(), 1);

        match screen.iter().next() {
            Some(Node::StyledText { role, get }) => {
                assert_eq!(*role, crate::ui::TextRole::Error);
                let state = CounterState { count: 5 };
                assert_eq!(get(&state), "エラー件数: 5");
            }
            _ => panic!("StyledText バリアントであるべき"),
        }
    }

    /// `text_field()` ビルダーで `Node::TextField` がツリーに追加され、label が
    /// 保持されることに加え、実画面から取り出した `get`/`set` が状態を経由して
    /// 往復することを検証する（TASK-1.1-S2 / #18）。
    #[test]
    fn text_field_builder_adds_text_field_node() {
        let screen: Screen<CounterState> =
            Screen::new("test").text_field("メモ", get_note, set_note);
        assert_eq!(screen.len(), 1);

        match screen.iter().next() {
            Some(Node::TextField { label, .. }) => assert_eq!(label, "メモ"),
            _ => panic!("TextField バリアントであるべき"),
        }
    }

    /// `.text()` / `.text_field()` / `.button()` / `.list()` ビルダーを混在させても
    /// 挿入順が保たれることを検証する（TASK-1.1-S2 / #18、既存の直接構築版テストの
    /// ビルダー経由への拡張）。
    #[test]
    fn builders_preserve_insertion_order_when_mixed() {
        let screen: Screen<CounterState> = Screen::new("ビルダー混在画面")
            .text("見出し")
            .text_field("メモ", get_note, set_note)
            .button(super::super::widget::WidgetId::from_raw(1), "+1", on_click)
            .list(expand_items);

        assert_eq!(screen.len(), 4);
        let labels: Vec<&'static str> = screen
            .iter()
            .map(|node| match node {
                Node::Text(_) => "text",
                Node::TextField { .. } => "text_field",
                Node::DynamicText { .. } => "dynamic_text",
                Node::StyledText { .. } => "styled_text",
                Node::Button { .. } => "button",
                Node::IndexedButton { .. } => "indexed_button",
                Node::CellButton { .. } => "cell_button",
                Node::Row { .. } => "row",
                Node::List { .. } => "list",
                Node::VirtualList { .. } => "virtual_list",
                // 本テストは TASK-8.3-S2 時点で `Widget` を使わない
                // （個別ウィジェット実装は S7 以降のスコープ）。到達しないはずの分岐だが、
                // 網羅性のために明示する（Issue #97）。
                Node::Widget(_) => "widget",
                Node::RgbaImage { .. } => "rgba_image",
            })
            .collect();
        assert_eq!(labels, vec!["text", "text_field", "button", "list"]);
    }

    /// `button()` ビルダーで `Node::Button` がツリーに追加されることを確認する
    /// （TASK-1.1-S3 由来）。
    #[test]
    fn button_builder_adds_button_node() {
        let screen: Screen<CounterState> = Screen::new("test").button(
            super::super::widget::WidgetId::from_raw(1),
            "increment",
            on_click,
        );
        assert_eq!(screen.len(), 1);
        let labels: Vec<&str> = screen
            .iter()
            .map(|node| match node {
                Node::Button { label, .. } => label.as_str(),
                _ => panic!("Button バリアントであるべき"),
            })
            .collect();
        assert_eq!(labels, vec!["increment"]);
    }

    /// `list()` ビルダーで `Node::List` がツリーに追加されることを検証する（TASK-1.1-S4）。
    #[test]
    fn list_builder_adds_list_node() {
        let screen: Screen<CounterState> = Screen::new("リスト画面").list(expand_items);

        assert_eq!(screen.len(), 1);
        match screen.iter().next() {
            Some(Node::List { .. }) => {}
            _ => panic!("List バリアントであるべき"),
        }
    }

    /// 空の一覧を返す `expand` を持つ `List` は、`visit_expanded` で自身のみ訪問され
    /// 子ノードは 1 件も訪問されないことを検証する。
    #[test]
    fn visit_expanded_visits_only_list_itself_when_empty() {
        fn empty_expand(_state: &CounterState) -> Vec<Node<CounterState>> {
            Vec::new()
        }

        let screen: Screen<CounterState> = Screen::new("空リスト").list(empty_expand);
        let state = CounterState { count: 0 };

        let mut visited = 0;
        screen.visit_expanded(&state, |_| visited += 1);

        assert_eq!(visited, 1);
    }

    /// 子ノードを持つ `List` を `visit_expanded` すると、`List` 自身と展開された
    /// 子ノードが挿入順（展開順）で訪問されることを検証する。
    #[test]
    fn visit_expanded_visits_list_then_expanded_children_in_order() {
        let screen: Screen<CounterState> = Screen::new("リスト画面")
            .button(
                super::super::widget::WidgetId::from_raw(1),
                "見出し",
                on_click,
            )
            .list(expand_items);
        let state = CounterState { count: 0 };

        let mut visited = Vec::new();
        screen.visit_expanded(&state, |node| match node {
            Node::Button { label, .. } => visited.push(label.clone()),
            Node::Text(text) => visited.push(text.clone()),
            _ => {}
        });

        assert_eq!(visited, vec!["見出し", "a", "b"]);
    }

    /// ネストした `List`（`List` の子として `List` を展開する）が `visit_expanded` で
    /// 深さ優先に再帰展開されることを検証する。
    #[test]
    fn visit_expanded_recursively_expands_nested_list() {
        fn inner_expand(_state: &CounterState) -> Vec<Node<CounterState>> {
            vec![
                Node::Text("inner-a".to_string()),
                Node::Text("inner-b".to_string()),
            ]
        }
        fn outer_expand(_state: &CounterState) -> Vec<Node<CounterState>> {
            vec![
                Node::Text("outer-a".to_string()),
                Node::List {
                    expand: inner_expand,
                    axis: super::super::node::Axis::default(),
                },
            ]
        }

        let screen: Screen<CounterState> = Screen::new("ネストリスト").list(outer_expand);
        let state = CounterState { count: 0 };

        let mut visited = Vec::new();
        screen.visit_expanded(&state, |node| {
            if let Node::Text(text) = node {
                visited.push(text.clone());
            }
        });

        assert_eq!(visited, vec!["outer-a", "inner-a", "inner-b"]);
    }

    /// `radio_button()` ビルダーで `Node::Widget` がツリーに追加されることを検証する
    /// （TASK-8.3-S22 / Issue #117）。`RadioButton` 自体の詳細な挙動テスト（排他選択・
    /// 描画・ヒットテスト等）は `ui/radio_button.rs` のユニットテストが担い、本テストは
    /// `Screen` 側のツリー構築が壊れないことのみを確認する。
    #[test]
    fn radio_button_builder_adds_widget_node() {
        fn get_selected(_state: &CounterState) -> usize {
            0
        }
        fn set_selected(_state: &mut CounterState, _value: usize) {}

        let screen: Screen<CounterState> = Screen::new("ラジオボタン画面").radio_button(
            super::WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        assert_eq!(screen.len(), 1);
        match screen.iter().next() {
            Some(Node::Widget(_)) => {}
            _ => panic!("Widget バリアントであるべき"),
        }
    }

    /// `.image()` / `.image_bound()` が egui で描かれる `ImageWidget` を作ることの判定（Issue #1427）。
    /// 描かれない別型 `Image` へ戻すと偽になる。
    fn is_image_widget(node: Option<&Node<CounterState>>) -> bool {
        matches!(node, Some(Node::Widget(w))
            if w.as_any().is_some_and(|a| a.is::<super::ImageWidget<CounterState>>()))
    }

    /// `image()` ビルダーで `ImageWidget` の `Node::Widget` がツリーに追加されることを検証する
    /// （TASK-8.3-S34 / Issue #129）。`ImageWidget` 自体の詳細な挙動テスト（measure・paint・
    /// hit_test 等）は `ui/image_widget.rs` のユニットテストが担い、本テストは `Screen` 側の
    /// ツリー構築が壊れないことのみを確認する。
    #[test]
    fn image_builder_adds_widget_node() {
        let screen: Screen<CounterState> =
            Screen::new("画像画面").image(super::WidgetId::from_raw(1), "assets/logo.png", "ロゴ");

        assert_eq!(screen.len(), 1);
        assert!(is_image_widget(screen.iter().next()));
    }

    /// `image_bound()` ビルダーで `Node::Widget` がツリーに追加されることを検証する
    /// （TASK-8.3-S34 / Issue #129）。`.image()` と同じく `Screen` 側のツリー構築の
    /// みを確認し、状態束縛の挙動詳細は `ui/image_widget.rs` のユニットテストに委ねる。
    #[test]
    fn image_bound_builder_adds_widget_node() {
        fn get_avatar_url(_state: &CounterState) -> String {
            "https://example.com/avatar.png".to_string()
        }

        let screen: Screen<CounterState> = Screen::new("画像画面（状態束縛）").image_bound(
            super::WidgetId::from_raw(1),
            get_avatar_url,
            "アバター",
        );

        assert_eq!(screen.len(), 1);
        assert!(is_image_widget(screen.iter().next()));
    }

    /// `icon()` ビルダーで `Node::Widget` がツリーに追加されることを検証する
    /// （TASK-8.3-S39 / Issue #134）。`Icon` 自体の詳細な挙動テスト（measure・paint・
    /// hit_test 等）は `ui/icon.rs` のユニットテストが担い、本テストは `Screen` 側の
    /// ツリー構築が壊れないことのみを確認する。
    #[test]
    fn icon_builder_adds_widget_node() {
        let screen: Screen<CounterState> =
            Screen::new("アイコン画面").icon(super::WidgetId::from_raw(1), "settings", "設定");

        assert_eq!(screen.len(), 1);
        match screen.iter().next() {
            Some(Node::Widget(_)) => {}
            _ => panic!("Widget バリアントであるべき"),
        }
    }

    /// `icon_bound()` ビルダーで `Node::Widget` がツリーに追加されることを検証する
    /// （TASK-8.3-S39 / Issue #134）。`.icon()` と同じく `Screen` 側のツリー構築の
    /// みを確認し、状態束縛の挙動詳細は `ui/icon.rs` のユニットテストに委ねる。
    #[test]
    fn icon_bound_builder_adds_widget_node() {
        fn get_active_icon(_state: &CounterState) -> String {
            "search".to_string()
        }

        let screen: Screen<CounterState> = Screen::new("アイコン画面（状態束縛）").icon_bound(
            super::WidgetId::from_raw(1),
            get_active_icon,
            "現在のアイコン",
        );

        assert_eq!(screen.len(), 1);
        match screen.iter().next() {
            Some(Node::Widget(_)) => {}
            _ => panic!("Widget バリアントであるべき"),
        }
    }

    /// `render_mode` の既定値が `Canvas` であることを検証する（REQ-4 / TASK-4.4-S3、
    /// 既存挙動を変えない安全側の既定）。
    #[test]
    fn render_mode_defaults_to_canvas() {
        let screen: Screen<CounterState> = Screen::new("既定値確認");
        assert_eq!(screen.render_mode(), RenderMode::Canvas);
    }

    /// `with_render_mode` で指定した値が `render_mode()` に反映されることを検証する
    /// （REQ-4 / TASK-4.4-S3）。
    #[test]
    fn with_render_mode_overrides_default() {
        let screen: Screen<CounterState> =
            Screen::new("DOM 出力画面").with_render_mode(RenderMode::Dom);
        assert_eq!(screen.render_mode(), RenderMode::Dom);
    }

    /// `clone()` した `Screen` が `render_mode` も保持することを検証する
    /// （手動実装の `Clone` にフィールド追随漏れがないことの回帰防止）。
    #[test]
    fn clone_preserves_render_mode() {
        let screen: Screen<CounterState> =
            Screen::new("複製確認").with_render_mode(RenderMode::Dom);
        let cloned = screen.clone();
        assert_eq!(cloned.render_mode(), RenderMode::Dom);
    }

    // --- VirtualList（TASK-5.5-S2 / REQ-5、Issue #85） ---

    /// `VirtualList` 専用のテスト状態。項目数のみを持つ。
    struct VirtualListState {
        len: usize,
    }

    fn virtual_len(state: &VirtualListState) -> usize {
        state.len
    }

    fn virtual_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
        Node::Text(format!("item-{index}"))
    }

    /// `virtual_list()` ビルダーで `Node::VirtualList` がツリーに追加されることを検証する。
    #[test]
    fn virtual_list_builder_adds_virtual_list_node() {
        let screen: Screen<VirtualListState> =
            Screen::new("仮想化リスト画面").virtual_list(virtual_len, virtual_item, 40.0);

        assert_eq!(screen.len(), 1);
        match screen.iter().next() {
            Some(Node::VirtualList { item_extent, .. }) => assert_eq!(*item_extent, 40.0),
            _ => panic!("VirtualList バリアントであるべき"),
        }
    }

    /// `visit_expanded` は `VirtualList` を `0..len(state)` の全項目へ展開する
    /// （既存の描画・スナップショット走査経路の意味論的後方互換を優先する設計判断）。
    #[test]
    fn visit_expanded_fully_expands_virtual_list() {
        let screen: Screen<VirtualListState> = Screen::new("仮想化リスト画面")
            .text("見出し")
            .virtual_list(virtual_len, virtual_item, 40.0);
        let state = VirtualListState { len: 5 };

        let mut visited = Vec::new();
        screen.visit_expanded(&state, |node| {
            if let Node::Text(text) = node {
                visited.push(text.clone());
            }
        });

        assert_eq!(
            visited,
            vec!["見出し", "item-0", "item-1", "item-2", "item-3", "item-4"]
        );
    }

    /// `visit_windowed` は `range` に含まれる index のみへ `item` を呼び出す
    /// （性能経路の中心的な契約）。
    #[test]
    fn visit_windowed_visits_only_indices_within_range() {
        let screen: Screen<VirtualListState> =
            Screen::new("仮想化リスト画面").virtual_list(virtual_len, virtual_item, 40.0);
        let state = VirtualListState { len: 300 };

        let mut visited = Vec::new();
        screen.visit_windowed(&state, &(100..103), |node| {
            if let Node::Text(text) = node {
                visited.push(text.clone());
            }
        });

        assert_eq!(visited, vec!["item-100", "item-101", "item-102"]);
    }

    /// `range` が `len(state)` を超える場合、`len(state)` でクランプされ panic しない。
    #[test]
    fn visit_windowed_clamps_range_to_len() {
        let screen: Screen<VirtualListState> =
            Screen::new("仮想化リスト画面").virtual_list(virtual_len, virtual_item, 40.0);
        let state = VirtualListState { len: 5 };

        let mut visited = Vec::new();
        screen.visit_windowed(&state, &(3..1000), |node| {
            if let Node::Text(text) = node {
                visited.push(text.clone());
            }
        });

        assert_eq!(visited, vec!["item-3", "item-4"]);
    }

    /// 空の `range` は 1 件も `item` を呼び出さない。
    #[test]
    fn visit_windowed_visits_nothing_for_empty_range() {
        let screen: Screen<VirtualListState> =
            Screen::new("仮想化リスト画面").virtual_list(virtual_len, virtual_item, 40.0);
        let state = VirtualListState { len: 300 };

        let mut visited = 0;
        screen.visit_windowed(&state, &(150..150), |_| visited += 1);

        // `VirtualList` 自身への訪問（f(node) 呼び出し）は 1 回発生するが、
        // 展開された子ノードは 0 件のはず。
        assert_eq!(visited, 1);
    }

    /// `visit_windowed` は `VirtualList` 以外の既存バリアント（`Text` / `TextField` /
    /// `Button` / `List`）を `visit_expanded` と同じ全走査で訪問する
    /// （`VirtualList` を使わない既存画面の挙動を変えない後方互換性）。
    #[test]
    fn visit_windowed_visits_non_virtual_list_nodes_fully() {
        let screen: Screen<CounterState> = Screen::new("混在画面")
            .text("見出し")
            .button(super::super::widget::WidgetId::from_raw(1), "+1", on_click)
            .list(expand_items);
        let state = CounterState { count: 0 };

        let mut visited = Vec::new();
        screen.visit_windowed(&state, &(0..0), |node| match node {
            Node::Text(text) => visited.push(text.clone()),
            Node::Button { label, .. } => visited.push(label.clone()),
            _ => {}
        });

        assert_eq!(visited, vec!["見出し", "+1", "a", "b"]);
    }

    /// `menu()` ビルダーで `Node::Widget` がツリーに追加されることを検証する
    /// （TASK-8.3-S49 / Issue #144）。`Menu` 自体の詳細な挙動テスト（measure・paint・
    /// hit_test 等）は `ui/menu.rs` のユニットテストが担い、本テストは `Screen` 側の
    /// ツリー構築が壊れないことのみを確認する。
    #[test]
    fn menu_builder_adds_widget_node() {
        fn get_menu_open(_state: &CounterState) -> bool {
            false
        }
        fn get_selected(_state: &CounterState) -> Option<usize> {
            None
        }
        fn set_menu_open(_state: &mut CounterState, _open: bool) {}
        fn set_selected(_state: &mut CounterState, _index: usize) {}

        let screen: Screen<CounterState> = Screen::new("メニュー画面").menu(
            super::WidgetId::from_raw(1),
            "選択してください",
            vec!["項目A".to_string(), "項目B".to_string()],
            get_menu_open,
            set_menu_open,
            get_selected,
            set_selected,
        );

        assert_eq!(screen.len(), 1);
        match screen.iter().next() {
            Some(Node::Widget(_)) => {}
            _ => panic!("Widget バリアントであるべき"),
        }
    }

    /// `visit_windowed` が `Row` の子を辿る（Issue #1288）。
    #[test]
    fn visit_windowed_descends_into_row() {
        let screen: Screen<CounterState> = Screen::new("r").with_node(Node::Row {
            children: vec![
                Node::Text("a".to_string()),
                Node::Button {
                    label: "b".to_string(),
                    on_click,
                    id: super::super::widget::WidgetId::from_raw(1),
                },
            ],
            tap: None,
        });
        let mut kinds = Vec::new();
        screen.visit_windowed(&CounterState { count: 0 }, &(0..0), |n| {
            kinds.push(match n {
                Node::Row { .. } => "row",
                Node::Text(_) => "text",
                Node::Button { .. } => "button",
                _ => "other",
            })
        });
        assert_eq!(kinds, vec!["row", "text", "button"]);
    }
}
