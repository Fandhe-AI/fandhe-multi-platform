//! `Screen<S>` を構成するノードの型定義（TASK-1.1-S1 / S3 / S4）。
//!
//! 状態アクセスは `fn(&S) -> T` / `fn(&mut S, T)` の関数ポインタに限定する。
//! クロージャ（環境キャプチャ）を受け付けないことで、状態 `S` への暗黙の外部参照を
//! 型システムで拒否し、AI 生成コードに対する安全網とする（spec REQ-13 / NFR-9、
//! PoC-6 で実証済みの設計を踏襲）。
//!
//! `List` バリアントの子要素展開（TASK-1.1-S4 / REQ-1）は [`Node::expand_children`] と
//! [`super::screen::Screen::visit_expanded`] で実装する。プラットフォームレイヤは
//! `visit_expanded` を描画走査の基盤として利用する想定。
//!
//! 各バリアントの DSL ビルダーメソッド（`.text()` / `.text_field()` 等）は
//! [`super::screen::Screen`] が提供する（TASK-1.1-S2 / #18）。描画マッピングは
//! [`super::render::render_headless`] が [`super::screen::Screen::visit_expanded`] の
//! 上に構築している。状態管理・イベント配線全体は Issue #21 が担う。
//!
//! TASK-13.1-S4（REQ-13）: 読み取り方向の状態アクセス（`Node::TextField::get` /
//! `Node::List::expand`）は、`state` モジュールが提供する SSOT 型エイリアス
//! [`crate::state::ReadFn`]（実体は同一の `fn(&S) -> T`）へ統合し、関数ポインタ表記の
//! 集約先を明示する（非破壊変更。型の実体は変わらない）。書き込み方向
//! （`Node::TextField::set`）は [`crate::state::WriteFn`] へ統合済み（本ファイル参照、
//! Issue #307）。
//!
//! TASK-8.3-S2（REQ-8 / REQ-9 / Issue #97）: 18 種のウィジェット（ボタン〜カード・
//! セパレータ）を追加する際、既存 4 バリアントと同様に enum バリアントを増やし続けると
//! `match` 網羅性の破壊が走査コード全体へ線形に伝播する（`docs/architecture/widget-architecture.md`
//! §4.1）。これを避けるため、共通トレイト [`super::widget::Widget`] を実装する型を
//! 1 バリアント [`Node::Widget`] へ集約する。既存 4 バリアントは移行コスト回避のため
//! 温存し、新規ウィジェットのみ `Node::Widget` 経由でトレイトオブジェクト化する
//! 段階的移行方針を取る（設計文書 §4.1「推奨理由」）。
//!
//! TASK-5.5-S2（REQ-5 / Issue #85）: `Node::List` は都度全項目を展開するため、
//! 300 項目規模のリストでは毎フレーム全件分の `Vec<Node<S>>` アロケーションが
//! 発生する（親 issue #83、`docs/architecture/list-virtualization-architecture.md`）。
//! これを避けるため、index ベースで項目を遅延展開する [`Node::VirtualList`] を
//! 追加した（設計文書 §3.3 の契約）。[`Screen::visit_windowed`][super::screen::Screen::visit_windowed]
//! が可視範囲 [`super::virtual_list::VisibleRange`] 内の index のみへ `item` を
//! 呼び出す性能経路を担う一方、[`Node::expand_children`] / `visit_expanded` 経由の
//! 走査は `VirtualList` も全展開する（既存の描画・スナップショット走査経路の
//! 意味論的後方互換を優先する設計判断。性能経路は `visit_windowed` に限定する旨は
//! 各メソッドの doc に明記する）。
//!
//! Issue #581（REQ-1、PR #595 codex レビュー P1 指摘）: 単一の動的テキストを
//! `Node::List`（1 件の `Node::Text` を返す `expand`）で代用すると、`List` の
//! 「動的な子要素一覧」という契約が崩れる。`fandhe-web-dom` は `List` を
//! `<ul><li>…</li></ul>` へ、`fandhe-swiftui-bridge` は `SnapshotNode::List` へ
//! 変換する契約のため、単一ラベルがリストとしてマークアップ・アクセシビリティへ
//! 誤って公開されてしまう。状態 `S` から都度文字列を読み取る動的テキスト専用の
//! [`Node::DynamicText`] を追加し、各バックエンドで `Node::Text` と同格の
//! テキストとして変換させる（`.text()` は静的文字列のみ、本バリアントは
//! `crate::state::ReadFn<S, String>` で都度評価する点のみが異なる）。
//!
//! Issue #491（REQ-1 / REQ-13）: `Node::List::expand` / `Node::VirtualList::item` が
//! 動的生成する子の `Node::Button` は、[`super::event::click_button`] のラベル一致
//! 非展開走査では解決できない（子ノードは `Screen::iter()` に現れないため）。
//! `item`/`expand` は生成時点で項目 index を知っているため、index をノードへ
//! 埋め込む [`Node::IndexedButton`] を追加し、ツリー上の位置（path）で解決する
//! [`super::event::click_button_at`] を新設した（既存 `Node::Button` /
//! `event::click_button` は変更しない、非破壊な追加）。

