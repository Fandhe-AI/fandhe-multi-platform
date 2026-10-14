//! ウィジェット共通トレイト・インタラクション状態・共通スタイリング基盤
//! （TASK-8.3-S2 / REQ-8 / REQ-9 / REQ-13、Issue #97）。
//!
//! 親設計文書 `docs/architecture/widget-architecture.md`（TASK-8.3-S1、#96）の
//! §4.3 が確定したシグネチャ契約を Rust コードとして定義するモジュールであり、
//! 以下を提供する。
//!
//! - [`WidgetId`] / [`Measurement`] / [`PaintCommand`] / [`AccessibilityRole`]:
//!   `Widget<S>` トレイトが使うプレースホルダ型
//! - [`InteractionState`] / [`InteractionInput`]: hover/pressed/disabled/focus の
//!   一時的 UI 状態と、入力に応じた状態遷移ロジック
//! - [`VisualState`] / [`StateStyles`]: 状態 → 見た目を解決する共通スタイリング基盤
//! - [`Widget`]: ウィジェット実装層の共通契約（object safe トレイト）
//!
//! **[`PaintCommand`] の契約**（TASK-8.4-S14、デザイントークン基盤、#170）:
//! [`PaintCommand::StyledRect`] / [`PaintCommand::StyledText`] と、
//! [`Widget::paint_styled`] という既定実装付きメソッドを持つ。トークン
//! （`crate::token`）解決済みの**具体値**（`ColorValue` / `TypographyValue`）のみを
//! 運び、`TokenRef` 自体は保持しない契約とする（描画バックエンドはトークンの存在を
//! 知らない、`docs/architecture/design-token-architecture.md` §5.4 準拠）。基本
//! ウィジェット（Action/Input/Display カテゴリ、`ui/style_tokens.rs` 参照）のみが
//! `paint_styled` をオーバーライドし、Container/Overlay カテゴリ（#171）は
//! 既定実装（`paint()` への無スタイルフォールバック）のまま据え置く。
//!
//! 二層モデル（設計文書 §3）における「ウィジェット実装層」の受け皿であり、
//! アプリ状態 `S` へは一切含めない。宣言層（`Screen<S>` / `Node<S>`）の
//! 既存 API・REQ-13 の関数ポインタ限定方針は変更しない後方互換な拡張とする。
//!
//! [`Node::Widget`][super::node::Node::Widget] が `Box<dyn Widget<S>>` を保持する
//! 呼び出し元となる。

/// ウィジェット実体を画面ツリー上で一意に特定する識別子（TASK-8.3-S1 契約）。
///
/// `label` のような表示文言とは独立させ、将来のイベントディスパッチ（S3、#98）・
/// マウント/reconcile（設計文書 §4.4）のキーとして用いる。ツリー上の位置から
/// 導出する安定な値とし、ユーザー入力由来の文字列を直接キーにしない設計とする
/// （OWASP A03 対策、`.claude/rules/security.md`）。
///
/// ツリー位置（親からの子インデックス列）を `u64` へ折り畳むアルゴリズムの
/// 具体化は S3（#98）のスコープとする（設計文書 §4.2「未解決事項」）。本 issue
/// では識別子の型と最小 API（`from_raw` / `raw`）のみを定義する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WidgetId(u64);

impl WidgetId {
    /// 生の `u64` 値から `WidgetId` を構築する。
    ///
    /// ツリー位置からの折り畳みアルゴリズム自体は S3（#98）が実装するため、
    /// 本 issue の時点では変換元を問わない最小コンストラクタとして提供する。
    pub const fn from_raw(raw: u64) -> Self {
        WidgetId(raw)
    }

    /// 内部の `u64` 値を取り出す。
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// レイアウトエンジン（S4、`taffy` 導入見込み）へ渡す計測結果のプレースホルダ型。
///
/// `taffy::geometry::Size<f32>` 等の具体型への対応は S4（#99）が確定する
/// （設計文書 §4.3・§9「未解決事項」）。本 issue はプレースホルダまでを契約とする。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    /// 幅（論理ピクセル相当、単位は具体化未定・S4 のスコープ）。
    pub width: f32,
    /// 高さ（論理ピクセル相当、単位は具体化未定・S4 のスコープ）。
    pub height: f32,
}

/// 描画プリミティブへの変換結果を表すプレースホルダ型。
///
/// `render_headless`（TASK-1.1-S2）との両立を前提に、ヘッドレス環境でも
/// 生成可能な列とする（wgpu 等の具体描画 API には依存しない）。
///
/// `StyledRect` / `StyledText`（TASK-8.4-S14、#170）・`CompositionText`
/// （TASK-8.5-S5、#186）・`Circle`（#828）は既存バリアントを温存した加法的追加であり、
/// いずれも解決済み・合成済みの具体値のみを運ぶ（`crate::token::TokenRef` 等の未解決参照は
/// 保持しない）。本クレート内で `PaintCommand` を網羅 match する箇所は
/// `style_tokens::style_paint_commands` のみで、同関数はワイルドカード腕
/// （`other => 素通し`）を持つため網羅性は壊れない（各ウィジェットの `paint()` は
/// 常に `vec![...]` の直接構築であり、`fandhe-web-dom` / `fandhe-swiftui-bridge` 側も
/// テスト内でのバリアント構築のみ）ため、バリアント追加は既存契約を壊さない
/// （`ui/widget.rs` モジュール doc 参照）。`Circle` の実描画への変換は
/// `fandhe-egui-proxy::paint_command` / `fandhe-web-dom::dom_tree` が担う
/// （`fandhe-swiftui-bridge` は対象外、Issue #828）。
#[derive(Debug, Clone, PartialEq)]
pub enum PaintCommand {
    /// テキスト描画（内容・位置は各ウィジェット実装が確定する）。
    Text(String),
    /// 矩形描画（背景・枠線等、詳細は S4/S2 以降の個別ウィジェットが確定する）。
    Rect,
    /// トークン解決済みの塗り色を持つ矩形（TASK-8.4-S14、#170）。
    StyledRect {
        /// 解決済みの塗り色。
        fill: crate::token::ColorValue,
    },
    /// トークン解決済みの文字色・タイポグラフィを持つテキスト（TASK-8.4-S14、#170）。
    StyledText {
        /// 表示文言（既存 `Text` と同じくウィジェットが保持する表示値のみを運び、
        /// トークンキー・バインド識別子は含めない、OWASP A03 対策）。
        content: String,
        /// 解決済みの文字色。
        color: crate::token::ColorValue,
        /// 解決済みのタイポグラフィ値。
        typography: crate::token::TypographyValue,
    },
    /// コンポジション（未確定文字列）を含む合成テキスト描画
    /// （TASK-8.5-S5 / REQ-8、Issue #186）。
    ///
    /// `docs/architecture/text-input-state-model.md` §4.4 の
    /// `display = buffer[..anchor] ⊕ composition.text ⊕ buffer[anchor..]` を 3 セグメント
    /// のまま運び、プラットフォームレイヤが preedit 区間へ下線・背景色等の視覚表現を
    /// 適用できるようにする（装飾の具体表現はプラットフォーム慣習に委ねる）。
    /// 装飾を持たないレンダラは 3 セグメントを連結して `Text` 相当として描画してよい。
    /// 生成は [`super::composition_display::CompositionDisplay::into_paint_command`] を経由する
    /// （`TextFieldWidget::paint` が Composing 中に本バリアントを出力する。配線は
    /// S8（TASK-8.5-S8、#189）で完了済み）。
    CompositionText {
        /// 確定バッファのうち anchor より前の部分。
        before: String,
        /// 未確定文字列（preedit）。下線等の装飾対象。
        preedit: String,
        /// 確定バッファのうち anchor 以降の部分。
        after: String,
        /// preedit 内のカーソル範囲（preedit 先頭を 0 とした char 単位、`(start, end)`）。
        /// `None` は「preedit 末尾にキャレット」既定動作（`ime-architecture.md` §4.6、
        /// Web の `compositionupdate` がカーソル位置を提供しないケースのフォールバック）。
        preedit_cursor: Option<(usize, usize)>,
    },
    /// 円描画（トークン解決済みの塗り色、Issue #828）。
    ///
    /// `Rect` / `StyledRect` は暗黙に外側の `origin`/`size` を境界とするが、円は
    /// その矩形境界と一致しない（内接円では表現が破綻する用途がある）ため、
    /// 中心座標・半径を自前で持つ契約とする。egui-proxy は
    /// `paint_command::circle_shape` / `paint_command::paint_circle` へ、web-dom は
    /// `dom_tree::circle_to_dom_node`（`border-radius: 50%` の `div`）へ変換する
    /// （`fandhe-swiftui-bridge` は対象外、上記モジュール doc 参照）。
    Circle {
        /// 中心座標（論理ピクセル相当、単位は `Measurement` と同じく S4 で具体化）。
        center: (f32, f32),
        /// 半径（論理ピクセル相当）。
        radius: f32,
        /// 解決済みの塗り色。
        fill: crate::token::ColorValue,
    },
}