/// `Node::List` / `Node::VirtualList` の子要素配置軸（TASK-17.2 の前提移行、
/// `docs/architecture/tv-focus-navigation-architecture.md` §4.3）。
///
/// 実描画後の矩形（レイアウトエンジンの計測結果）とは別種の情報であり、開発者が
/// `Screen` 構築時に明示的に宣言する意味情報である（共通コアは実測ジオメトリを
/// 持たない、`responsibility-boundary.md` §3 の責務分界を参照）。TASK-17.2 本体の
/// フォーカス移動解決（方向キーがこのコンテナの意味論をどう扱うか）が参照する
/// 唯一の判断材料となる想定で、本 sub-issue では型定義とフィールド追加のみを行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Axis {
    /// 子要素が縦一列に並ぶ（既定）。既存の縦一列前提の挙動を変えない安全側の
    /// 既定値であり、これまで `axis` を持たなかった全呼び出し元は本バリアントと
    /// 等価に振る舞う。
    #[default]
    Vertical,
    /// 子要素が横一列に並ぶ。
    Horizontal,
    /// 子要素がグリッド状に並ぶ。`columns` は 1 行あたりの列数。
    Grid {
        /// 1 行あたりの列数。`0` の扱い（`1` として扱う等）は利用側（TASK-17.2 本体）の
        /// 契約とし、本型自体は値の妥当性検証を行わない（panic を API 契約にしない、
        /// coding-rust.md のエラーハンドリング方針）。
        columns: usize,
    },
}

/// テキストが担う役割（Issue #812、親 #811 の 1/3）。
///
/// AI 型安全 DSL の方針として、DSL には色を直接書かせない。開発者は役割
/// （`Body` / `Error`）のみを宣言し、役割から実際の色（`token::semantic_color::SemanticColorRole`）
/// への解決はプラットフォームレイヤ（`fandhe-egui-proxy` 等）の `TokenResolver` が担う
/// （後続 #813）。共通コアはトークン基盤に依存しない責務分界を保つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    /// 通常の本文テキスト。
    Body,
    /// エラー・警告を示すテキスト。`SemanticColorRole::Error`（`"error"`）へ解決される
    /// 想定（#813）。form 等の「エラー:」接頭辞による代替表現を置き換える受け皿となる
    /// （#814）。
    Error,
}

/// [`RgbaPixels`] の一辺の上限（ピクセル）。WebGL2 が保証する最小の
/// `MAX_TEXTURE_SIZE` に合わせ、描画バックエンドでのテクスチャ確保失敗と
/// 巨大確保による DoS を構築時点で拒否する。
pub const RGBA_MAX_SIDE: u32 = 2048;

/// 画像ノード [`Node::RgbaImage`] が運ぶ RGBA8 画素列（行優先・パディングなし・
/// 非乗算アルファ、`fandhe_device::Image` と同じレイアウト）。
///
/// 描画バックエンド（`fandhe-egui-proxy` / `fandhe-web-dom`）は長さ不一致で panic する
/// API（egui の `ColorImage::from_rgba_unmultiplied` 等）へ渡すため、不正な値を
/// 型の外へ出さないよう検証は [`RgbaPixels::new`] の 1 か所に集約し、フィールドは
/// 非公開にする。`Arc` なので `Screen` を毎フレーム作り直して `Node` を clone しても
/// 画素はコピーされない。
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaPixels {
    width: u32,
    height: u32,
    rgba: std::sync::Arc<[u8]>,
}

impl RgbaPixels {
    /// 検証付きコンストラクタ。寸法が 0、[`RGBA_MAX_SIDE`] 超、または
    /// `rgba.len() != width * height * 4`（オーバーフローは `None`）の場合は `None`。
    pub fn new(width: u32, height: u32, rgba: impl Into<std::sync::Arc<[u8]>>) -> Option<Self> {
        if width == 0 || height == 0 || width > RGBA_MAX_SIDE || height > RGBA_MAX_SIDE {
            return None;
        }
        let rgba = rgba.into();
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (rgba.len() == expected).then_some(Self {
            width,
            height,
            rgba,
        })
    }