/// アクセシビリティロール（REQ-9 / `accesskit` 接続点）。
///
/// `accesskit::Role` への 1:1 マッピングを想定するが、`fandhe-core` は
/// `accesskit` に直接依存しない（依存方向規約、`.claude/rules/coding-rust.md`）ため、
/// 独自の列挙として定義し、プラットフォームレイヤ側で `accesskit::Role` へ変換する。
/// 18 種のウィジェットに対応する（設計文書 §4.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityRole {
    /// ボタン相当（`accesskit::Role::Button` へ対応予定）。
    /// [`super::button::Button`]（TASK-8.3-S7 / #102）が返す。
    Button,
    /// テキスト入力欄相当（`accesskit::Role::TextInput` へ対応予定）。
    TextInput,
    /// リスト相当（`accesskit::Role::List` へ対応予定）。
    List,
    /// ラベル・テキスト相当（`accesskit::Role::Label` へ対応予定、TASK-8.3-S10 / #105）。
    /// `LabelWidget`（TASK-8.3-S11 / #106）でも共用する。
    Label,
    /// チェックボックス相当（`accesskit::Role::CheckBox` へ対応予定、
    /// TASK-8.3-S19 / #114）。`checkbox::Checkbox` / `checkbox::CheckboxWidget`
    /// の双方で共用する。
    CheckBox,
    /// ラジオボタン相当（`accesskit::Role::RadioButton` へ対応予定、TASK-8.3-S22 / #117）。
    RadioButton,
    /// ラジオボタングループ相当（`accesskit::Role::RadioGroup` へ対応予定、
    /// TASK-8.3-S23 / #118）。`radio_button::RadioButton`（TASK-8.3-S22）とは別に、
    /// グループ単位の排他選択ウィジェット `radio_group::RadioGroupWidget`（TASK-8.3-S23）が使う。
    RadioGroup,
    /// トグルスイッチ相当（`accesskit::Role::Switch` へ対応予定、
    /// TASK-8.3-S25 / #120）。`CheckBox` を流用せず専用バリアントとする理由は、
    /// プラットフォーム側のアクセシビリティロール（SwiftUI `Toggle` / accesskit
    /// `Switch`）と 1:1 対応させるため（`toggle_switch::ToggleSwitch` の
    /// モジュール doc 参照）。
    Switch,
    /// スライダー相当（`accesskit::Role::Slider` へ対応予定、
    /// TASK-8.3-S28 / #123）。`slider::Slider` が実装する。
    Slider,
    /// プログレス表示（バー・スピナー双方）相当（`accesskit::Role::ProgressIndicator`
    /// へ対応予定、TASK-8.3-S31 / #126）。determinate（`progress::ProgressBar`）と
    /// indeterminate（`progress::Spinner`）を専用ロールに分けない理由は、accesskit
    /// `Role::ProgressIndicator` / SwiftUI `ProgressView` がいずれも両者を包含する
    /// ロールとして定義されているため（`progress` モジュール doc 参照）。
    /// `progress::ProgressWidget`（TASK-8.3-S32 / #127）も同じロールを使う。
    ProgressIndicator,
    /// 画像相当（`accesskit::Role::Image` へ対応予定、TASK-8.3-S34 / #129）。
    /// `image::Image` が実装する。状態・イベント処理を担う
    /// [`super::image_widget::ImageWidget`]（TASK-8.3-S35 / #130）も同じロールを共用する
    /// （`image_widget` モジュール doc 参照）。
    Image,
    /// スクロールビュー相当（`accesskit::Role::ScrollView` へ対応予定、
    /// TASK-8.3-S40 / #135）。`scroll_view::ScrollView` が実装する。
    ScrollView,
    /// レイアウト専用コンテナ相当（`accesskit::Role::Group` へ対応予定、
    /// TASK-8.3-S43 / #138）。`stack_grid::Stack` / `stack_grid::Grid` が
    /// 共用する。SwiftUI では `Group` / `VStack` / `HStack` 系が近い概念であり、
    /// 視覚的な配置のみを担うレイアウトコンテナへ個別ロールを増やさない判断
    /// （`stack_grid` モジュール doc 参照）。状態・イベント処理を担う
    /// [`super::stack_grid_widget::StackWidget`] / [`super::stack_grid_widget::GridWidget`]
    /// （TASK-8.3-S44 / #139）も同じロールを共用する。
    Group,
    /// ダイアログ・モーダル相当（`accesskit::Role::Dialog` へ対応予定、
    /// TASK-8.3-S46 / #141）。`dialog::Dialog` が実装する。状態・イベント処理を
    /// 担う `dialog_widget::DialogWidget`（TASK-8.3-S47 / #142）も同じロールを共用する。
    /// `AlertDialog` ロール分岐は行わず本バリアントへ統一する（`dialog_widget`
    /// モジュール doc 参照）。
    Dialog,
    /// メニュー・ドロップダウン相当（`accesskit::Role::ComboBox` へ対応予定、
    /// TASK-8.3-S49 / #144）。`menu::Menu` が実装する。SwiftUI では
    /// `Picker` / `Menu` が近い概念だが、プラットフォームレイヤ側でのマッピング判断は
    /// スコープ外とし、`fandhe-core` 側は accesskit の 1 ロールへ寄せる（`menu`
    /// モジュール doc 参照）。項目単位の `MenuItem` ロール分割は `PaintCommand` が
    /// 構造を表現できるようになるまでのスコープ外事項（`ProgressIndicator` の
    /// 共用ロール判断と同型）。
    ComboBox,
    /// タブリスト相当（`accesskit::Role::TabList` へ対応予定、
    /// TASK-8.3-S52 / #147）。`tabs::Tabs` が実装する。個々のタブへの
    /// `accesskit::Role::Tab` 付与は子ウィジェット階層が前提のため、`tabs` モジュール
    /// doc に記載のとおりスコープ外とする（`MenuItem` ロール分割見送りと
    /// 同型の判断）。タブはラジオボタングループと同じ「排他選択」が本質のウィジェットでも
    /// あり、状態・イベント処理を担う `tab_bar::TabBarWidget`（TASK-8.3-S53 / #148）も
    /// 同じロールを共用する（`image::Image` / `image_widget::ImageWidget` と同じ判断）。
    TabList,
    /// ツールチップ相当（`accesskit::Role::Tooltip` へ対応予定、
    /// TASK-8.3-S55 / #150）。`tooltip::Tooltip` が実装する。バブル自体は
    /// 非インタラクティブな装飾要素であり、`hit_test` はアンカー側へ委譲する
    /// （`tooltip` モジュール doc 参照）。状態・イベント処理を担う
    /// `tooltip_widget::TooltipWidget`（TASK-8.3-S56 / #151）も同じロールを共用する
    /// （`tabs::Tabs` / `tab_bar::TabBarWidget` と同じ判断）。
    Tooltip,
    /// 区切り線相当（`accesskit::Role::Splitter` へ対応予定、WAI-ARIA `separator` 相当、
    /// TASK-8.3-S58 / #153）。`card::Separator` が実装する。カード
    /// （`card::Card`）はレイアウト専用コンテナと同じ性質のため既存の
    /// [`AccessibilityRole::Group`]（TASK-8.3-S43）を共用し、専用バリアントは
    /// 増やさない（`card` モジュール doc 参照）。
    Separator,
}