    /// 幅（ピクセル）。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ（ピクセル）。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// RGBA8 画素列（長さは常に `width * height * 4`）。
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

impl std::fmt::Debug for RgbaPixels {
    /// カメラ画像はプライバシー情報のため、寸法とバイト長のみ出す（画素をログへ流さない）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RgbaPixels")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("len", &self.rgba.len())
            .finish()
    }
}

/// 画面を構成する 1 要素を表す型。状態型 `S` に対してジェネリックであり、
/// 異なる状態型のノードを同一 `Screen<S>` に混在させるとコンパイルエラーになる。
///
/// `Debug` / `Clone` は手動実装する（下記参照）。全フィールド（`String` と
/// 関数ポインタ）は状態型 `S` を所有しないため、`derive` マクロが自動付与する
/// `S: Debug` / `S: Clone` という不要な境界を避けるため。
pub enum Node<S> {
    /// 静的テキスト表示。状態 `S` に依存しない。
    Text(String),
    /// ラベル付きテキスト入力欄。
    ///
    /// `get` で状態 `S` から現在値を取得し、ユーザー入力の反映は `set` を通じて
    /// 状態 `S` を書き換える。プラットフォームレイヤの描画・入力ハンドリングは
    /// S2（#18）以降で `get`/`set` をどう呼び出すかを実装する。
    TextField {
        /// 入力欄に添えるラベル文字列。
        label: String,
        /// 状態 `S` から表示用の現在値を取得する関数ポインタ。
        ///
        /// 型は `state` モジュールの [`crate::state::ReadFn`]（`fn(&S) -> String` の
        /// SSOT 型エイリアス）と同一であり、代入互換である（TASK-13.1-S4）。
        get: crate::state::ReadFn<S, String>,
        /// ユーザー入力を状態 `S` へ反映する関数ポインタ。
        ///
        /// 型は `state` モジュールの [`crate::state::WriteFn`]（`fn(&mut S, String)` の
        /// SSOT 型エイリアス）と同一であり、代入互換である（#307 で統合）。
        set: crate::state::WriteFn<S, String>,
    },
    /// ボタン。押下時に状態 `S` を変更するコールバックを持つ。
    ///
    /// `on_click` はクロージャではなく関数ポインタ `fn(&mut S)` に限定する。
    /// クロージャによる暗黙のキャプチャを型レベルで遮断し、
    /// 状態アクセスの経路を `&mut S` 引数のみに限定するための設計判断
    /// （REQ-13 系列の AI 型安全 DSL の方針・PoC-6 踏襲）。
    Button {
        /// ボタンのラベル。`event::click_button` がラベル一致検索の
        /// キーとして用いる（表示文言と検索キーを兼ねる設計）。
        label: String,
        /// 押下時に呼ばれるハンドラ。関数ポインタに限定する理由は
        /// この variant のドキュメンテーションコメントを参照。
        on_click: fn(&mut S),
        /// フォーカス識別・決定操作の安定識別子（TASK-17.2 の前提移行、
        /// `docs/architecture/tv-focus-navigation-architecture.md` §3.1）。
        /// `label`（表示文言）とは独立し、開発者が `Screen` 構築時に一意な
        /// 値を割り当てる契約（既存の [`super::widget::WidgetId`] 運用と同一）。
        id: super::widget::WidgetId,
    },
    /// 状態 `S` から都度文字列を読み取る動的テキスト表示（Issue #581 / REQ-1）。
    ///
    /// `.text()`（`Node::Text`、静的文字列）との使い分け: 表示内容が状態 `S` に
    /// 追随する必要がある場合は本バリアントを使う。`Node::List` を 1 件のみの
    /// `Node::Text` 展開で代用しない（モジュール冒頭の Issue #581 コメント参照。
    /// `List` は「動的な子要素一覧」の契約を各バックエンドが前提にしている）。
    DynamicText {
        /// 状態 `S` から表示用の現在値を取得する関数ポインタ。
        ///
        /// 型は `state` モジュールの [`crate::state::ReadFn`]（`fn(&S) -> String` の
        /// SSOT 型エイリアス）と同一であり、代入互換である。
        get: crate::state::ReadFn<S, String>,
    },
    /// 役割付きの動的テキスト表示（Issue #812、親 #811 の 1/3）。
    ///
    /// `Node::DynamicText` との使い分け: 表示内容が状態 `S` に追随するだけでなく、
    /// 意味的な役割（[`TextRole`]）を運ぶ必要がある場合は本バリアントを使う
    /// （`Screen::error_text` 参照）。`PaintCommand::StyledText`（描画バックエンドが
    /// 役割解決後の色を運ぶ描画命令）とは別物であり、こちらは role の宣言のみを持つ。
    /// 色への解決は本 Issue のスコープ外（#813 が `TokenResolver` で担う）。
    StyledText {
        /// このテキストが担う役割。
        role: TextRole,
        /// 状態 `S` から表示用の現在値を取得する関数ポインタ。
        ///
        /// 型は `state` モジュールの [`crate::state::ReadFn`]（`fn(&S) -> String` の
        /// SSOT 型エイリアス）と同一であり、代入互換である。
        get: crate::state::ReadFn<S, String>,
    },
    /// 動的な子要素一覧の表示（TASK-1.1-S4 / REQ-1）。
    ///
    /// `expand` は状態 `S` から子ノード一覧を都度生成する関数ポインタで、子ノードの型は
    /// `Node<S>` 自身に固定される。これにより子孫に任意のバリアント（ネストした `List`
    /// を含む）を持たせられる。展開は描画・走査のたびに `expand(state)` を呼び直す設計
    /// とし、状態変化を都度反映する（[`Node::expand_children`] / [`super::screen::Screen::visit_expanded`]
    /// が呼び出し元）。
    List {
        /// 状態 `S` から子ノード一覧を生成する関数ポインタ。
        ///
        /// 型は `state` モジュールの [`crate::state::ReadFn`]（`fn(&S) -> Vec<Node<S>>` の
        /// SSOT 型エイリアス）と同一であり、代入互換である（TASK-13.1-S4）。
        expand: crate::state::ReadFn<S, Vec<Node<S>>>,
        /// 子要素の配置軸（TASK-17.2 の前提移行、
        /// `docs/architecture/tv-focus-navigation-architecture.md` §4.3）。
        /// 実測ジオメトリではなく開発者が宣言する意味情報であり、後続の
        /// フォーカス移動解決（TASK-17.2 本体）が方向キーの解決規則として
        /// 参照する。既定は [`Axis::Vertical`]。
        axis: Axis,
    },
    /// index ベースで項目を遅延展開するリスト（`Node::List` の仮想化版、
    /// TASK-5.5-S2 / REQ-5、Issue #85）。
    ///
    /// `expand` の代わりに `len`（総項目数）と `item`（index から 1 項目分の
    /// `Node<S>` を生成する関数ポインタ）を持つ。[`super::screen::Screen::visit_windowed`]
    /// が可視範囲 [`super::virtual_list::VisibleRange`] に含まれる index のみへ
    /// `item` を呼び出すことで、`Node::List::expand` のような全件展開を避ける
    /// （設計文書 `docs/architecture/list-virtualization-architecture.md` §3.3）。
    /// ネストした `VirtualList` の座標系変換は扱わない（設計文書 §5 の将来課題）。
    VirtualList {
        /// 状態 `S` から総項目数を取得する関数ポインタ。
        ///
        /// 型は `state` モジュールの [`crate::state::ReadFn`]（`fn(&S) -> usize` の
        /// SSOT 型エイリアス）と同一であり、代入互換である。
        len: crate::state::ReadFn<S, usize>,
        /// 状態 `S` と index から 1 項目分の `Node<S>` を生成する関数ポインタ。
        ///
        /// クロージャの環境キャプチャを避けるため、素の関数ポインタに限定する
        /// （`Node::Button::on_click` 等と同じ AI 型安全 DSL の設計判断）。型は
        /// `state` モジュールの [`crate::state::ReadIndexedFn`]（`fn(&S, usize) -> T` の
        /// SSOT 型エイリアス）と同一である。
        item: crate::state::ReadIndexedFn<S, Node<S>>,
        /// 1 項目あたりの高さ（固定項目高さ前提、可変高さは設計文書 §5 の将来課題）。
        item_extent: f32,
        /// 子要素の配置軸（TASK-17.2 の前提移行、
        /// `docs/architecture/tv-focus-navigation-architecture.md` §4.3）。
        /// `Node::List::axis` と同じ契約。既定は [`Axis::Vertical`]。
        axis: Axis,
    },
    /// 動的生成子（`List::expand` / `VirtualList::item`）内で使う、生成時 index を
    /// 埋め込んだボタン（Issue #491 / REQ-1 / REQ-13）。
    ///
    /// `Node::Button` はラベル一致の非展開走査（`event::click_button`）でのみ解決でき、
    /// 動的生成子の子までは辿れない（`event.rs` の制約参照）。本バリアントは
    /// 生成元（`item`/`expand`）が埋め込んだ `index` を保持し、
    /// [`super::event::click_button_at`] がツリー上の位置（path）を降下して解決した
    /// うえで `on_click(state, index)` を呼び出す契約とする。
    ///
    /// `on_click` は [`crate::state::WriteFn`]（`fn(&mut S, usize)`）と同一の
    /// SSOT 型エイリアスを再利用し（新規型エイリアスを追加しない非破壊方針）、
    /// 関数ポインタに限定することでクロージャの環境キャプチャを型レベルで拒否する
    /// （`Node::Button::on_click` と同じ AI 型安全 DSL の設計判断、REQ-13）。
    IndexedButton {
        /// ボタンのラベル。`click_button_at` が path 解決後の整合性ガード
        /// （葉ノードのラベル一致確認）に使う。
        label: String,
        /// 生成元（`item`/`expand`）が埋め込む項目 index。
        index: usize,
        /// 押下時に呼ばれるハンドラ。`on_click(state, index)` の形で呼ばれる。
        on_click: crate::state::WriteFn<S, usize>,
        /// フォーカス識別・決定操作の安定識別子（TASK-17.2 の前提移行、
        /// `docs/architecture/tv-focus-navigation-architecture.md` §3.1）。
        /// 項目の並び順（`index`）ではなく、項目が表すデータの永続的な
        /// 同一性から採番することを推奨する（`index` は挿入・削除でシフトし
        /// 得るため識別子として使うと再解決の意味が崩れる、設計文書同節）。
        id: super::widget::WidgetId,
    },
    /// 共通トレイト [`super::widget::Widget`] を実装するウィジェット実体
    /// （TASK-8.3-S2 / REQ-8、Issue #97）。
    ///
    /// `docs/architecture/widget-architecture.md` §4.1 が推奨する「既存 4 バリアントは
    /// 温存し、新規ウィジェット（18 種、S7〜S60）のみ本バリアント経由でトレイト
    /// オブジェクト化する」段階的移行方針の受け皿。`dyn Widget<S>` は `Self: Sized` を
    /// 要求する標準の `Clone` を実装できないため、複製は
    /// [`super::widget::Widget::clone_box`] 経由で行う（下記手動 `Clone` 実装参照）。
    Widget(Box<dyn super::widget::Widget<S>>),
    /// 実画素（RGBA8）の画像表示（Issue #5）。`Screen::image` / `ImageWidget` が識別子
    /// 文字列しか持たないのに対し、本バリアントは画素そのものを運ぶ（カメラ撮影の
    /// サムネイル等）。静的な値で足りる（`Screen` は毎フレーム状態から作り直される）。
    RgbaImage {
        /// 検証済みの画素列。
        pixels: RgbaPixels,
        /// 代替テキスト（web の `alt`、egui の `alt_text` 等のアクセシビリティ用）。
        alt: String,
    },
}