/// ウィジェットへの入力を表す列挙（本 issue で新設、設計文書 §5「S2」の割当）。
///
/// ポインタ出入り・押下・解放・フォーカス移動を表現し、[`InteractionState::apply`]
/// の遷移入力として使う。外部由来の文字列を一切キーにせず、列挙のみで構成する
/// ことで OWASP A03（インジェクション）対策を型レベルで担保する
/// （`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionInput {
    /// ポインタがウィジェット領域に入った。
    PointerEntered,
    /// ポインタがウィジェット領域から出た。
    PointerExited,
    /// ポインタボタンが押下された。
    PointerPressed,
    /// ポインタボタンが解放された。
    PointerReleased,
    /// フォーカスを獲得した。
    FocusGained,
    /// フォーカスを喪失した。
    FocusLost,
}

/// ウィジェットが一時的に持つインタラクション状態（TASK-8.3-S1 契約・本 issue で実装）。
///
/// アプリ状態 `S` には含めない（設計文書 §3 二層モデル）。`Widget<S>` を実装する
/// 各ウィジェット実体が内部フィールドとして保持し、[`Self::apply`] /
/// [`Self::set_disabled`] を通じて更新する想定（`Widget<S>` トレイト自体の
/// シグネチャは設計文書 §4.3 から変更しない拡張点として提供する）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractionState {
    /// ポインタがウィジェット領域内にあるか。
    pub hovered: bool,
    /// ポインタボタンが押下されたままか。
    pub pressed: bool,
    /// 無効化されているか（`true` の間はすべての入力を無視する、[`Self::apply`] 参照）。
    pub disabled: bool,
    /// フォーカスを保持しているか（[`VisualState`] とは直交、[`Self::visual_state`] 参照）。
    pub focused: bool,
}

impl InteractionState {
    /// `input` に応じて状態を遷移させる。
    ///
    /// 状態遷移の意味論（本 issue の設計判断）:
    /// - **disabled 中はすべての入力を無視する**。無効化されたウィジェットが
    ///   hover/press/focus の見た目に反応しない、という直感的な意味論を優先する。
    /// - **`PointerExited` は `hovered` と `pressed` を同時に解除する**。押下中に
    ///   領域外へ出たら押下をキャンセルする、最も単純で安全な意味論とする
    ///   （ドラッグ継続等が必要になった場合は個別ウィジェット実装側の拡張点とし、
    ///   本トレイトのシグネチャ自体は変えない）。
    pub fn apply(&mut self, input: InteractionInput) {
        if self.disabled {
            return;
        }
        match input {
            InteractionInput::PointerEntered => self.hovered = true,
            InteractionInput::PointerExited => {
                self.hovered = false;
                self.pressed = false;
            }
            InteractionInput::PointerPressed => self.pressed = true,
            InteractionInput::PointerReleased => self.pressed = false,
            InteractionInput::FocusGained => self.focused = true,
            InteractionInput::FocusLost => self.focused = false,
        }
    }

    /// disabled フラグを切り替える。
    ///
    /// `true` にした時点で `hovered` / `pressed` / `focused` をすべて解除する。
    /// 無効化されたウィジェットが hover 等の見た目のまま残る不整合を防ぐための
    /// 意図的な副作用（本 issue の設計判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
        if disabled {
            self.hovered = false;
            self.pressed = false;
            self.focused = false;
        }
    }

    /// 見た目の解決に使う優先順位付き視覚状態を返す（[`VisualState`] 参照）。
    ///
    /// `focused` は視覚状態と直交する（フォーカスリング等の加算的装飾）ため
    /// `VisualState` には混ぜず、呼び出し元が `self.focused` を併用する設計とする
    /// （本 issue の設計判断、`docs/architecture/widget-architecture.md` を補完）。
    pub fn visual_state(&self) -> VisualState {
        if self.disabled {
            VisualState::Disabled
        } else if self.pressed {
            VisualState::Pressed
        } else if self.hovered {
            VisualState::Hovered
        } else {
            VisualState::Idle
        }
    }
}

/// 見た目の解決に使う優先順位付き視覚状態（本 issue で新設）。
///
/// 優先順位は `Disabled > Pressed > Hovered > Idle`（[`InteractionState::visual_state`]
/// が導出）。`focused` は本列挙に含まれない直交した状態であり、フォーカスリング等の
/// 加算的装飾として別途扱う（`docs/architecture/widget-architecture.md` §4.3 を
/// 補完する本 issue の設計判断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualState {
    /// 通常状態（hover/press/disabled のいずれでもない）。
    Idle,
    /// ポインタがホバー中。
    Hovered,
    /// ポインタで押下中。
    Pressed,
    /// 無効化されている（他の全状態より優先）。
    Disabled,
}

/// 状態別スタイル値のコンテナ（本 issue で新設、共通スタイリング基盤の核心）。
///
/// base 値 + 状態別 `Option<T>` オーバーライドを持ち、[`Self::resolve`] で
/// 未設定状態を base へフォールバック解決する。具体的な色・寸法等のトークン型
/// （デザイントークン、TASK-8.4）は `T` として後続タスクが注入するため、本 issue
/// では `T` を型パラメータとしたジェネリックなコンテナのみを提供する。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StateStyles<T> {
    base: T,
    hovered: Option<T>,
    pressed: Option<T>,
    disabled: Option<T>,
}

impl<T> StateStyles<T> {
    /// `base` 値のみを持つ `StateStyles` を構築する（状態別オーバーライドなし）。
    pub const fn new(base: T) -> Self {
        StateStyles {
            base,
            hovered: None,
            pressed: None,
            disabled: None,
        }
    }

    /// `Hovered` 状態のオーバーライド値を設定する（ビルダー形式）。
    pub fn with_hovered(mut self, value: T) -> Self {
        self.hovered = Some(value);
        self
    }

    /// `Pressed` 状態のオーバーライド値を設定する（ビルダー形式）。
    pub fn with_pressed(mut self, value: T) -> Self {
        self.pressed = Some(value);
        self
    }

    /// `Disabled` 状態のオーバーライド値を設定する（ビルダー形式）。
    pub fn with_disabled(mut self, value: T) -> Self {
        self.disabled = Some(value);
        self
    }

    /// `visual` に対応するスタイル値を解決する。
    ///
    /// 状態別オーバーライドが未設定の場合（`Idle` を含む）は `base` へ
    /// フォールバックする。
    pub fn resolve(&self, visual: VisualState) -> &T {
        let overridden = match visual {
            VisualState::Idle => None,
            VisualState::Hovered => self.hovered.as_ref(),
            VisualState::Pressed => self.pressed.as_ref(),
            VisualState::Disabled => self.disabled.as_ref(),
        };
        overridden.unwrap_or(&self.base)
    }
}

/// ウィジェット実装層の共通契約（TASK-8.3-S1 設計文書 §4.3 の契約を実装する
/// トレイト定義。シグネチャは設計文書から変更しない）。
///
/// [`super::node::Node::Widget`]（`Box<dyn Widget<S>>`）が保持する
/// 実体が実装する。メソッドはすべて `&self`/`&mut self` 経由（ジェネリック引数を
/// 取らない）に限定し、object safety を満たす。`std::fmt::Debug` をスーパートレイトに
/// 持つことで、`Node<S>` の手動 `Debug` 実装を「その場で読める」形で拡張できる
/// （`Debug` はオブジェクトセーフなためこの制約を課しても object safety は保たれる）。
///
/// 各ウィジェット実装は内部に [`InteractionState`] を保持し、入力イベントに応じて
/// [`InteractionState::apply`] / [`InteractionState::set_disabled`] を呼ぶことで
/// `interaction_state()` が返す値を更新する（本 issue の拡張点。トレイト自体の
/// シグネチャは変更しない）。
///
/// `Send + Sync` をスーパートレイトに持つ（PR #357 レビュー指摘対応）。
/// [`super::node::Node::Widget`] は `Box<dyn Widget<S>>` を保持するが、Rust の enum は
/// 全バリアントが `Send`（`Sync`）の場合にのみ `Send`（`Sync`）になるため、この境界が
/// ないと `Widget` バリアントの追加だけで `Node<S>` / `Screen<S>` 全体が
/// `!Send` / `!Sync` になり、ウィジェットを一切使わないツリーも含めてマルチスレッド
/// 利用（`state::StateStore` を跨スレッドで共有する等）が壊れる。
pub trait Widget<S>: std::fmt::Debug + Send + Sync {
    /// このウィジェット実体のツリー上の識別子。
    fn id(&self) -> WidgetId;