impl<S> Node<S> {
    /// 自身が `List` または `VirtualList` の場合に `state` を渡して子ノード一覧を展開する。
    ///
    /// それ以外のバリアントに対しては空の `Vec` を返す（panic を API 契約にしない、
    /// coding-rust.md のエラーハンドリング方針）。ネストした `List` / `VirtualList` の
    /// 再帰展開は [`super::screen::Screen::visit_expanded`] が担い、本メソッドは
    /// 1 段展開のみを行う。
    ///
    /// `VirtualList` は `0..len(state)` の全 index へ `item` を呼び出す全展開を行う
    /// （TASK-5.5-S2）。既存の描画・スナップショット走査経路（`render_headless` /
    /// `fandhe-swiftui-bridge::codec` / `fandhe-web-dom::dom_tree` 等）が項目を
    /// 消さない意味論的後方互換を優先する設計判断であり、性能経路（可視範囲のみを
    /// 展開する経路）は [`super::screen::Screen::visit_windowed`] が別途担う。
    pub fn expand_children(&self, state: &S) -> Vec<Node<S>> {
        match self {
            Node::List { expand, .. } => expand(state),
            Node::VirtualList { len, item, .. } => {
                (0..len(state)).map(|i| item(state, i)).collect()
            }
            // Widget が持つ状態依存の子ノード列（`DialogWidget::content` 等）を
            // 展開する（Issue #739）。`Widget::children` の既定実装は空 `Vec` の
            // ため、オーバーライドしないウィジェットは従来どおり子を持たない。
            Node::Widget(widget) => widget.children(state),
            _ => Vec::new(),
        }
    }
}