    /// アプリ状態 `S` から表示に必要な値を読み取り、内部表示用キャッシュを
    /// 更新する（reconcile フェーズ、設計文書 §4.4）。宣言層から渡された
    /// 関数ポインタ（`fn(&S) -> T` 相当、REQ-13 制約下）のみを介して `S` に触れる。
    ///
    /// **未配線の契約（TASK-8.3-S2 時点、Issue #97 レビューで判明）**:
    /// `render_headless`（`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・
    /// `to_snapshot_node`（`fandhe-swiftui-bridge`）の `Node::Widget` 走査は
    /// `&Node<S>` の共有参照のみを持ち、`&mut self` を要求する本メソッドを
    /// 呼び出す経路が存在しない。そのため上記 3 箇所は現在 `Node::Widget` を
    /// 状態非依存のプレースホルダとして扱い、`paint()` への委譲を行わない。
    /// S3（#98）で reconcile/sync パス（`&mut` を `Node::Widget` へ届ける手段）を
    /// 確立するまで、いずれのバックエンドも `paint()` を呼ぶ実マッピングへ
    /// 戻してはならない（戻すと構築時点の古いキャッシュ値を状態非依存に
    /// 描画してしまう設計的な罠を再導入する）。
    ///
    /// なお [`Widget::children`] は本メソッドを経ない共有参照走査からも呼べる
    /// （子ノード列は `state` から都度導出する契約のため）。egui-proxy は
    /// `clone_box` した上で本メソッドを呼んで sync 済みのクローンを描画に使う
    /// （Issue #739）。
    fn sync_from_state(&mut self, state: &S);

    /// レイアウトエンジン（S4）から与えられた制約のもとで自身の必要寸法を返す。
    fn measure(&self, available: Measurement) -> Measurement;

    /// 確定した配置矩形をもとに、描画プリミティブ列を生成する。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand>;

    /// ヒットテスト・イベント配送（S3）の対象になるかを判定する。
    /// 実際のディスパッチは `StateStore::dispatch` 相当の呼び出し元が
    /// `WidgetId` 一致で行う想定（設計文書 §4.2）。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool;

    /// このウィジェットのアクセシビリティロール（REQ-9 接続点）。
    fn accessibility_role(&self) -> AccessibilityRole;

    /// このウィジェットのアクセシブルネーム（REQ-9 / TASK-9.1-S2 接続点）。
    ///
    /// プラットフォームレイヤ（`fandhe-egui-proxy::accessibility::widget_ax_node`）が
    /// `accesskit::Node::set_label` へそのまま渡す値であり、画面上に既に表示されている
    /// ラベル文字列のみを返す契約とする（新規に機微情報を露出しない、OWASP A03 対策・
    /// `.claude/rules/security.md` 準拠）。
    ///
    /// 既定実装は `None`（アクセシブルネームなし）を返すため、本メソッドを
    /// オーバーライドしない既存実装は挙動不変のまま追加できる（加法的拡張点、
    /// `Widget<S>` トレイトのシグネチャ自体は変更しない設計文書 §4.3 の方針に従う）。
    /// ラベル文字列を保持するウィジェット（`Button` / `Checkbox` / `RadioButton` /
    /// `ToggleSwitch` / `Slider` / `TextInput` / `TextFieldWidget` / `Menu` /
    /// `ProgressBar` / `Label` / `LabelWidget` 等）がオーバーライドする。
    fn accessibility_label(&self) -> Option<String> {
        None
    }

    /// 現在のインタラクション状態（本 issue が更新・参照ロジックを実装）。
    fn interaction_state(&self) -> InteractionState;

    /// オブジェクトセーフな複製（`dyn Widget<S>` は標準の `Clone` を実装できない
    /// ための代替、設計文書 §4.1）。
    fn clone_box(&self) -> Box<dyn Widget<S>>;