impl<S> std::fmt::Debug for Node<S> {
    /// デバッグ出力用。関数ポインタはアドレスではなくバリアント名・ラベルのみを表示し、
    /// 後続 sub-issue（プラットフォームレイヤ・テスト）でのデバッグ出力を簡便にする。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Node::Text(text) => f.debug_tuple("Text").field(text).finish(),
            Node::TextField { label, .. } => {
                f.debug_struct("TextField").field("label", label).finish()
            }
            Node::DynamicText { .. } => f.debug_struct("DynamicText").finish(),
            Node::StyledText { role, .. } => {
                f.debug_struct("StyledText").field("role", role).finish()
            }
            Node::Button { label, id, .. } => f
                .debug_struct("Button")
                .field("label", label)
                .field("id", id)
                .finish(),
            Node::IndexedButton {
                label, index, id, ..
            } => f
                .debug_struct("IndexedButton")
                .field("label", label)
                .field("index", index)
                .field("id", id)
                .finish(),
            Node::List { axis, .. } => f.debug_struct("List").field("axis", axis).finish(),
            Node::VirtualList { item_extent, .. } => f
                .debug_struct("VirtualList")
                .field("item_extent", item_extent)
                .finish(),
            // `Widget<S>` は `std::fmt::Debug` をスーパートレイトに持つため
            // （`super::widget::Widget` 参照）、`dyn Widget<S>` はそのまま
            // `{:?}` 整形できる（TASK-8.3-S2、設計文書 §4.1「Debug 手動実装との整合」）。
            Node::Widget(widget) => f.debug_tuple("Widget").field(widget).finish(),
            Node::RgbaImage { pixels, alt } => f
                .debug_struct("RgbaImage")
                .field("pixels", pixels)
                .field("alt", alt)
                .finish(),
        }
    }
}

impl<S> Clone for Node<S> {
    /// 関数ポインタ・`String` はいずれも複製可能なため、状態型 `S` に依存せず複製できる。
    fn clone(&self) -> Self {
        match self {
            Node::Text(text) => Node::Text(text.clone()),
            Node::TextField { label, get, set } => Node::TextField {
                label: label.clone(),
                get: *get,
                set: *set,
            },
            Node::DynamicText { get } => Node::DynamicText { get: *get },
            Node::StyledText { role, get } => Node::StyledText {
                role: *role,
                get: *get,
            },
            Node::Button {
                label,
                on_click,
                id,
            } => Node::Button {
                label: label.clone(),
                on_click: *on_click,
                id: *id,
            },
            Node::IndexedButton {
                label,
                index,
                on_click,
                id,
            } => Node::IndexedButton {
                label: label.clone(),
                index: *index,
                on_click: *on_click,
                id: *id,
            },
            Node::List { expand, axis } => Node::List {
                expand: *expand,
                axis: *axis,
            },
            Node::VirtualList {
                len,
                item,
                item_extent,
                axis,
            } => Node::VirtualList {
                len: *len,
                item: *item,
                item_extent: *item_extent,
                axis: *axis,
            },
            // `dyn Widget<S>` は `Self: Sized` を要求する標準の `Clone` を実装できない
            // ため、オブジェクトセーフな複製メソッド `clone_box` へ委譲する
            // （TASK-8.3-S2、設計文書 §4.1）。
            Node::Widget(widget) => Node::Widget(widget.clone_box()),
            Node::RgbaImage { pixels, alt } => Node::RgbaImage {
                pixels: pixels.clone(),
                alt: alt.clone(),
            },
        }
    }
}

#[cfg(test)]
mod rgba_tests {
    use super::{RGBA_MAX_SIDE, RgbaPixels};

    #[test]
    fn rgba_pixels_validates() {
        assert!(RgbaPixels::new(2, 1, vec![0u8; 8]).is_some());
        assert!(RgbaPixels::new(2, 1, vec![0u8; 7]).is_none());
        assert!(RgbaPixels::new(0, 1, vec![]).is_none());
        assert!(RgbaPixels::new(1, 0, vec![]).is_none());
        assert!(RgbaPixels::new(RGBA_MAX_SIDE + 1, 1, vec![0u8; 4]).is_none());
        assert!(RgbaPixels::new(u32::MAX, u32::MAX, vec![0u8; 4]).is_none());
    }

    #[test]
    fn debug_hides_pixels() {
        let p = RgbaPixels::new(1, 1, vec![9u8, 8, 7, 6]).unwrap();
        let s = format!("{p:?}");
        assert!(s.contains("len") && !s.contains('9'), "{s}");
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis, Node};
    use crate::ui::widget::WidgetId;

    /// テスト専用の状態型。関数ポインタが状態型 `S` と整合することを検証するために使う。
    struct CounterState {
        count: i32,
        note: String,
    }

    #[test]
    fn text_node_holds_static_string() {
        let node: Node<CounterState> = Node::Text("hello".to_string());
        match node {
            Node::Text(s) => assert_eq!(s, "hello"),
            _ => panic!("Text バリアントであるべき"),
        }
    }