    /// トークン解決込みの描画列を返す（TASK-8.4-S14、#170）。
    ///
    /// 既定実装は [`Widget::paint`] へフォールバックするため、本メソッドを
    /// オーバーライドしない Container/Overlay カテゴリ（S15、#171 以降のスコープ）の
    /// ウィジェットは挙動不変のままである。基本ウィジェット（Action/Input/Display、
    /// `super::style_tokens` 参照）はこのメソッドをオーバーライドし、
    /// `super::style_tokens::resolve_widget_style` の解決結果を
    /// [`PaintCommand::StyledRect`] / [`PaintCommand::StyledText`] へ組み立てる。
    /// 解決失敗（未定義キー・型不一致等）は `paint()` の無スタイル列へフォールバック
    /// し、panic しない（coding-rust.md の panic 禁止方針）。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        _resolver: &crate::token::TokenResolver,
    ) -> Vec<PaintCommand> {
        self.paint(origin, size)
    }

    /// 状態依存の子ノード列を返す（`Node::List::expand` と同じ意味論、Issue #739）。
    ///
    /// `Node::expand_children`（`ui/node.rs`）・`Screen::visit_node_expanded`
    /// （`ui/screen.rs`）・`ui::event::resolve_node_at`（`ui/event.rs`）が本メソッド
    /// 経由で子へ降下する。いずれも `&Node<S>` の共有参照走査であり
    /// [`Widget::sync_from_state`] を経由しないため、実装は内部キャッシュではなく
    /// 引数 `state` から直接開閉等の状態を判定すること（さもないと構築時点の
    /// 古い状態のまま子ノードを出し続けてしまう）。既定実装は子なし（`Vec::new()`）
    /// を返すため、本メソッドをオーバーライドしない既存ウィジェットは挙動不変。
    fn children(&self, _state: &S) -> Vec<super::node::Node<S>> {
        Vec::new()
    }

    /// プラットフォームレイヤ（`fandhe-egui-proxy::render_node` 等）が具象型へ
    /// downcast するための穴（Issue #739）。`Node::Widget(Box<dyn Widget<S>>)` の
    /// 共有参照からしか辿れない現行契約を崩さずに、`DialogWidget` のような固有
    /// フィールド（title / body / dismiss_label 等）へアクセスするために使う。
    ///
    /// 既定実装は `None`。`Self: 'static` を満たす具象型のみ `Some(self)` を返す
    /// ようオーバーライドできる（対象は egui-proxy が実描画する型に限る想定）。
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }

    /// プラットフォームレイヤが描画した標準部品（egui のネイティブチェックボックス・
    /// トグル・スライダー・ラジオ等）の値変更を状態 `S` へ書き込む穴（Issue #805 /
    /// 親 #804）。`Event::WidgetValueChanged` → `ui::event::set_widget_value_at` が
    /// `path` でこのウィジェットを解決した後に呼ぶ。実装は構築時に渡された
    /// `state::WriteFn`（関数ポインタ、`&self` から呼べる）で `S` へ書き込み、
    /// クロージャキャプチャを経由しない（REQ-13 の環境キャプチャ不可方針を維持）。
    ///
    /// 既定実装は常に `false`（値変更を持たないウィジェットの挙動不変）。
    /// オーバーライドする実装は `value` の型が自身の扱う型と一致しない場合、および
    /// 値そのものが不正（`Slider` の `NaN`・`RadioButton` の index 不一致等）な場合に
    /// `false` を返し、状態を変更しない契約とする（描画時と dispatch 時の状態乖離に
    /// よる誤反映を防ぐガード、`ButtonClickedAt` の label ガードと同じ A08 対策）。
    fn apply_value(&self, _state: &mut S, _value: &crate::state::WidgetValue) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- InteractionState ---

    #[test]
    fn default_interaction_state_is_all_false() {
        let state = InteractionState::default();
        assert!(!state.hovered);
        assert!(!state.pressed);
        assert!(!state.disabled);
        assert!(!state.focused);
    }

    #[test]
    fn pointer_entered_sets_hovered() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::PointerEntered);
        assert!(state.hovered);
    }

    #[test]
    fn pointer_exited_clears_hovered_and_pressed() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::PointerEntered);
        state.apply(InteractionInput::PointerPressed);
        assert!(state.hovered);
        assert!(state.pressed);

        state.apply(InteractionInput::PointerExited);
        assert!(!state.hovered);
        assert!(!state.pressed);
    }

    #[test]
    fn press_then_release_transitions_pressed_flag() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::PointerPressed);
        assert!(state.pressed);

        state.apply(InteractionInput::PointerReleased);
        assert!(!state.pressed);
    }

    #[test]
    fn disabled_state_ignores_all_inputs() {
        let mut state = InteractionState::default();
        state.set_disabled(true);

        state.apply(InteractionInput::PointerEntered);
        state.apply(InteractionInput::PointerPressed);
        state.apply(InteractionInput::FocusGained);

        assert!(!state.hovered);
        assert!(!state.pressed);
        assert!(!state.focused);
        assert!(state.disabled);
    }

    #[test]
    fn set_disabled_true_clears_hover_press_focus() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::PointerEntered);
        state.apply(InteractionInput::PointerPressed);
        state.apply(InteractionInput::FocusGained);
        assert!(state.hovered && state.pressed && state.focused);

        state.set_disabled(true);

        assert!(!state.hovered);
        assert!(!state.pressed);
        assert!(!state.focused);
        assert!(state.disabled);
    }

    #[test]
    fn focus_is_independent_of_hover_and_pressed() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::FocusGained);
        assert!(state.focused);
        assert!(!state.hovered);
        assert!(!state.pressed);

        state.apply(InteractionInput::PointerEntered);
        assert!(state.focused);
        assert!(state.hovered);
    }

    // --- VisualState ---

    #[test]
    fn visual_state_priority_disabled_over_pressed_over_hovered() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::PointerEntered);
        state.apply(InteractionInput::PointerPressed);
        assert_eq!(state.visual_state(), VisualState::Pressed);

        state.set_disabled(true);
        assert_eq!(state.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn visual_state_idle_when_no_interaction() {
        let state = InteractionState::default();
        assert_eq!(state.visual_state(), VisualState::Idle);
    }

    #[test]
    fn visual_state_hovered_when_only_hovered() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::PointerEntered);
        assert_eq!(state.visual_state(), VisualState::Hovered);
    }

    #[test]
    fn focused_does_not_affect_visual_state() {
        let mut state = InteractionState::default();
        state.apply(InteractionInput::FocusGained);
        assert_eq!(state.visual_state(), VisualState::Idle);
    }

    // --- StateStyles ---

    #[test]
    fn state_styles_resolves_to_base_when_no_override() {
        let styles = StateStyles::new("base-color".to_string());
        assert_eq!(styles.resolve(VisualState::Idle), "base-color");
        assert_eq!(styles.resolve(VisualState::Hovered), "base-color");
        assert_eq!(styles.resolve(VisualState::Pressed), "base-color");
        assert_eq!(styles.resolve(VisualState::Disabled), "base-color");
    }

    #[test]
    fn state_styles_resolves_overrides_per_state() {
        let styles = StateStyles::new("base".to_string())
            .with_hovered("hovered".to_string())
            .with_pressed("pressed".to_string())
            .with_disabled("disabled".to_string());

        assert_eq!(styles.resolve(VisualState::Idle), "base");
        assert_eq!(styles.resolve(VisualState::Hovered), "hovered");
        assert_eq!(styles.resolve(VisualState::Pressed), "pressed");
        assert_eq!(styles.resolve(VisualState::Disabled), "disabled");
    }

    #[test]
    fn state_styles_falls_back_to_base_for_partial_overrides() {
        let styles = StateStyles::new(0i32).with_pressed(9);

        assert_eq!(*styles.resolve(VisualState::Idle), 0);
        assert_eq!(*styles.resolve(VisualState::Hovered), 0);
        assert_eq!(*styles.resolve(VisualState::Pressed), 9);
        assert_eq!(*styles.resolve(VisualState::Disabled), 0);
    }

    // --- Widget<S> object safety / Node::Widget integration ---

    /// テスト専用の状態型。`Widget<S>` の object safety・`sync_from_state` 経由の
    /// 状態アクセスを検証するために使う。
    struct CounterState {
        count: i32,
    }

    /// テスト専用のダミーウィジェット実装。`Widget<S>` が object safe であること
    /// （`Box<dyn Widget<S>>` へ格納できること）を検証する最小実装。
    #[derive(Debug, Clone)]
    struct DummyWidget {
        id: WidgetId,
        label: String,
        interaction: InteractionState,
        cached_count: i32,
    }

    impl Widget<CounterState> for DummyWidget {
        fn id(&self) -> WidgetId {
            self.id
        }

        fn sync_from_state(&mut self, state: &CounterState) {
            self.cached_count = state.count;
        }

        fn measure(&self, _available: Measurement) -> Measurement {
            Measurement {
                width: 10.0,
                height: 10.0,
            }
        }

        fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
            vec![PaintCommand::Text(format!(
                "{}: {}",
                self.label, self.cached_count
            ))]
        }

        fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
            point.0 >= origin.0
                && point.0 <= origin.0 + size.width
                && point.1 >= origin.1
                && point.1 <= origin.1 + size.height
        }

        fn accessibility_role(&self) -> AccessibilityRole {
            AccessibilityRole::Button
        }

        fn interaction_state(&self) -> InteractionState {
            self.interaction
        }

        fn clone_box(&self) -> Box<dyn Widget<CounterState>> {
            Box::new(self.clone())
        }
    }

    #[test]
    fn widget_trait_object_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<CounterState>> = Box::new(DummyWidget {
            id: WidgetId::from_raw(1),
            label: "count".to_string(),
            interaction: InteractionState::default(),
            cached_count: 0,
        });

        let mut widget = widget;
        widget.sync_from_state(&CounterState { count: 3 });
        let commands = widget.paint(
            (0.0, 0.0),
            Measurement {
                width: 10.0,
                height: 10.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Text("count: 3".to_string())]);
    }

    #[test]
    fn widget_clone_box_preserves_state() {
        let widget: Box<dyn Widget<CounterState>> = Box::new(DummyWidget {
            id: WidgetId::from_raw(2),
            label: "count".to_string(),
            interaction: InteractionState::default(),
            cached_count: 5,
        });

        let cloned = widget.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(2));
        let commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 1.0,
                height: 1.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Text("count: 5".to_string())]);
    }

    #[test]
    fn widget_debug_output_does_not_panic() {
        let widget: Box<dyn Widget<CounterState>> = Box::new(DummyWidget {
            id: WidgetId::from_raw(3),
            label: "count".to_string(),
            interaction: InteractionState::default(),
            cached_count: 0,
        });
        let output = format!("{widget:?}");
        assert!(output.contains("DummyWidget"));
    }

    #[test]
    fn accessibility_label_default_is_none_when_not_overridden() {
        // DummyWidget は accessibility_label をオーバーライドしないため、
        // トレイト既定実装（None）がそのまま返ることを確認する（TASK-9.1-S2）。
        let widget: Box<dyn Widget<CounterState>> = Box::new(DummyWidget {
            id: WidgetId::from_raw(4),
            label: "count".to_string(),
            interaction: InteractionState::default(),
            cached_count: 0,
        });
        assert_eq!(widget.accessibility_label(), None);
    }
}