    #[test]
    fn text_field_get_set_are_type_checked_against_state() {
        // get/set は状態型 CounterState に対する関数ポインタとして定義され、
        // 型不整合があればこの時点でコンパイルエラーになる（型安全性の検証）。
        fn get(state: &CounterState) -> String {
            state.note.clone()
        }
        fn set(state: &mut CounterState, value: String) {
            state.note = value;
        }

        let node: Node<CounterState> = Node::TextField {
            label: "メモ".to_string(),
            get,
            set,
        };

        let mut state = CounterState {
            count: 0,
            note: "初期値".to_string(),
        };

        match node {
            Node::TextField { get, set, .. } => {
                assert_eq!(get(&state), "初期値");
                set(&mut state, "更新後".to_string());
                assert_eq!(state.note, "更新後");
            }
            _ => panic!("TextField バリアントであるべき"),
        }
    }

    #[test]
    fn dynamic_text_get_reads_current_state_value() {
        fn get(state: &CounterState) -> String {
            format!("カウント: {}", state.count)
        }

        let node: Node<CounterState> = Node::DynamicText { get };
        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };

        match node {
            Node::DynamicText { get } => {
                assert_eq!(get(&state), "カウント: 0");
                state.count = 3;
                assert_eq!(get(&state), "カウント: 3");
            }
            _ => panic!("DynamicText バリアントであるべき"),
        }
    }

    #[test]
    fn styled_text_holds_role_and_reads_current_state_value() {
        fn get(state: &CounterState) -> String {
            format!("エラー件数: {}", state.count)
        }

        let node: Node<CounterState> = Node::StyledText {
            role: super::TextRole::Error,
            get,
        };
        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };

        match node.clone() {
            Node::StyledText { role, get } => {
                assert_eq!(role, super::TextRole::Error);
                assert_eq!(get(&state), "エラー件数: 0");
                state.count = 2;
                assert_eq!(get(&state), "エラー件数: 2");
            }
            _ => panic!("StyledText バリアントであるべき"),
        }
        // clone しても role が保たれることを確認する。
        match node {
            Node::StyledText { role, .. } => assert_eq!(role, super::TextRole::Error),
            _ => panic!("StyledText バリアントであるべき"),
        }
    }

    #[test]
    fn button_on_click_mutates_state() {
        fn increment(state: &mut CounterState) {
            state.count += 1;
        }

        let node: Node<CounterState> = Node::Button {
            label: "+1".to_string(),
            on_click: increment,
            id: WidgetId::from_raw(1),
        };

        let mut state = CounterState {
            count: 0,
            note: String::new(),
        };

        match node {
            Node::Button { on_click, .. } => {
                on_click(&mut state);
                assert_eq!(state.count, 1);
            }
            _ => panic!("Button バリアントであるべき"),
        }
    }

    /// `count` 件の重複ラベルを持つ `Text` の子ノードへ展開する `List` を定義する
    /// （TASK-1.1-S4）。
    fn repeated_text_list(state: &CounterState) -> Vec<Node<CounterState>> {
        (0..state.count)
            .map(|_| Node::Text(state.note.clone()))
            .collect()
    }

    #[test]
    fn list_expand_children_reads_from_state() {
        let node: Node<CounterState> = Node::List {
            expand: repeated_text_list,
            axis: Axis::default(),
        };
        let state = CounterState {
            count: 2,
            note: "item".to_string(),
        };

        let children = node.expand_children(&state);

        assert_eq!(children.len(), 2);
        for child in &children {
            match child {
                Node::Text(text) => assert_eq!(text, "item"),
                _ => panic!("Text バリアントであるべき"),
            }
        }
    }

    #[test]
    fn list_expand_children_empty_when_state_yields_no_items() {
        let node: Node<CounterState> = Node::List {
            expand: repeated_text_list,
            axis: Axis::default(),
        };
        let state = CounterState {
            count: 0,
            note: "item".to_string(),
        };

        assert!(node.expand_children(&state).is_empty());
    }

    #[test]
    fn expand_children_is_empty_for_non_list_variants() {
        fn no_op(_state: &mut CounterState) {}
        let node: Node<CounterState> = Node::Button {
            label: "hello".to_string(),
            on_click: no_op,
            id: WidgetId::from_raw(1),
        };
        let state = CounterState {
            count: 3,
            note: "item".to_string(),
        };

        assert!(node.expand_children(&state).is_empty());
    }

    #[test]
    fn list_expand_children_supports_nested_list() {
        fn nested(_state: &CounterState) -> Vec<Node<CounterState>> {
            vec![
                Node::Text("見出し".to_string()),
                Node::List {
                    expand: repeated_text_list,
                    axis: Axis::default(),
                },
            ]
        }

        let outer: Node<CounterState> = Node::List {
            expand: nested,
            axis: Axis::default(),
        };
        let state = CounterState {
            count: 2,
            note: "item".to_string(),
        };

        let children = outer.expand_children(&state);
        assert_eq!(children.len(), 2);

        match &children[1] {
            Node::List { expand, .. } => {
                let inner = expand(&state);
                assert_eq!(inner.len(), 2);
            }
            _ => panic!("List バリアントであるべき"),
        }
    }

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

    #[test]
    fn virtual_list_len_and_item_are_type_checked_against_state() {
        // len/item は状態型 VirtualListState に対する関数ポインタとして定義され、
        // 型不整合があればこの時点でコンパイルエラーになる（型安全性の検証）。
        let node: Node<VirtualListState> = Node::VirtualList {
            len: virtual_len,
            item: virtual_item,
            item_extent: 40.0,
            axis: Axis::default(),
        };
        let state = VirtualListState { len: 3 };

        match node {
            Node::VirtualList { len, item, .. } => {
                assert_eq!(len(&state), 3);
                match item(&state, 1) {
                    Node::Text(text) => assert_eq!(text, "item-1"),
                    _ => panic!("Text バリアントであるべき"),
                }
            }
            _ => panic!("VirtualList バリアントであるべき"),
        }
    }

    /// `expand_children` は `VirtualList` を `0..len(state)` の全 index へ展開する
    /// （TASK-5.5-S2: 意味論的後方互換のための全展開経路）。
    #[test]
    fn virtual_list_expand_children_expands_all_indices() {
        let node: Node<VirtualListState> = Node::VirtualList {
            len: virtual_len,
            item: virtual_item,
            item_extent: 40.0,
            axis: Axis::default(),
        };
        let state = VirtualListState { len: 3 };

        let children = node.expand_children(&state);

        assert_eq!(children.len(), 3);
        for (i, child) in children.iter().enumerate() {
            match child {
                Node::Text(text) => assert_eq!(text, &format!("item-{i}")),
                _ => panic!("Text バリアントであるべき"),
            }
        }
    }

    /// `VirtualList` の `len` が 0 を返す場合、`expand_children` は空の `Vec` を返す。
    #[test]
    fn virtual_list_expand_children_empty_when_len_is_zero() {
        let node: Node<VirtualListState> = Node::VirtualList {
            len: virtual_len,
            item: virtual_item,
            item_extent: 40.0,
            axis: Axis::default(),
        };
        let state = VirtualListState { len: 0 };

        assert!(node.expand_children(&state).is_empty());
    }

    /// `Clone` した `VirtualList` が全フィールド（`len` / `item` / `item_extent`）を
    /// 保持することを検証する（手動実装の `Clone` にフィールド追随漏れがないことの
    /// 回帰防止）。
    #[test]
    fn virtual_list_clone_preserves_all_fields() {
        let node: Node<VirtualListState> = Node::VirtualList {
            len: virtual_len,
            item: virtual_item,
            item_extent: 40.0,
            axis: Axis::default(),
        };
        let cloned = node.clone();

        match cloned {
            Node::VirtualList { item_extent, .. } => assert_eq!(item_extent, 40.0),
            _ => panic!("VirtualList バリアントであるべき"),
        }
        let state = VirtualListState { len: 2 };
        assert_eq!(cloned.expand_children(&state).len(), 2);
    }

    /// `Debug` 出力が panic せず、バリアント名・`item_extent` を含むことを検証する
    /// （関数ポインタのアドレスは表示しない設計、他バリアントの `Debug` 実装と同じ方針）。
    #[test]
    fn virtual_list_debug_output_does_not_panic() {
        let node: Node<VirtualListState> = Node::VirtualList {
            len: virtual_len,
            item: virtual_item,
            item_extent: 40.0,
            axis: Axis::default(),
        };
        let output = format!("{node:?}");
        assert!(output.contains("VirtualList"));
        assert!(output.contains("40.0") || output.contains("40"));
    }

    /// PR #357 レビュー指摘の回帰防止テスト: `Node::Widget(Box<dyn Widget<S>>)` を
    /// 保持していても `Node<S>` / `Screen<S>` が `Send` + `Sync` であり続けることを
    /// コンパイル時に固定化する。Rust の enum は全バリアントが `Send`（`Sync`）の
    /// 場合にのみ `Send`（`Sync`）になるため、`Widget` トレイト側に `Send + Sync`
    /// 境界（`super::widget::Widget`）がないと、この関数自体がコンパイルエラーになる。
    #[test]
    fn node_and_screen_remain_send_and_sync_with_widget_variant() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Node<CounterState>>();
        assert_send_sync::<crate::ui::screen::Screen<CounterState>>();
    }
}
