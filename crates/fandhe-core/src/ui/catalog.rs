//! ウィジェットカタログ（サンプルギャラリー土台、TASK-8.3-S6 / REQ-8 / Issue #101）。
//!
//! 親 issue #95（ウィジェットセット新規開発）の分解 sub-issue（6/60）。
//! `docs/architecture/widget-architecture.md`（S1、PR #351）§5 が規定する
//! 「`Widget<S>` を実装する具体型（S7〜S60、18 種）を列挙・登録する仕組み」の土台として、
//! 登録・列挙 API（[`WidgetCatalog`]）とヘッドレスなサンプルギャラリー画面
//! （[`gallery_screen`]）を提供する。後続の各ウィジェット実装工程（S9・S12・S15…、
//! 計 18 件）が [`WidgetCatalog::register`] でエントリを登録する受け皿となる。
//!
//! **自己完結設計**: `recycle.rs` / `scroll.rs` / `hit_test.rs` / `layout/` が採った
//! 「既存 main の API のみに依存する自己完結モジュール」方式を踏襲し、カタログは
//! `Widget<S>` トレイトへ依存せず、既存の [`super::screen::Screen`] / [`super::node::Node`] /
//! [`super::render::render_headless`] のみで実装する（テストコードのみ検証目的で
//! `Widget<S>` / `AccessibilityRole` を参照する）。`Widget<S>` トレイト・
//! `AccessibilityRole` 等のメタ情報との本接続は後続作業（out-of-scope-tracking.md 対象）。
//!
//! **スコープ外**（PR 本文に明記・ユーザーへ報告）:
//! - `Widget<S>` トレイト実装後のカタログ接続（`AccessibilityRole` 等の追加）
//! - 実描画（`fandhe-egui-proxy`）でのギャラリーアプリ配線
//! - 個別 18 種ウィジェットのエントリ登録（S7〜S60 の各工程）

use super::button::Button;
use super::card::{Card, Separator, SeparatorOrientation};
use super::label::Label;
use super::list_view::ListView;
use super::node::Node;
use super::render::render_headless;
use super::screen::Screen;
use super::stack_grid::{Grid, Stack};
use super::text_input::TextInput;
use super::widget::WidgetId;

#[cfg(test)]
use super::widget::{AccessibilityRole, Measurement, Widget};

/// カタログエントリを粗く分類するカテゴリ（S1 設計 §5 の分類方針を踏襲）。
///
/// 18 種のウィジェットをボタン系・入力系・表示系・コンテナ系・オーバーレイ系に
/// 大別する。新規カテゴリの追加は S7 以降が必要に応じて明示的に拡張する
/// （`#[non_exhaustive]` は付けない。既存の match 網羅性チェックで拡張漏れを検知する
/// 設計判断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CatalogCategory {
    /// ボタン等、操作の起点となるウィジェット。
    Action,
    /// テキストフィールド・チェックボックス等、入力を受け取るウィジェット。
    Input,
    /// ラベル・画像・アイコン・プログレス等、情報を表示するウィジェット。
    Display,
    /// リスト・スクロール・スタック / グリッド・カード等、子要素を内包するウィジェット。
    Container,
    /// ダイアログ・メニュー・ツールチップ・タブ等、一時的に前面表示されるウィジェット。
    Overlay,
}

/// カタログ登録・検索で失敗し得るエラー（panic を API 契約にしない、coding-rust.md）。
///
/// `thiserror` は使わず手書きで実装する（S1 実装時点で `fandhe-core` の依存は
/// taffy のみであり、本 issue で新規依存を追加しない設計判断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    /// 登録しようとした `name` が既存エントリと重複している。
    DuplicateName(&'static str),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::DuplicateName(name) => {
                write!(f, "カタログエントリ名が重複しています: {name}")
            }
        }
    }
}

impl std::error::Error for CatalogError {}

/// カタログ 1 件分のメタ情報。
///
/// `name` はカタログ内で一意なキーであり、**静的文字列リテラル限定**とする。
/// 外部入力（ユーザー入力・環境変数・ネットワーク）由来の文字列をキー経路に
/// 含めない設計判断（S1 設計 §4.2 と同じ A03 インジェクション対策の考え方、
/// security.md 準拠）。
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    /// カタログ内一意キー。一覧表示・`find` 検索の両方で用いる。
    pub name: &'static str,
    /// 粗い分類（[`CatalogCategory`]）。
    pub category: CatalogCategory,
    /// 一覧表示用の 1 行説明。
    pub description: &'static str,
    /// デモ画面のヘッドレス描画行を返す関数ポインタ。
    ///
    /// クロージャの環境キャプチャを許さない関数ポインタに限定する（REQ-13 系列の
    /// AI 型安全 DSL の設計判断、`Node::Button::on_click` 等と同じ考え方）。各エントリの
    /// デモは内部で任意の状態型 `S` の `Screen<S>` を組み立てて `render_headless` した
    /// 結果を返せるため、共通の状態型を強制しない。
    pub preview: fn() -> Vec<String>,
}

/// ウィジェットカタログの登録・列挙本体。
///
/// 登録順を保持した `Vec<CatalogEntry>` を内部に持つ。同名エントリの登録は拒否し
/// （[`CatalogError::DuplicateName`]）、panic を API 契約にしない（coding-rust.md）。
#[derive(Debug, Clone, Default)]
pub struct WidgetCatalog {
    entries: Vec<CatalogEntry>,
}

impl WidgetCatalog {
    /// 空のカタログを作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// エントリを登録する。`name` が既存エントリと重複する場合は
    /// [`CatalogError::DuplicateName`] を返し、カタログの状態は変更しない。
    pub fn register(&mut self, entry: CatalogEntry) -> Result<(), CatalogError> {
        if self.find(entry.name).is_some() {
            return Err(CatalogError::DuplicateName(entry.name));
        }
        self.entries.push(entry);
        Ok(())
    }

    /// 登録順を保ったエントリ一覧を返す。
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    /// `name` に一致するエントリを検索する。見つからない場合は `None`
    /// （未登録名検索を panic ではなく `Option` で返す、coding-rust.md）。
    pub fn find(&self, name: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// `category` に属するエントリのみを登録順で返す。
    pub fn entries_in(&self, category: CatalogCategory) -> Vec<&CatalogEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.category == category)
            .collect()
    }

    /// 既存 9 バリアント（Text / TextField / Button / List / RadioButton / ToggleSwitch /
    /// Image / Icon / ScrollView）に対応する組み込みエントリを登録済みのカタログを返す。
    /// 各 preview は `sample.rs` と同じ様式で関数ポインタのみを用いて構築する。
    /// `radio_button` エントリは `RadioButton<S>`（TASK-8.3-S24 / Issue #119）を登録する。
    /// `toggle_switch` エントリは `ToggleSwitch<S>`（TASK-8.3-S27 / Issue #122）を登録する。
    /// `image` エントリは `Image<S>`（TASK-8.3-S36 / Issue #131）を登録する（ロード状態・
    /// イベント処理は別モジュール `ImageWidget<S>` のスコープであり、本エントリは公開 API
    /// のみに依存する）。`icon` エントリは `Icon<S>`（TASK-8.3-S39 / Issue #134）を
    /// 登録する（`icon.rs` モジュール doc 参照）。`scroll_view` エントリは `ScrollView<S>`
    /// （コンテナ型、`Label::fixed` を子コンテンツとして配置。TASK-8.3-S42 / Issue #137）を
    /// 登録する（`scroll_view.rs` モジュール doc 参照。状態束縛オフセットは
    /// Issue #136 のスコープとして未接続）。`stack` / `grid` エントリは `Stack<S>` /
    /// `Grid<S>`（`stack_grid.rs` 参照。TASK-8.3-S45 / Issue #140）を、`Screen::stack` /
    /// `Screen::grid` 経由で登録する（[`preview_stack`] / [`preview_grid`] 参照。状態・
    /// イベント処理は Issue #139 のスコープとして引き続き未接続、
    /// out-of-scope-tracking.md 対象）。`dialog` エントリは `Dialog<S>`（TASK-8.3-S48 /
    /// Issue #143）を `Screen::dialog` 経由で登録する（初の [`CatalogCategory::Overlay`]
    /// エントリ）。`Screen::dialog` が生成する `Dialog<S>` の `sync_from_state` は本
    /// モジュールの preview 経路（`Screen::visit` の共有参照走査）からは呼び出せない契約
    /// （`widget.rs` 参照）のため、開閉イベント処理は Issue #142 のスコープとして引き続き
    /// 未接続とする（out-of-scope-tracking.md 対象）。`menu` エントリは `Menu<S>`
    /// （`menu.rs` 参照。TASK-8.3-S51 / Issue #146）を `Screen::menu` 経由で登録する、
    /// `Overlay` カテゴリの 2 件目のエントリ。`dialog` と同じ理由で、開閉・選択イベント
    /// 配線は Issue #145 のスコープとして引き続き未接続とする）。`tabs` エントリは
    /// `Tabs<S>`（`tabs.rs` 参照。TASK-8.3-S54 / Issue #149）を `Screen::tabs` 経由で
    /// 登録する、`Overlay` カテゴリの 3 件目のエントリ。`dialog` / `menu` と同じ理由で
    /// タブ切替イベント配線は Issue #148 のスコープとして引き続き未接続とする）。
    /// `tooltip` エントリは `Tooltip<S>`（`tooltip.rs` 参照。TASK-8.3-S57 / Issue #152）を
    /// `Screen::tooltip` 経由で登録する、`Overlay` カテゴリの 4 件目のエントリ。`dialog` /
    /// `menu` / `tabs` と同じ理由で hover/focus 連動の表示・非表示配線は Issue #151 の
    /// スコープとして引き続き未接続とする）。`label` エントリは `Label<S>`（`label.rs`
    /// 参照。TASK-8.3-S12 / Issue #107）を `Screen::label` 経由で登録する、`Display`
    /// カテゴリの 3 件目のエントリ。`LabelWidget<S>`（インタラクション状態の実配線、
    /// Issue #106）は `Screen::label` が構築する `Label<S>` とは別型であり（`label.rs`
    /// モジュール doc 参照）、preview 経路からは呼び出せないため引き続き未接続とする）。
    /// `checkbox` エントリは `Checkbox<S>`（`checkbox.rs` 参照。TASK-8.3-S21 /
    /// Issue #116）を `Screen::checkbox` 経由で登録する、`Input` カテゴリの 4 件目の
    /// エントリ。`CheckboxWidget<S>`（イベント配線の別型実装、Issue #115）は preview
    /// 経路からは呼び出せないため、`label` エントリが `LabelWidget<S>` に対して採る
    /// 判断と同型で引き続き未接続とする）。`slider` エントリは `Slider<S>`（`slider.rs`
    /// 参照。TASK-8.3-S30 / Issue #125）を `Screen::slider` 経由で登録する、`Input`
    /// カテゴリの 5 件目のエントリ。`checkbox` / `label` と同じ理由でドラッグ・キー操作に
    /// よる値変更配線は Issue #124 のスコープとして引き続き未接続とする。なお本段落の
    /// 「未接続」はカタログの preview 経路に限った話で、`Screen::checkbox` /
    /// `Screen::slider` 等が組む本番経路は egui-proxy で実描画され値変更も
    /// `Event::WidgetValueChanged` で配線済み（Issue #805 / #806））。`progress_bar`
    /// / `spinner` エントリは `ProgressBar<S>` / `Spinner<S>`（TASK-8.3-S33 /
    /// Issue #128）を `Screen::progress_bar` / `Screen::spinner` 経由で登録する、
    /// `Display` カテゴリの 5 件目・6 件目のエントリ。`ProgressWidget<S>` /
    /// `ProgressMode`（状態・イベント処理の別型実装、Issue #127）は `label` エントリが
    /// `LabelWidget<S>` に対して採る判断と同型で preview 経路からは呼び出せないため
    /// 引き続き未接続とする。`progress.rs` モジュール doc のスコープ外事項 (b) を
    /// 参照）。`card` / `separator` エントリは `Card<S>` / `Separator<S>`（TASK-8.3-S60 /
    /// Issue #155）を `Screen::card` / `Screen::separator` 経由で登録する。`card` は
    /// `Container` カテゴリの 5 件目、`separator` は `Display` カテゴリの 7 件目の
    /// エントリ。`dialog` / `menu` / `tabs` / `tooltip` と同じ理由でカードのタップ・
    /// hover エレベーション配線は Issue #154 のスコープとして引き続き未接続とする。
    ///
    /// 組み込みエントリの `name` はいずれもリテラルで一意であるため、`register` が
    /// 重複エラーを返すことはない（`expect` は初期化不変条件のみに許容する
    /// coding-rust.md の方針に沿う）。
    pub fn with_builtins() -> Self {
        let mut catalog = Self::new();
        for entry in builtin_entries() {
            catalog
                .register(entry)
                .expect("組み込みエントリの name はリテラルで一意である不変条件");
        }
        catalog
    }
}

/// 組み込み 22 エントリ（Text / TextField / Button / List / RadioButton / ToggleSwitch /
/// Image / Icon / ScrollView / Stack / Grid / Dialog / Menu / Tabs / Tooltip / Label /
/// Checkbox / Slider / ProgressBar / Spinner / Card / Separator）を返す。
///
/// [`WidgetCatalog::with_builtins`] から利用される。各 preview は最小限の
/// `Screen<S>` を組み立てて [`render_headless`] した結果を返す。`radio_button`
/// （TASK-8.3-S24 / Issue #119）・`toggle_switch`（TASK-8.3-S27 / Issue #122）・
/// `image`（TASK-8.3-S36 / Issue #131）・`icon`（TASK-8.3-S39 / Issue #134）・
/// `scroll_view`（TASK-8.3-S42 / Issue #137）は各エントリの公開型を利用する。
/// `stack` / `grid` は `Stack<S>` / `Grid<S>`（TASK-8.3-S45 / Issue #140）を利用する
/// （[`WidgetCatalog::with_builtins`] doc 参照）。`dialog` は `Dialog<S>`
/// （TASK-8.3-S48 / Issue #143）を利用する。初の [`CatalogCategory::Overlay`]
/// エントリ。`menu` は `Menu<S>`（TASK-8.3-S51 / Issue #146）を利用する、`Overlay`
/// カテゴリの 2 件目のエントリ。`tabs` は `Tabs<S>`（TASK-8.3-S54 / Issue #149）を
/// 利用する、`Overlay` カテゴリの 3 件目のエントリ。`tooltip` は `Tooltip<S>`
/// （TASK-8.3-S57 / Issue #152）を利用する、`Overlay` カテゴリの 4 件目のエントリ。
/// `label` は `Label<S>`（TASK-8.3-S12 / Issue #107）を `Screen::label` 経由で
/// 利用する、`Display` カテゴリの 3 件目のエントリ。`checkbox` は `Checkbox<S>`
/// （TASK-8.3-S21 / Issue #116）を `Screen::checkbox` 経由で利用する、`Input`
/// カテゴリの 4 件目のエントリ。
/// `slider` は `Slider<S>`（TASK-8.3-S30 / Issue #125）を `Screen::slider` 経由で
/// 利用する、`Input` カテゴリの 5 件目のエントリ。状態・イベント処理（Issue #124）は
/// `checkbox` と同じ理由で preview 経路では未接続のまま（本番経路の `Screen::slider`
/// は配線済み、Issue #805 / #806）。`progress_bar` / `spinner` は
/// `ProgressBar<S>` / `Spinner<S>`（TASK-8.3-S33 / Issue #128）を
/// `Screen::progress_bar` / `Screen::spinner` 経由で利用する、`Display` カテゴリの
/// 5 件目・6 件目のエントリ（[`WidgetCatalog::with_builtins`] doc 参照）。
/// `button` エントリの接続先は `Widget<S>` 実装型 [`super::button::Button`]
/// （TASK-8.3-S9 / Issue #104。`preview_button` / [`button_preview_screen`] doc 参照）。
/// `name` はキーとして `button` のまま温存する（`DuplicateName` 制約下でエントリ数・
/// 件数固定テストへの影響を避ける判断、`button_widget.rs` モジュール doc 参照）。
/// `text_field` エントリの接続先は `Widget<S>` 実装型 [`super::text_input::TextInput`]
/// （TASK-8.3-S15 / Issue #110。`preview_text_field` / [`text_field_preview_screen`]
/// doc 参照、`button` エントリと同型の判断）。`name` はキーとして `text_field` の
/// まま温存する（`button` エントリと同じ理由）。
/// `list` エントリの接続先は `Widget<S>` 実装型 [`super::list_view::ListView`]
/// （TASK-8.3-S18 / Issue #113。`preview_list` / [`list_preview_screen`] doc 参照、
/// `button` エントリと同じ判断）。状態・イベント処理（Issue #112）は未接続のため、
/// `hit_test` は既定値（常に `false`）のまま接続する（`list_view.rs` モジュール doc
/// 参照）。`name` はキーとして `list` のまま温存する（`button` と同じ理由）。
/// `card` / `separator` は `Card<S>` / `Separator<S>`（TASK-8.3-S60 / Issue #155）を
/// 利用する。`card` は `Container` カテゴリの 5 件目、`separator` は `Display`
/// カテゴリの 7 件目のエントリ。新規エントリは末尾へ追記する（既存テストの件数固定を
/// 崩さないため）。
fn builtin_entries() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            name: "text",
            category: CatalogCategory::Display,
            description: "静的テキストを表示するウィジェット",
            preview: preview_text,
        },
        CatalogEntry {
            name: "text_field",
            category: CatalogCategory::Input,
            description: "ラベル付きテキスト入力欄",
            preview: preview_text_field,
        },
        CatalogEntry {
            name: "button",
            category: CatalogCategory::Action,
            description: "押下時に状態を更新するボタン",
            preview: preview_button,
        },
        CatalogEntry {
            name: "list",
            category: CatalogCategory::Container,
            description: "状態から子要素を都度展開するリスト",
            preview: preview_list,
        },
        CatalogEntry {
            name: "radio_button",
            category: CatalogCategory::Input,
            description: "グループ内排他選択を行うラジオボタン",
            preview: preview_radio_button,
        },
        CatalogEntry {
            name: "toggle_switch",
            category: CatalogCategory::Input,
            description: "on/off を切り替えるトグルスイッチ",
            preview: preview_toggle_switch,
        },
        CatalogEntry {
            name: "image",
            category: CatalogCategory::Display,
            description: "代替テキスト付きの画像プレースホルダを表示するウィジェット",
            preview: preview_image,
        },
        CatalogEntry {
            name: "icon",
            category: CatalogCategory::Display,
            description: "グリフ名とアクセシビリティラベルでアイコンを表示するウィジェット",
            preview: preview_icon,
        },
        CatalogEntry {
            name: "scroll_view",
            category: CatalogCategory::Container,
            description: "コンテンツをビューポート内で縦スクロール表示するコンテナ",
            preview: preview_scroll_view,
        },
        CatalogEntry {
            name: "stack",
            category: CatalogCategory::Container,
            description: "子を縦・横方向に等間隔で積むレイアウトコンテナ",
            preview: preview_stack,
        },
        CatalogEntry {
            name: "grid",
            category: CatalogCategory::Container,
            description: "固定列数の行優先で子を並べるレイアウトコンテナ",
            preview: preview_grid,
        },
        CatalogEntry {
            name: "dialog",
            category: CatalogCategory::Overlay,
            description: "開閉状態をアプリ状態に置くモーダル / 非モーダルの確認ダイアログ",
            preview: preview_dialog,
        },
        CatalogEntry {
            name: "menu",
            category: CatalogCategory::Overlay,
            description: "トリガー押下で開閉するメニュー・ドロップダウン",
            preview: preview_menu,
        },
        CatalogEntry {
            name: "tabs",
            category: CatalogCategory::Overlay,
            description: "選択中タブ index をアプリ状態に置くタブ切替コンテナ",
            preview: preview_tabs,
        },
        CatalogEntry {
            name: "tooltip",
            category: CatalogCategory::Overlay,
            description: "アンカーに付随する本文バブルを明示的に表示・非表示するツールチップ",
            preview: preview_tooltip,
        },
        CatalogEntry {
            name: "label",
            category: CatalogCategory::Display,
            description: "状態束縛テキストを表示するラベル",
            preview: preview_label,
        },
        CatalogEntry {
            name: "checkbox",
            category: CatalogCategory::Input,
            description: "チェック有無をアプリ状態に束縛するチェックボックス",
            preview: preview_checkbox,
        },
        CatalogEntry {
            name: "slider",
            category: CatalogCategory::Input,
            description: "範囲内の数値をアプリ状態に束縛するスライダー",
            preview: preview_slider,
        },
        CatalogEntry {
            name: "progress_bar",
            category: CatalogCategory::Display,
            description: "進捗値をバーとパーセント表記で表示するウィジェット",
            preview: preview_progress_bar,
        },
        CatalogEntry {
            name: "spinner",
            category: CatalogCategory::Display,
            description: "進捗値が不明な処理中表示を行うスピナー",
            preview: preview_spinner,
        },
        CatalogEntry {
            name: "card",
            category: CatalogCategory::Container,
            description: "複数子を余白付きで縦に積むカードコンテナ",
            preview: preview_card,
        },
        CatalogEntry {
            name: "separator",
            category: CatalogCategory::Display,
            description: "水平・垂直の区切り線セパレータ",
            preview: preview_separator,
        },
    ]
}

/// `text` エントリの preview 用ダミー状態（プレビューは状態遷移を伴わないため空）。
#[derive(Default)]
struct PreviewState;

fn preview_text() -> Vec<String> {
    let screen: Screen<PreviewState> = Screen::new("Text プレビュー").text("見出しテキスト");
    render_headless(&screen, &PreviewState)
}

fn preview_text_field_get(_state: &PreviewState) -> String {
    "サンプル値".to_string()
}

fn preview_text_field_set(_state: &mut PreviewState, _value: String) {}

/// `text_field` エントリの preview 画面を組み立てる（TASK-8.3-S15 / Issue #110）。
///
/// `Widget<S>` 実装型 [`super::text_input::TextInput`]（Issue #108/#109）を
/// `Node::Widget` 経由で 1 件だけ配置したデモ画面を返す（[`button_preview_screen`] と同じ
/// 様式、レガシー `Node::TextField`（`Screen::text_field`）からの差し替え）。
/// [`preview_text_field`]（`render_headless` 経由）と、`#[cfg(test)] mod tests` 側で
/// `Widget<S>` トレイトメソッド（`accessibility_role`）越しに実配線を検証する用途の双方から
/// 参照する（`text_field_preview_widget_is_backed_by_real_text_input` 参照）。表示文言は
/// preview 専用の固定リテラルであり外部入力ではない（A03 対策）。
fn text_field_preview_screen() -> Screen<PreviewState> {
    Screen::new("TextField プレビュー").with_node(Node::Widget(Box::new(TextInput::bound(
        WidgetId::from_raw(1),
        "ラベル",
        preview_text_field_get,
        preview_text_field_set,
    ))))
}

/// `text_field` エントリの preview（TASK-8.3-S15 / Issue #110）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `button` 等の他エントリと同様にタイトル行のみを返す（実描画・reconcile 配線は既存契約
/// どおり引き続きスコープ外）。
fn preview_text_field() -> Vec<String> {
    render_headless(&text_field_preview_screen(), &PreviewState)
}

/// `button` エントリの preview 用押下ハンドラ（TASK-8.3-S9 / Issue #104）。
///
/// プレビューは状態遷移を伴わないため（[`preview_button_on_click`] 改め、他エントリの
/// no-op ハンドラ群と同じ判断）、no-op とする。環境キャプチャ不可の関数ポインタに
/// 限定する（REQ-13、`Button::new` の `on_click` 契約）。
fn preview_button_on_click(_state: &mut PreviewState) {}

/// `button` エントリの preview 画面を組み立てる（TASK-8.3-S9 / Issue #104）。
///
/// `Widget<S>` 実装型 [`super::button::Button`]（Issue #102/#103）を
/// `Node::Widget` 経由で 1 件だけ配置したデモ画面を返す（`stack_preview_screen` /
/// `tabs_preview_screen` 等と同じ様式、レガシー `Node::Button`（`Screen::button`）からの
/// 差し替え）。[`preview_button`]（`render_headless` 経由）と、`#[cfg(test)] mod tests`
/// 側で `Widget<S>` トレイトメソッド（`accessibility_role`）越しに実配線を検証する用途の
/// 双方から参照する（`button_preview_widget_is_backed_by_real_button` 参照）。表示文言は
/// preview 専用の固定リテラルであり外部入力ではない（A03 対策）。
fn button_preview_screen() -> Screen<PreviewState> {
    Screen::new("Button プレビュー").with_node(Node::Widget(Box::new(Button::new(
        WidgetId::from_raw(1),
        "サンプルボタン",
        preview_button_on_click,
    ))))
}

/// `button` エントリの preview（TASK-8.3-S9 / Issue #104）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `stack` / `tabs` 等の他エントリと同様にタイトル行のみを返す（実描画・reconcile 配線は
/// 既存契約どおり引き続きスコープ外）。
fn preview_button() -> Vec<String> {
    render_headless(&button_preview_screen(), &PreviewState)
}

/// `list` エントリの preview 画面を組み立てる（TASK-8.3-S18 / Issue #113）。
///
/// `Widget<S>` 実装型 [`super::list_view::ListView`]（Issue #111）を
/// `Node::Widget` 経由で 1 件だけ配置した
/// デモ画面を返す（[`button_preview_screen`] と同じ様式、レガシー `Node::List`
/// （`Screen::list` ビルダー）からの差し替え）。[`preview_list`]（`render_headless` 経由）
/// と、`#[cfg(test)] mod tests` 側で `Widget<S>` トレイトメソッド
/// （`accessibility_role` / `measure`）越しに実配線を検証する用途の双方から参照する
/// （`list_preview_widget_is_backed_by_real_list_view` 参照）。表示項目は preview 専用の
/// 固定リテラルであり外部入力ではない（A03 対策）。`ListView::fixed` を用いるため
/// `sync_from_state` を経ずとも構築直後から項目が反映される
/// （`list_view.rs` の `ListItemSource::Static` doc 参照）。
fn list_preview_screen() -> Screen<PreviewState> {
    Screen::new("List プレビュー").with_node(Node::Widget(Box::new(ListView::fixed(
        WidgetId::from_raw(1),
        vec!["項目 1".to_string(), "項目 2".to_string()],
    ))))
}

/// `list` エントリの preview（TASK-8.3-S18 / Issue #113）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `button` 等の他エントリと同様にタイトル行のみを返す（実描画・reconcile 配線は
/// 既存契約どおり引き続きスコープ外）。
fn preview_list() -> Vec<String> {
    render_headless(&list_preview_screen(), &PreviewState)
}

/// `radio_button` エントリ用の get/set（TASK-8.3-S24 / Issue #119）。
///
/// カタログのプレビューは状態遷移を伴わないため（`preview_text_field_*` と同じ判断）、
/// 常に選択肢 0 を返す・書き込みは no-op とする。
fn preview_radio_button_get(_state: &PreviewState) -> usize {
    0
}

fn preview_radio_button_set(_state: &mut PreviewState, _value: usize) {}

/// `radio_button` エントリの preview（TASK-8.3-S24 / Issue #117 の `RadioButton<S>` を
/// 1 件だけ配置したデモ画面）。`render_headless` は `Node::Widget` を未配線のまま
/// プレースホルダ扱いする契約（`widget.rs` 参照）のため、タイトル行のみを返す。
fn preview_radio_button() -> Vec<String> {
    let screen: Screen<PreviewState> = Screen::new("RadioButton プレビュー").radio_button(
        WidgetId::from_raw(1),
        "選択肢A",
        0,
        preview_radio_button_get,
        preview_radio_button_set,
    );
    render_headless(&screen, &PreviewState)
}

/// `toggle_switch` エントリ用の get/set（TASK-8.3-S27 / Issue #122）。
///
/// カタログのプレビューは状態遷移を伴わないため（`preview_radio_button_*` と同じ判断）、
/// 常に off（`false`）を返す・書き込みは no-op とする。
fn preview_toggle_switch_get(_state: &PreviewState) -> bool {
    false
}

fn preview_toggle_switch_set(_state: &mut PreviewState, _value: bool) {}

/// `toggle_switch` エントリの preview（TASK-8.3-S27 / Issue #122。`ToggleSwitch<S>`
/// （Issue #120）を 1 件だけ配置したデモ画面）。`render_headless` は `Node::Widget` を
/// 未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、タイトル行のみを返す。
fn preview_toggle_switch() -> Vec<String> {
    let screen: Screen<PreviewState> = Screen::new("ToggleSwitch プレビュー").toggle_switch(
        WidgetId::from_raw(1),
        "通知",
        preview_toggle_switch_get,
        preview_toggle_switch_set,
    );
    render_headless(&screen, &PreviewState)
}

/// `image` エントリの preview（TASK-8.3-S36 / Issue #131。`Image<S>`（Issue #129）を
/// 1 件だけ配置したデモ画面）。`render_headless` は `Node::Widget` を
/// 未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、タイトル行のみを
/// 返す。ソース識別子はカタログの静的プレビュー用の固定値であり外部入力ではない。
fn preview_image() -> Vec<String> {
    let screen: Screen<PreviewState> = Screen::new("Image プレビュー").image(
        WidgetId::from_raw(1),
        "assets/preview.png",
        "プレビュー画像",
    );
    render_headless(&screen, &PreviewState)
}

/// `icon` エントリの preview（TASK-8.3-S39 / Issue #134。自己完結実装した `Icon<S>`
/// （`icon.rs` 参照）を 1 件だけ配置したデモ画面）。`render_headless` は `Node::Widget`
/// を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、タイトル行のみを
/// 返す。アイコン名は preview 専用の固定リテラルであり外部入力ではない。
fn preview_icon() -> Vec<String> {
    let screen: Screen<PreviewState> =
        Screen::new("Icon プレビュー").icon(WidgetId::from_raw(1), "settings", "設定アイコン");
    render_headless(&screen, &PreviewState)
}

/// `scroll_view` エントリの preview（TASK-8.3-S42 / Issue #137。S40（Issue #135）の
/// コンテナ型 `ScrollView<S>`（`scroll_view.rs` 参照）へ、子コンテンツとして
/// `Label::fixed`（S10 / #105）を 1 件だけ配置したデモ画面）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// タイトル行のみを返す。ラベル文字列は preview 専用の固定リテラルであり外部入力ではない。
fn preview_scroll_view() -> Vec<String> {
    let screen: Screen<PreviewState> = Screen::new("ScrollView プレビュー").scroll_view(
        WidgetId::from_raw(1),
        Label::fixed(WidgetId::from_raw(2), "フィード一覧"),
    );
    render_headless(&screen, &PreviewState)
}

/// `stack` エントリの preview 画面を組み立てる（TASK-8.3-S45 / Issue #140）。
///
/// `Stack<S>`（`stack_grid.rs` 参照、Issue #138）へ、
/// 子コンテンツとして `Label::fixed`（S10 / #105）を 2 件、縦方向（既定の
/// [`super::layout::FlexDirection::Column`]）に並べて配置したデモ画面を返す。
/// [`preview_stack`]（`render_headless` 経由）と、テスト側で `Widget<S>` トレイト
/// メソッド（`accessibility_role` / `measure`）越しに実配線を検証する用途の双方から
/// 参照する（`stack_preview_widget_is_backed_by_real_stack_with_two_children` 参照）。
/// 表示文字列は preview 専用の固定リテラルであり外部入力ではない（A03 対策）。
fn stack_preview_screen() -> Screen<PreviewState> {
    let stack = Stack::new(WidgetId::from_raw(1))
        .with_child(Label::fixed(WidgetId::from_raw(2), "項目 1"))
        .with_child(Label::fixed(WidgetId::from_raw(3), "項目 2"));
    Screen::new("Stack プレビュー").stack(stack)
}

/// `stack` エントリの preview（TASK-8.3-S45 / Issue #140）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `scroll_view` エントリと同様にタイトル行のみを返す（子ラベルの実描画は `Widget<S>`
/// の reconcile 配線後のスコープ）。
fn preview_stack() -> Vec<String> {
    render_headless(&stack_preview_screen(), &PreviewState)
}

/// `grid` エントリの preview 画面を組み立てる（[`stack_preview_screen`] と対）。
/// 固定 2 列の行優先配置で `Label::fixed` を 2 件配置したデモ画面を返す。
fn grid_preview_screen() -> Screen<PreviewState> {
    let grid = Grid::new(WidgetId::from_raw(1))
        .with_columns(2)
        .with_child(Label::fixed(WidgetId::from_raw(2), "セル (0, 0)"))
        .with_child(Label::fixed(WidgetId::from_raw(3), "セル (0, 1)"));
    Screen::new("Grid プレビュー").grid(grid)
}

/// `grid` エントリの preview（TASK-8.3-S45 / Issue #140）。出力契約は
/// [`preview_stack`] と同じ（タイトル行のみ）。
fn preview_grid() -> Vec<String> {
    render_headless(&grid_preview_screen(), &PreviewState)
}

/// `dialog` エントリの preview 用開閉状態読み取り関数（TASK-8.3-S48 / Issue #143）。
///
/// プレビューは状態遷移を伴わないため（[`preview_radio_button_get`] 等と同じ判断）、
/// 常に開状態（`true`）を固定で返す。環境キャプチャ不可の関数ポインタに限定する
/// （REQ-13、[`crate::state::ReadFn`] 契約）。
fn preview_dialog_open(_state: &PreviewState) -> bool {
    true
}

/// `dialog` エントリの preview 画面を組み立てる（TASK-8.3-S48 / Issue #143）。
///
/// `Dialog<S>`（`dialog.rs` 参照、Issue #141）を
/// `Screen::dialog` 経由で 1 件だけ配置したデモ画面を返す。[`preview_dialog`]
/// （`render_headless` 経由）と、`#[cfg(test)] mod tests` 側で `Widget<S>` トレイト
/// メソッド（`accessibility_role`）越しに実配線を検証する用途の双方から参照する
/// （`dialog_preview_widget_is_backed_by_real_dialog` 参照）。表示文言は preview 専用の
/// 固定リテラルであり外部入力ではない（A03 対策）。
fn dialog_preview_screen() -> Screen<PreviewState> {
    Screen::new("Dialog プレビュー").dialog(
        WidgetId::from_raw(1),
        "確認",
        "この操作を実行しますか？",
        preview_dialog_open,
    )
}

/// `dialog` エントリの preview（TASK-8.3-S48 / Issue #143）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `stack` / `grid` エントリと同様にタイトル行のみを返す（`Dialog<S>` は
/// `sync_from_state` が呼ばれるまで既定で閉状態、`dialog.rs` 参照）。
fn preview_dialog() -> Vec<String> {
    render_headless(&dialog_preview_screen(), &PreviewState)
}

/// `menu` エントリの preview 用開閉状態・選択中項目 index 読み取り関数
/// （TASK-8.3-S51 / Issue #146）。
///
/// プレビューは状態遷移を伴わないため（[`preview_dialog_open`] と同じ判断）、
/// 常に開状態（`true`）・未選択（`None`）を固定で返す。環境キャプチャ不可の
/// 関数ポインタに限定する（REQ-13、[`crate::state::ReadFn`] 契約）。
fn preview_menu_open(_state: &PreviewState) -> bool {
    true
}

fn preview_menu_selected(_state: &PreviewState) -> Option<usize> {
    None
}

/// `menu` エントリの preview 画面を組み立てる（TASK-8.3-S51 / Issue #146）。
///
/// `Menu<S>`（`menu.rs` 参照、Issue #144）を `Screen::menu`
/// 経由で 1 件だけ配置したデモ画面を返す。[`preview_menu`]（`render_headless` 経由）と、
/// `#[cfg(test)] mod tests` 側で `Widget<S>` トレイトメソッド（`accessibility_role`）
/// 越しに実配線を検証する用途の双方から参照する
/// （`menu_preview_widget_is_backed_by_real_menu` 参照）。表示文言は preview 専用の
/// 固定リテラルであり外部入力ではない（A03 対策）。
fn menu_preview_screen() -> Screen<PreviewState> {
    Screen::new("Menu プレビュー").menu(
        WidgetId::from_raw(1),
        "選択してください",
        vec!["項目 A".to_string(), "項目 B".to_string()],
        preview_menu_open,
        preview_menu_selected,
    )
}

/// `menu` エントリの preview（TASK-8.3-S51 / Issue #146）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `dialog` エントリと同様にタイトル行のみを返す（`Menu<S>` は `sync_from_state` が
/// 呼ばれるまで既定で閉状態・未選択、`menu.rs` 参照）。
fn preview_menu() -> Vec<String> {
    render_headless(&menu_preview_screen(), &PreviewState)
}

/// `tabs` エントリの preview 用選択中タブ index 読み取り関数（TASK-8.3-S54 / Issue #149）。
///
/// プレビューは状態遷移を伴わないため（[`preview_menu_selected`] 等と同じ判断）、
/// 常に先頭タブ（`0`）を固定で返す。環境キャプチャ不可の関数ポインタに限定する
/// （REQ-13、[`crate::state::ReadFn`] 契約）。
fn preview_tabs_selected(_state: &PreviewState) -> usize {
    0
}

/// `tabs` エントリの preview 画面を組み立てる（TASK-8.3-S54 / Issue #149）。
///
/// `Tabs<S>`（`tabs.rs` 参照、Issue #147）を
/// `Screen::tabs` 経由で 1 件だけ配置したデモ画面を返す。[`preview_tabs`]
/// （`render_headless` 経由）と、`#[cfg(test)] mod tests` 側で `Widget<S>` トレイト
/// メソッド（`accessibility_role`）越しに実配線を検証する用途の双方から参照する
/// （`tabs_preview_widget_is_backed_by_real_tabs` 参照）。表示文言は preview 専用の
/// 固定リテラルであり外部入力ではない（A03 対策）。
fn tabs_preview_screen() -> Screen<PreviewState> {
    Screen::new("Tabs プレビュー").tabs(
        WidgetId::from_raw(1),
        vec!["タブ A".to_string(), "タブ B".to_string()],
        preview_tabs_selected,
    )
}

/// `tabs` エントリの preview（TASK-8.3-S54 / Issue #149）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `menu` エントリと同様にタイトル行のみを返す（`Tabs<S>` は `sync_from_state` が
/// 呼ばれるまで既定で `cached_selected == None`、`tabs.rs` 参照）。
fn preview_tabs() -> Vec<String> {
    render_headless(&tabs_preview_screen(), &PreviewState)
}

/// `tooltip` エントリの preview 画面を組み立てる（TASK-8.3-S57 / Issue #152）。
///
/// `Tooltip<S>`（`tooltip.rs` 参照、Issue #150）を
/// `Screen::tooltip` 経由で、アンカーとして `Label::fixed`（S10 / #105）を 1 件だけ
/// 配置したデモ画面を返す。[`preview_tooltip`]（`render_headless` 経由）と、
/// `#[cfg(test)] mod tests` 側で `Widget<S>` トレイトメソッド（`accessibility_role`）
/// 越しに実配線を検証する用途の双方から参照する
/// （`tooltip_preview_widget_is_backed_by_real_tooltip` 参照）。表示文言は preview 専用の
/// 固定リテラルであり外部入力ではない（A03 対策）。
fn tooltip_preview_screen() -> Screen<PreviewState> {
    Screen::new("Tooltip プレビュー").tooltip(
        WidgetId::from_raw(1),
        "保存します",
        Label::fixed(WidgetId::from_raw(2), "保存"),
    )
}

/// `tooltip` エントリの preview（TASK-8.3-S57 / Issue #152）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `tabs` エントリと同様にタイトル行のみを返す（`Tooltip<S>` は構築直後は既定で
/// 非表示状態、`tooltip.rs` 参照）。
fn preview_tooltip() -> Vec<String> {
    render_headless(&tooltip_preview_screen(), &PreviewState)
}

/// `label` エントリの preview 用テキスト読み取り関数（TASK-8.3-S12 / Issue #107）。
///
/// プレビューは状態遷移を伴わないため（`preview_dialog_open` 等と同じ判断）、
/// 常に固定文言を返す。環境キャプチャ不可の関数ポインタに限定する
/// （REQ-13、[`crate::state::ReadFn`] 契約）。
fn preview_label_get(_state: &PreviewState) -> String {
    "サンプルラベル".to_string()
}

/// `label` エントリの preview（TASK-8.3-S12 / Issue #107。
/// `Label<S>`（`label.rs` 参照、Issue #105）を `Screen::label` 経由で 1 件だけ配置したデモ画面）。
/// `render_headless` は `Node::Widget` を未配線のままプレースホルダ扱いする契約
/// （`widget.rs` 参照）のため、他の `Widget<S>` 系エントリと同様にタイトル行のみを返す
/// （`Label<S>` は `sync_from_state` が呼ばれるまで `Bound` テキストは空文字列のまま、
/// `label.rs` 参照）。
fn preview_label() -> Vec<String> {
    let screen: Screen<PreviewState> =
        Screen::new("Label プレビュー").label(WidgetId::from_raw(1), preview_label_get);
    render_headless(&screen, &PreviewState)
}

/// `checkbox` エントリ用の get/set（TASK-8.3-S21 / Issue #116）。
///
/// カタログのプレビューは状態遷移を伴わないため（`preview_toggle_switch_*` と同じ判断）、
/// 常に未チェック（`false`）を返す・書き込みは no-op とする。
fn preview_checkbox_get(_state: &PreviewState) -> bool {
    false
}

fn preview_checkbox_set(_state: &mut PreviewState, _value: bool) {}

/// `checkbox` エントリの preview 画面を組み立てる（TASK-8.3-S21 / Issue #116）。
///
/// `Checkbox<S>`（`checkbox.rs` 参照、Issue #114）を
/// `Screen::checkbox` 経由で 1 件だけ配置したデモ画面を返す。[`preview_checkbox`]
/// （`render_headless` 経由）と、`#[cfg(test)] mod tests` 側で `Widget<S>` トレイト
/// メソッド（`accessibility_role` / `measure`）越しに実配線を検証する用途の双方から
/// 参照する（`checkbox_preview_widget_is_backed_by_real_checkbox` 参照）。表示文言は
/// preview 専用の固定リテラルであり外部入力ではない（A03 対策）。
fn checkbox_preview_screen() -> Screen<PreviewState> {
    Screen::new("Checkbox プレビュー").checkbox(
        WidgetId::from_raw(1),
        "同意する",
        preview_checkbox_get,
        preview_checkbox_set,
    )
}

/// `checkbox` エントリの preview（TASK-8.3-S21 / Issue #116）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `label` エントリと同様にタイトル行のみを返す（`Checkbox<S>` は `sync_from_state` が
/// 呼ばれるまで既定で未チェック、`checkbox.rs` 参照）。
fn preview_checkbox() -> Vec<String> {
    render_headless(&checkbox_preview_screen(), &PreviewState)
}

/// `slider` エントリ用の get/set（TASK-8.3-S30 / Issue #125）。
///
/// カタログのプレビューは状態遷移を伴わないため（`preview_checkbox_*` と同じ判断）、
/// 常に固定値（`0.5`）を返す・書き込みは no-op とする。
fn preview_slider_get(_state: &PreviewState) -> f32 {
    0.5
}

fn preview_slider_set(_state: &mut PreviewState, _value: f32) {}

/// `slider` エントリの preview 画面を組み立てる（TASK-8.3-S30 / Issue #125）。
///
/// `Slider<S>`（`slider.rs` 参照、Issue #123）を
/// `Screen::slider` 経由で 1 件だけ配置したデモ画面を返す。[`preview_slider`]
/// （`render_headless` 経由）と、`#[cfg(test)] mod tests` 側で `Widget<S>` トレイト
/// メソッド（`accessibility_role` / `measure`）越しに実配線を検証する用途の双方から
/// 参照する（`slider_preview_widget_is_backed_by_real_slider` 参照）。表示文言は
/// preview 専用の固定リテラルであり外部入力ではない（A03 対策）。
fn slider_preview_screen() -> Screen<PreviewState> {
    Screen::new("Slider プレビュー").slider(
        WidgetId::from_raw(1),
        "音量",
        0.0,
        1.0,
        preview_slider_get,
        preview_slider_set,
    )
}

/// `slider` エントリの preview（TASK-8.3-S30 / Issue #125）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `checkbox` エントリと同様にタイトル行のみを返す（`Slider<S>` は `sync_from_state` が
/// 呼ばれるまで既定で `min` 値のまま、`slider.rs` 参照）。
fn preview_slider() -> Vec<String> {
    render_headless(&slider_preview_screen(), &PreviewState)
}

/// `progress_bar` エントリ用の進捗値読み取り関数（TASK-8.3-S33 / Issue #128）。
///
/// カタログのプレビューは状態遷移を伴わないため（`preview_toggle_switch_*` と
/// 同じ判断）、常に固定値（42%）を返す。環境キャプチャ不可の関数ポインタに
/// 限定する（REQ-13、[`crate::state::ReadFn`] 契約）。
fn preview_progress_get(_state: &PreviewState) -> f32 {
    0.42
}

/// `progress_bar` エントリの preview 画面を組み立てる（TASK-8.3-S33 / Issue #128）。
///
/// `Screen::progress_bar` 経由で `ProgressBar<S>`（`progress.rs` 参照、Issue #126）を
/// 1 件だけ配置したデモ画面を返す。[`preview_progress_bar`]（`render_headless` 経由）
/// から参照する。表示文言は preview 専用の固定リテラルであり外部入力ではない
/// （A03 対策）。
fn progress_bar_preview_screen() -> Screen<PreviewState> {
    Screen::new("ProgressBar プレビュー").progress_bar(
        WidgetId::from_raw(1),
        "ダウンロード",
        preview_progress_get,
    )
}

/// `progress_bar` エントリの preview（TASK-8.3-S33 / Issue #128）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `checkbox` エントリと同様にタイトル行のみを返す（`ProgressBar<S>` は
/// `sync_from_state` が呼ばれるまで既定で `cached_fraction == 0.0`、`progress.rs` 参照）。
fn preview_progress_bar() -> Vec<String> {
    render_headless(&progress_bar_preview_screen(), &PreviewState)
}

/// `spinner` エントリの preview 画面を組み立てる（TASK-8.3-S33 / Issue #128）。
///
/// `Screen::spinner` 経由で `Spinner<S>`（Issue #126）
/// （`progress.rs` 参照、状態バインディングを持たない indeterminate 表示）を
/// 1 件だけ配置したデモ画面を返す。[`preview_spinner`]（`render_headless` 経由）から
/// 参照する。
fn spinner_preview_screen() -> Screen<PreviewState> {
    Screen::new("Spinner プレビュー").spinner(WidgetId::from_raw(1))
}

/// `spinner` エントリの preview（TASK-8.3-S33 / Issue #128）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `progress_bar` エントリと同様にタイトル行のみを返す。
fn preview_spinner() -> Vec<String> {
    render_headless(&spinner_preview_screen(), &PreviewState)
}

/// `card` エントリの preview 画面を組み立てる（TASK-8.3-S60 / Issue #155）。
///
/// `Card<S>`（`card.rs` 参照、Issue #153）へ、
/// 子コンテンツとして `Label::fixed`（S10 / #105）を 2 件、水平の `Separator`
/// （`SeparatorOrientation::Horizontal`、既定）を挟んで配置したデモ画面を返す。
/// [`preview_card`]（`render_headless` 経由）と、`#[cfg(test)] mod tests` 側で
/// `Widget<S>` トレイトメソッド（`accessibility_role` / `measure`）越しに実配線を
/// 検証する用途の双方から参照する（`card_preview_widget_is_backed_by_real_card`
/// 参照）。表示文言は preview 専用の固定リテラルであり外部入力ではない（A03 対策）。
fn card_preview_screen() -> Screen<PreviewState> {
    let card = Card::new(WidgetId::from_raw(1))
        .with_padding(8.0)
        .with_child(Label::fixed(WidgetId::from_raw(2), "タイトル"))
        .with_child(Separator::new(WidgetId::from_raw(3)))
        .with_child(Label::fixed(WidgetId::from_raw(4), "本文"));
    Screen::new("Card プレビュー").card(card)
}

/// `card` エントリの preview（TASK-8.3-S60 / Issue #155）。`render_headless` は
/// `Node::Widget` を未配線のままプレースホルダ扱いする契約（`widget.rs` 参照）のため、
/// `tooltip` エントリと同様にタイトル行のみを返す（子ラベル・セパレータの実描画は
/// `Widget<S>` の reconcile 配線後のスコープ）。
fn preview_card() -> Vec<String> {
    render_headless(&card_preview_screen(), &PreviewState)
}

/// `separator` エントリの preview 画面を組み立てる（[`card_preview_screen`] と対、
/// TASK-8.3-S60 / Issue #155）。`Card<S>`（Issue #153）が公開面を定めた
/// `Separator<S>` を、水平・垂直の 2 種類配置したデモ画面を返す。
fn separator_preview_screen() -> Screen<PreviewState> {
    Screen::new("Separator プレビュー")
        .separator(Separator::new(WidgetId::from_raw(1)))
        .separator(
            Separator::new(WidgetId::from_raw(2)).with_orientation(SeparatorOrientation::Vertical),
        )
}

/// `separator` エントリの preview（TASK-8.3-S60 / Issue #155）。出力契約は
/// [`preview_card`] と同じ（タイトル行のみ）。
fn preview_separator() -> Vec<String> {
    render_headless(&separator_preview_screen(), &PreviewState)
}

/// サンプルギャラリー画面が保持する状態。
///
/// `Node::List::expand` は `fn(&S) -> Vec<Node<S>>` の関数ポインタ限定でクロージャ
/// 捕捉不可のため（node.rs 参照）、カタログから導出したエントリ表示用データ
/// （name / category / description の文字列列）を state 経由で渡す
/// （`sample.rs` の `SampleState` と同じ様式）。
#[derive(Debug, Clone, Default)]
pub struct GalleryState {
    /// 一覧表示用に整形済みのエントリ行（`{name}（{category}）: {description}`）。
    pub entry_lines: Vec<String>,
}

/// [`CatalogCategory`] を一覧表示用の日本語ラベルへ変換する。
fn category_label(category: CatalogCategory) -> &'static str {
    match category {
        CatalogCategory::Action => "アクション",
        CatalogCategory::Input => "入力",
        CatalogCategory::Display => "表示",
        CatalogCategory::Container => "コンテナ",
        CatalogCategory::Overlay => "オーバーレイ",
    }
}

/// [`GalleryState`] からリスト表示用の子ノード一覧を生成する（`Node::List::expand`）。
fn expand_gallery_entries(state: &GalleryState) -> Vec<Node<GalleryState>> {
    state.entry_lines.iter().cloned().map(Node::Text).collect()
}

impl WidgetCatalog {
    /// カタログの登録内容からギャラリー画面用の状態を導出する。
    ///
    /// 登録順を保ったまま `{name}（{category}）: {description}` の 1 行へ整形する。
    pub fn gallery_state(&self) -> GalleryState {
        let entry_lines = self
            .entries
            .iter()
            .map(|entry| {
                format!(
                    "{}（{}）: {}",
                    entry.name,
                    category_label(entry.category),
                    entry.description
                )
            })
            .collect();
        GalleryState { entry_lines }
    }

    /// カタログの登録内容から「タイトル + エントリ一覧」のギャラリー画面を組み立てる。
    ///
    /// 既存ビルダー（`.text()` / `.list()`）のみで構成し、`render_headless` で
    /// 一覧を確認できるようにする（`Widget<S>` トレイトへは依存しない自己完結設計）。
    pub fn gallery_screen(&self) -> Screen<GalleryState> {
        Screen::new("ウィジェットカタログ")
            .text("登録済みウィジェット一覧")
            .list(expand_gallery_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(name: &'static str) -> CatalogEntry {
        CatalogEntry {
            name,
            category: CatalogCategory::Display,
            description: "テスト用エントリ",
            preview: || vec!["dummy".to_string()],
        }
    }

    #[test]
    fn register_succeeds_for_new_name() {
        let mut catalog = WidgetCatalog::new();
        assert!(catalog.register(sample_entry("widget-a")).is_ok());
        assert_eq!(catalog.entries().len(), 1);
    }

    #[test]
    fn register_rejects_duplicate_name() {
        let mut catalog = WidgetCatalog::new();
        catalog.register(sample_entry("widget-a")).unwrap();

        let err = catalog.register(sample_entry("widget-a")).unwrap_err();
        assert_eq!(err, CatalogError::DuplicateName("widget-a"));
        // 重複登録エラー時にエントリ数が増えていないことを確認する。
        assert_eq!(catalog.entries().len(), 1);
    }

    #[test]
    fn entries_preserve_registration_order() {
        let mut catalog = WidgetCatalog::new();
        catalog.register(sample_entry("widget-b")).unwrap();
        catalog.register(sample_entry("widget-a")).unwrap();

        let names: Vec<&str> = catalog.entries().iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["widget-b", "widget-a"]);
    }

    #[test]
    fn find_returns_matching_entry() {
        let mut catalog = WidgetCatalog::new();
        catalog.register(sample_entry("widget-a")).unwrap();

        assert!(catalog.find("widget-a").is_some());
        assert!(catalog.find("widget-missing").is_none());
    }

    #[test]
    fn entries_in_filters_by_category() {
        let mut catalog = WidgetCatalog::new();
        catalog
            .register(CatalogEntry {
                category: CatalogCategory::Action,
                ..sample_entry("button-like")
            })
            .unwrap();
        catalog
            .register(CatalogEntry {
                category: CatalogCategory::Display,
                ..sample_entry("text-like")
            })
            .unwrap();

        let display_entries = catalog.entries_in(CatalogCategory::Display);
        assert_eq!(display_entries.len(), 1);
        assert_eq!(display_entries[0].name, "text-like");

        let action_entries = catalog.entries_in(CatalogCategory::Action);
        assert_eq!(action_entries.len(), 1);
        assert_eq!(action_entries[0].name, "button-like");
    }

    #[test]
    fn with_builtins_registers_twenty_two_entries_without_duplicate_error() {
        let catalog = WidgetCatalog::with_builtins();
        assert_eq!(catalog.entries().len(), 22);

        let names: Vec<&str> = catalog.entries().iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            vec![
                "text",
                "text_field",
                "button",
                "list",
                "radio_button",
                "toggle_switch",
                "image",
                "icon",
                "scroll_view",
                "stack",
                "grid",
                "dialog",
                "menu",
                "tabs",
                "tooltip",
                "label",
                "checkbox",
                "slider",
                "progress_bar",
                "spinner",
                "card",
                "separator"
            ]
        );
    }

    /// `radio_button` エントリが `Input` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S24 / Issue #119、カタログ登録の受け入れ条件）。
    #[test]
    fn with_builtins_registers_radio_button_in_input_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("radio_button")
            .expect("radio_button エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Input);

        let input_entries = catalog.entries_in(CatalogCategory::Input);
        assert!(input_entries.iter().any(|e| e.name == "radio_button"));
    }

    /// `toggle_switch` エントリが `Input` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S27 / Issue #122、カタログ登録の受け入れ条件）。
    #[test]
    fn with_builtins_registers_toggle_switch_in_input_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("toggle_switch")
            .expect("toggle_switch エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Input);

        let input_entries = catalog.entries_in(CatalogCategory::Input);
        assert!(input_entries.iter().any(|e| e.name == "toggle_switch"));
    }

    /// `image` エントリが `Display` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S36 / Issue #131、カタログ登録の受け入れ条件）。
    #[test]
    fn with_builtins_registers_image_in_display_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("image")
            .expect("image エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Display);

        let display_entries = catalog.entries_in(CatalogCategory::Display);
        assert!(display_entries.iter().any(|e| e.name == "image"));
    }

    /// `icon` エントリが `Display` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S39 / Issue #134、カタログ登録の受け入れ条件）。
    #[test]
    fn with_builtins_registers_icon_in_display_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("icon")
            .expect("icon エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Display);

        let display_entries = catalog.entries_in(CatalogCategory::Display);
        assert!(display_entries.iter().any(|e| e.name == "icon"));
    }

    /// `scroll_view` エントリが `Container` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S42 / Issue #137、カタログ登録の受け入れ条件）。
    #[test]
    fn with_builtins_registers_scroll_view_in_container_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("scroll_view")
            .expect("scroll_view エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Container);

        let container_entries = catalog.entries_in(CatalogCategory::Container);
        assert!(container_entries.iter().any(|e| e.name == "scroll_view"));
    }

    /// `stack` エントリが `Container` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S45 / Issue #140、カタログ登録の受け入れ条件）。
    #[test]
    fn with_builtins_registers_stack_in_container_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("stack")
            .expect("stack エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Container);

        let container_entries = catalog.entries_in(CatalogCategory::Container);
        assert!(container_entries.iter().any(|e| e.name == "stack"));
    }

    /// `grid` エントリが `Container` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S45 / Issue #140、カタログ登録の受け入れ条件）。
    #[test]
    fn with_builtins_registers_grid_in_container_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("grid")
            .expect("grid エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Container);

        let container_entries = catalog.entries_in(CatalogCategory::Container);
        assert!(container_entries.iter().any(|e| e.name == "grid"));
    }

    /// `stack_preview_screen` / `grid_preview_screen`（[`preview_stack`] /
    /// [`preview_grid`] が実際に利用する画面）が `Widget<S>` トレイト経由で本物の
    /// `Stack<S>` / `Grid<S>`（`Node::Widget` 経由）を保持していることを固定化する。
    /// `render_headless` は `Node::Widget` を不透過に扱う契約（`preview_stack` /
    /// `preview_grid` doc 参照）のため `builtin_previews_produce_non_empty_output` 系の
    /// 出力比較だけでは実 API 接続を検証できない。ここでは `Screen::visit` で
    /// `Node::Widget(widget)` を取り出し、`Widget::accessibility_role`（`Stack<S>` /
    /// `Grid<S>` はいずれも [`AccessibilityRole::Group`]、`stack_grid.rs` 参照）と
    /// `Widget::measure` の積み上げ結果（子 2 件分の `Label::fixed` の自然寸法の和以上）
    /// で実体が「子を並べるコンテナ」であることを検証する。プレースホルダ
    /// （`.text()` のみ）へ後退した場合、`Node::Widget` が現れず本テストは失敗する。
    #[test]
    fn stack_preview_widget_is_backed_by_real_stack_with_two_children() {
        let screen = stack_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured_height = 0.0_f32;
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Group);
                measured_height = widget.measure(available).height;
            }
        });
        assert_eq!(
            widget_count, 1,
            "Stack<S> は Node::Widget として 1 件現れるべき"
        );

        let label_height = Label::<PreviewState>::fixed(WidgetId::from_raw(9), "項目 1")
            .measure(available)
            .height;
        assert!(
            measured_height >= label_height * 2.0,
            "Stack の主軸寸法は子 2 件分の自然寸法の和以上であるべき（実際: {measured_height}）"
        );
    }

    /// `dialog` エントリが `Overlay` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S48 / Issue #143、カタログ登録の受け入れ条件。`Overlay` カテゴリが
    /// 空へ戻る回帰を検知する）。
    #[test]
    fn with_builtins_registers_dialog_in_overlay_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("dialog")
            .expect("dialog エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Overlay);

        let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
        assert!(overlay_entries.iter().any(|e| e.name == "dialog"));
    }

    /// `menu` エントリが `Overlay` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S51 / Issue #146、カタログ登録の受け入れ条件。`Overlay` カテゴリが
    /// `dialog` に続き 2 件目になることを検知する）。
    #[test]
    fn with_builtins_registers_menu_in_overlay_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("menu")
            .expect("menu エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Overlay);

        let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
        assert!(overlay_entries.iter().any(|e| e.name == "menu"));
        // 件数の固定値検証は `tabs` エントリ追加（TASK-8.3-S54）で 3 件目になるため、
        // このテストでは `menu` 単体の到達性のみを検証する（`Overlay` の総数は
        // `with_builtins_registers_tabs_in_overlay_category` 側で固定化する）。
    }

    /// `tabs` エントリが `Overlay` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S54 / Issue #149、カタログ登録の受け入れ条件）。
    /// 件数の固定値検証は `tooltip` エントリ追加（TASK-8.3-S57）で 4 件目になるため、
    /// このテストでは `tabs` 単体の到達性のみを検証する（`Overlay` の総数は
    /// `with_builtins_registers_tooltip_in_overlay_category` 側で固定化する）。
    #[test]
    fn with_builtins_registers_tabs_in_overlay_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("tabs")
            .expect("tabs エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Overlay);

        let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
        assert!(overlay_entries.iter().any(|e| e.name == "tabs"));
    }

    /// `tooltip` エントリが `Overlay` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S57 / Issue #152、カタログ登録の受け入れ条件。`Overlay` カテゴリが
    /// `dialog` / `menu` / `tabs` に続き 4 件目になることを検知する）。
    #[test]
    fn with_builtins_registers_tooltip_in_overlay_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("tooltip")
            .expect("tooltip エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Overlay);

        let overlay_entries = catalog.entries_in(CatalogCategory::Overlay);
        assert!(overlay_entries.iter().any(|e| e.name == "tooltip"));
        assert_eq!(overlay_entries.len(), 4);
    }

    /// [`tabs_preview_screen`]（[`preview_tabs`] が実際に利用する画面）が `Widget<S>`
    /// トレイト経由で本物の `Tabs<S>`（`Node::Widget` 経由）を保持していることを固定化する
    /// （TASK-8.3-S54 / Issue #149）。`render_headless` は `Node::Widget` を不透過に扱う
    /// 契約（`preview_tabs` doc 参照）のため `builtin_previews_produce_non_empty_output`
    /// 系の出力比較だけでは実 API 接続を検証できない。ここでは `Screen::visit` で
    /// `Node::Widget(widget)` を取り出し、`Widget::accessibility_role` が
    /// [`AccessibilityRole::TabList`]（`Tabs<S>` のみが持つ専用ロール、`tabs.rs` 参照）
    /// であることを検証する。`Screen::visit` は共有参照のみの走査であり
    /// `sync_from_state`（`&mut self` 要求）を呼べないため（`widget.rs` 契約）、
    /// `cached_selected` は既定の `None` のまま。`measure` は選択状態に依存しない
    /// タブバー + コンテンツ領域の合計寸法（`tabs.rs` の `Widget::measure` doc 参照）を
    /// 返すため正の寸法であることを併せて固定化する（プレースホルダ（`.text()` のみ）へ
    /// 後退した場合、`Node::Widget` が現れず本テストは失敗する）。
    #[test]
    fn tabs_preview_widget_is_backed_by_real_tabs() {
        let screen = tabs_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::TabList);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Tabs<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// [`tooltip_preview_screen`]（[`preview_tooltip`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `Tooltip<S>`（`Node::Widget` 経由）を保持している
    /// ことを固定化する（TASK-8.3-S57 / Issue #152）。`render_headless` は `Node::Widget`
    /// を不透過に扱う契約（`preview_tooltip` doc 参照）のため
    /// `builtin_previews_produce_non_empty_output` 系の出力比較だけでは実 API 接続を
    /// 検証できない。ここでは `Screen::visit` で `Node::Widget(widget)` を取り出し、
    /// `Widget::accessibility_role` が [`AccessibilityRole::Tooltip`]（`Tooltip<S>` のみが
    /// 持つ専用ロール、`tooltip.rs` 参照）であることを検証する。`Screen::visit` は
    /// 共有参照のみの走査であり `sync_from_state`（`&mut self` 要求）を呼べないため
    /// （`widget.rs` 契約）、`cached_text` は構築時の固定値のまま。`measure` はアンカー
    /// （`Label::fixed`）の計測結果をそのまま返す契約（`tooltip.rs` 参照）のため正の
    /// 寸法であることを併せて固定化する（プレースホルダ（`.text()` のみ）へ後退した場合、
    /// `Node::Widget` が現れず本テストは失敗する）。
    #[test]
    fn tooltip_preview_widget_is_backed_by_real_tooltip() {
        let screen = tooltip_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Tooltip);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Tooltip<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// `card` エントリが `Container` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S60 / Issue #155、カタログ登録の受け入れ条件。`Container` カテゴリが
    /// `list` / `scroll_view` / `stack` / `grid` に続き 5 件目になることを検知する）。
    #[test]
    fn with_builtins_registers_card_in_container_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("card")
            .expect("card エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Container);

        let container_entries = catalog.entries_in(CatalogCategory::Container);
        assert!(container_entries.iter().any(|e| e.name == "card"));
        assert_eq!(container_entries.len(), 5);
    }

    /// `separator` エントリが `Display` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S60 / Issue #155、カタログ登録の受け入れ条件。`Display` カテゴリが
    /// `text` / `image` / `icon` / `label` / `progress_bar` / `spinner` に続き 7 件目に
    /// なることを検知する）。
    #[test]
    fn with_builtins_registers_separator_in_display_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("separator")
            .expect("separator エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Display);

        let display_entries = catalog.entries_in(CatalogCategory::Display);
        assert!(display_entries.iter().any(|e| e.name == "separator"));
        assert_eq!(display_entries.len(), 7);
    }

    /// [`card_preview_screen`]（[`preview_card`] が実際に利用する画面）が `Widget<S>`
    /// トレイト経由で本物の `Card<S>`（`Node::Widget` 経由）を保持していることを固定化する
    /// （TASK-8.3-S60 / Issue #155）。`render_headless` は `Node::Widget` を不透過に扱う
    /// 契約（`preview_card` doc 参照）のため `builtin_previews_produce_non_empty_output`
    /// 系の出力比較だけでは実 API 接続を検証できない。ここでは `Screen::visit` で
    /// `Node::Widget(widget)` を取り出し、`Widget::accessibility_role` が
    /// [`AccessibilityRole::Group`]（`Card<S>` は `Stack<S>` / `Grid<S>` と共用する
    /// ロール、`card.rs` 参照）であることを検証する。`Card` はロールを共有するため
    /// `Stack` / `Grid` の回帰テストと同じく、`measure` が正の寸法（子 3 件 + padding
    /// 分）を返すことを併せて固定化する（プレースホルダ（`.text()` のみ）へ後退した場合、
    /// `Node::Widget` が現れず本テストは失敗する）。
    #[test]
    fn card_preview_widget_is_backed_by_real_card() {
        let screen = card_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Group);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Card<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// [`separator_preview_screen`]（[`preview_separator`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `Separator<S>`（`Node::Widget` 経由）を 2 件
    /// （水平・垂直）保持していることを固定化する（TASK-8.3-S60 / Issue #155）。
    /// `Separator<S>` のみが持つ専用ロール [`AccessibilityRole::Separator`]（`card.rs`
    /// 参照）を検証することで、`Card` の `Group` ロール共用とは独立した強い判別を行う。
    #[test]
    fn separator_preview_widget_is_backed_by_real_separator() {
        let screen = separator_preview_screen();
        let mut widget_count = 0;
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Separator);
            }
        });
        assert_eq!(
            widget_count, 2,
            "Separator<S> は Node::Widget として 2 件（水平・垂直）現れるべき"
        );
    }

    /// [`menu_preview_screen`]（[`preview_menu`] が実際に利用する画面）が `Widget<S>`
    /// トレイト経由で本物の `Menu<S>`（`Node::Widget` 経由）を保持していることを
    /// 固定化する（TASK-8.3-S51 / Issue #146）。`render_headless` は `Node::Widget` を
    /// 不透過に扱う契約（`preview_menu` doc 参照）のため
    /// `builtin_previews_produce_non_empty_output` 系の出力比較だけでは実 API 接続を
    /// 検証できない。ここでは `Screen::visit` で `Node::Widget(widget)` を取り出し、
    /// `Widget::accessibility_role` が [`AccessibilityRole::ComboBox`]（`Menu<S>` のみが
    /// 持つ専用ロール、`menu.rs` 参照）であることを検証する。`Screen::visit` は
    /// 共有参照のみの走査であり `sync_from_state`（`&mut self` 要求）を呼べないため
    /// （`widget.rs` 契約）、`cached_open` は既定の閉状態のまま。`measure` はその
    /// ドキュメント化済みの閉状態既定契約（トリガー矩形のみ、`menu.rs` 参照）どおりで
    /// あることを併せて固定化する（プレースホルダ（`.text()` のみ）へ後退した場合、
    /// `Node::Widget` が現れず本テストは失敗する）。
    #[test]
    fn menu_preview_widget_is_backed_by_real_menu() {
        let screen = menu_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::ComboBox);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Menu<S> は Node::Widget として 1 件現れるべき"
        );
        // sync_from_state 未実行（既定の閉状態）のため、ドキュメント化済みの契約通り
        // トリガー矩形のみの寸法が返る（`menu.rs` の `Widget::measure` doc 参照）。
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// [`dialog_preview_screen`]（[`preview_dialog`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `Dialog<S>`（`Node::Widget` 経由）を保持している
    /// ことを固定化する（TASK-8.3-S48 / Issue #143）。`render_headless` は `Node::Widget`
    /// を不透過に扱う契約（`preview_dialog` doc 参照）のため
    /// `builtin_previews_produce_non_empty_output` 系の出力比較だけでは実 API 接続を
    /// 検証できない。ここでは `Screen::visit` で `Node::Widget(widget)` を取り出し、
    /// `Widget::accessibility_role` が [`AccessibilityRole::Dialog`]（`Dialog<S>` のみが
    /// 持つ専用ロール、`dialog.rs` 参照）であることを検証する。`Screen::visit` は
    /// 共有参照のみの走査であり `sync_from_state`（`&mut self` 要求）を呼べないため
    /// （`widget.rs` 契約）、`cached_open` は既定の閉状態のまま。`measure` はその
    /// ドキュメント化済みの閉状態既定契約（ゼロ寸法、`dialog.rs` 参照）どおりであることを
    /// 併せて固定化する（プレースホルダ（`.text()` のみ）へ後退した場合、`Node::Widget`
    /// が現れず本テストは失敗する）。
    #[test]
    fn dialog_preview_widget_is_backed_by_real_dialog() {
        let screen = dialog_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Dialog);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Dialog<S> は Node::Widget として 1 件現れるべき"
        );
        // sync_from_state 未実行（既定の閉状態）のため、ドキュメント化済みの契約通り
        // ゼロ寸法が返る（`dialog.rs` の `Widget::measure` doc 参照）。
        assert_eq!(measured.width, 0.0);
        assert_eq!(measured.height, 0.0);
    }

    /// [`stack_preview_widget_is_backed_by_real_stack_with_two_children`] と対の
    /// `grid` 版。`Grid<S>::measure`（`stack_grid.rs` 参照）は幅を `available.width`
    /// 充填、高さを行高さの合計として返す（`Stack` のような子の合計とは非対称）ため、
    /// ここでは「空 children なら `(0.0, 0.0)`」という [`Grid::measure`] の契約を使い、
    /// 高さが `0.0` より大きい（= 少なくとも 1 行分の子が実在する）ことを検証する。
    #[test]
    fn grid_preview_widget_is_backed_by_real_grid_with_children() {
        let screen = grid_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured_height = 0.0_f32;
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Group);
                measured_height = widget.measure(available).height;
            }
        });
        assert_eq!(
            widget_count, 1,
            "Grid<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(
            measured_height > 0.0,
            "Grid に children が実在すれば高さは 0.0 より大きいはず（実際: {measured_height}）"
        );
    }

    /// `label` エントリが `Display` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S12 / Issue #107、カタログ登録の受け入れ条件。`Display` カテゴリは
    /// 最終的に `text` / `image` / `icon` / `label` / `progress_bar` / `spinner` /
    /// `separator`（TASK-8.3-S60 / Issue #155）の 7 件になる）。
    #[test]
    fn with_builtins_registers_label_in_display_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("label")
            .expect("label エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Display);

        let display_entries = catalog.entries_in(CatalogCategory::Display);
        assert!(display_entries.iter().any(|e| e.name == "label"));
        assert_eq!(display_entries.len(), 7);
    }

    /// `label` エントリの preview（[`preview_label`]）が `Screen::label` 経由で
    /// `Label<S>`（`Node::Widget` 経由）を実際に配置していることを固定化する
    /// （TASK-8.3-S12 / Issue #107）。`render_headless` は `Node::Widget` を不透過に
    /// 扱う契約（`preview_label` doc 参照）のため
    /// `builtin_previews_produce_non_empty_output` 系の出力比較だけでは実 API 接続を
    /// 検証できない。ここでは preview 用の画面を直接組み立て `Screen::visit` で
    /// `Node::Widget(widget)` を取り出し、`Widget::accessibility_role` が
    /// [`AccessibilityRole::Label`] であることを検証する。`Screen::visit` は
    /// 共有参照のみの走査であり `sync_from_state`（`&mut self` 要求）を呼べないため
    /// （`widget.rs` 契約）、`Bound` テキストの `cached_text` は既定の空文字列のまま
    /// （`label.rs` 参照）。`measure` は空文字列に対する寸法（幅 0.0・高さ
    /// `APPROX_LINE_HEIGHT` 相当）を返す契約のため、高さが正であることを併せて
    /// 固定化する（プレースホルダ（`.text()` のみ）へ後退した場合、`Node::Widget` が
    /// 現れず本テストは失敗する）。
    #[test]
    fn label_preview_widget_is_backed_by_real_label() {
        let screen: Screen<PreviewState> =
            Screen::new("Label プレビュー").label(WidgetId::from_raw(1), preview_label_get);
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Label);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Label<S> は Node::Widget として 1 件現れるべき"
        );
        // sync_from_state 未実行（Screen::visit は共有参照走査のみ）のため、
        // Bound テキストの cached_text は既定の空文字列のまま（label.rs 参照）。
        // 高さは文字数に依らず単一行の近似行高を返す契約（label.rs の
        // Widget::measure doc 参照）のため、正の寸法であることのみを固定化する。
        assert!(measured.height > 0.0);
    }

    /// [`button_preview_screen`]（[`preview_button`] が実際に利用する画面）が `Widget<S>`
    /// トレイト経由で本物の `Button<S>`（`Node::Widget` 経由）を保持していることを固定化する
    /// （TASK-8.3-S9 / Issue #104）。`render_headless` は `Node::Widget` を不透過に扱う契約
    /// （`preview_button` doc 参照）のため `builtin_previews_produce_non_empty_output` 系の
    /// 出力比較だけでは実 API 接続を検証できない。ここでは `Screen::visit` で
    /// `Node::Widget(widget)` を取り出し、`Widget::accessibility_role` が
    /// [`AccessibilityRole::Button`]（`button.rs` 参照）であること、`measure` が正の寸法を
    /// 返すことを検証する（プレースホルダ（`.text()` のみ）へ後退した場合、`Node::Widget`
    /// が現れず本テストは失敗する）。
    #[test]
    fn button_preview_widget_is_backed_by_real_button() {
        let screen = button_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Button);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Button<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// [`text_field_preview_screen`]（[`preview_text_field`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `TextInput<S>`（`Node::Widget` 経由）を保持している
    /// ことを固定化する（TASK-8.3-S15 / Issue #110、`button_preview_widget_is_backed_by_real_button`
    /// と同型）。`render_headless` は `Node::Widget` を不透過に扱う契約
    /// （`preview_text_field` doc 参照）のため `builtin_previews_produce_non_empty_output` 系の
    /// 出力比較だけでは実 API 接続を検証できない。ここでは `Screen::visit` で
    /// `Node::Widget(widget)` を取り出し、`Widget::accessibility_role` が
    /// [`AccessibilityRole::TextInput`]（`text_input.rs` 参照）であること、`measure` が
    /// 正の寸法を返すことを検証する（プレースホルダ（`.text()` のみ）へ後退した場合、
    /// `Node::Widget` が現れず本テストは失敗する）。
    #[test]
    fn text_field_preview_widget_is_backed_by_real_text_input() {
        let screen = text_field_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::TextInput);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "TextInput<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// [`list_preview_screen`]（[`preview_list`] が実際に利用する画面）が `Widget<S>`
    /// トレイト経由で本物の `ListView<S>`（`Node::Widget` 経由）を保持していることを
    /// 固定化する（TASK-8.3-S18 / Issue #113）。`render_headless` は `Node::Widget` を
    /// 不透過に扱う契約（`preview_list` doc 参照）のため
    /// `builtin_previews_produce_non_empty_output` 系の出力比較だけでは実 API 接続を
    /// 検証できない。ここでは `Screen::visit` で `Node::Widget(widget)` を取り出し、
    /// `Widget::accessibility_role` が [`AccessibilityRole::List`]（`list_view.rs` 参照）
    /// であること、`measure` が正の寸法を返すことを検証する（プレースホルダ
    /// （`.list()` のみ）へ後退した場合、`Node::Widget` が現れず本テストは失敗する）。
    #[test]
    fn list_preview_widget_is_backed_by_real_list_view() {
        let screen = list_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::List);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "ListView<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    #[test]
    fn builtin_previews_produce_non_empty_output() {
        let catalog = WidgetCatalog::with_builtins();
        for entry in catalog.entries() {
            let lines = (entry.preview)();
            assert!(
                !lines.is_empty(),
                "preview for {} should not be empty",
                entry.name
            );
        }
    }

    /// `checkbox` エントリが `Input` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S21 / Issue #116、カタログ登録の受け入れ条件。`Input` カテゴリが
    /// `text_field` / `radio_button` / `toggle_switch` に続き 4 件目になることを検知する）。
    #[test]
    fn with_builtins_registers_checkbox_in_input_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("checkbox")
            .expect("checkbox エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Input);

        let input_entries = catalog.entries_in(CatalogCategory::Input);
        assert!(input_entries.iter().any(|e| e.name == "checkbox"));
        assert_eq!(input_entries.len(), 5);
    }

    /// `slider` エントリが `Input` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S30 / Issue #125、カタログ登録の受け入れ条件。`Input` カテゴリが
    /// `text_field` / `radio_button` / `toggle_switch` / `checkbox` に続き 5 件目に
    /// なることを検知する）。
    #[test]
    fn with_builtins_registers_slider_in_input_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("slider")
            .expect("slider エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Input);

        let input_entries = catalog.entries_in(CatalogCategory::Input);
        assert!(input_entries.iter().any(|e| e.name == "slider"));
        assert_eq!(input_entries.len(), 5);
    }

    /// [`slider_preview_screen`]（[`preview_slider`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `Slider<S>`（`Node::Widget` 経由）を保持している
    /// ことを固定化する（TASK-8.3-S30 / Issue #125）。`render_headless` は `Node::Widget`
    /// を不透過に扱う契約（`preview_slider` doc 参照）のため
    /// `builtin_previews_produce_non_empty_output` 系の出力比較だけでは実 API 接続を
    /// 検証できない。ここでは `Screen::visit` で `Node::Widget(widget)` を取り出し、
    /// `Widget::accessibility_role` が [`AccessibilityRole::Slider`]（`slider.rs` 参照）
    /// であることを検証する。`Screen::visit` は共有参照のみの走査であり
    /// `sync_from_state`（`&mut self` 要求）を呼べないため（`widget.rs` 契約）、
    /// `cached_value` は既定の `min` のまま。`measure` は正の寸法を返す契約
    /// （`slider.rs` の `Widget::measure` doc 参照）のため、併せて固定化する
    /// （プレースホルダ（`.text()` のみ）へ後退した場合、`Node::Widget` が現れず
    /// 本テストは失敗する）。
    #[test]
    fn slider_preview_widget_is_backed_by_real_slider() {
        let screen = slider_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::Slider);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Slider<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// [`checkbox_preview_screen`]（[`preview_checkbox`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `Checkbox<S>`（`Node::Widget` 経由）を保持している
    /// ことを固定化する（TASK-8.3-S21 / Issue #116）。`render_headless` は `Node::Widget`
    /// を不透過に扱う契約（`preview_checkbox` doc 参照）のため
    /// `builtin_previews_produce_non_empty_output` 系の出力比較だけでは実 API 接続を
    /// 検証できない。ここでは `Screen::visit` で `Node::Widget(widget)` を取り出し、
    /// `Widget::accessibility_role` が [`AccessibilityRole::CheckBox`]（`checkbox.rs` 参照）
    /// であることを検証する。`Screen::visit` は共有参照のみの走査であり
    /// `sync_from_state`（`&mut self` 要求）を呼べないため（`widget.rs` 契約）、
    /// `cached_checked` は既定の未チェックのまま。`measure` は `BOX_SIZE` +
    /// ボックス・ラベル間ギャップ + ラベル近似幅の合計寸法（`checkbox.rs` の
    /// `Widget::measure` doc 参照）を返す契約のため、正の寸法であることを併せて
    /// 固定化する（プレースホルダ（`.text()` のみ）へ後退した場合、`Node::Widget` が
    /// 現れず本テストは失敗する）。
    #[test]
    fn checkbox_preview_widget_is_backed_by_real_checkbox() {
        let screen = checkbox_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(widget.accessibility_role(), AccessibilityRole::CheckBox);
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Checkbox<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// `progress_bar` エントリが `Display` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S33 / Issue #128、カタログ登録の受け入れ条件。`Display` カテゴリは
    /// 最終的に `text` / `image` / `icon` / `label` / `progress_bar` / `spinner` /
    /// `separator`（TASK-8.3-S60 / Issue #155）の 7 件になる）。
    #[test]
    fn with_builtins_registers_progress_bar_in_display_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("progress_bar")
            .expect("progress_bar エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Display);

        let display_entries = catalog.entries_in(CatalogCategory::Display);
        assert!(display_entries.iter().any(|e| e.name == "progress_bar"));
        assert_eq!(display_entries.len(), 7);
    }

    /// `spinner` エントリが `Display` カテゴリで一意に検索できることを確認する
    /// （TASK-8.3-S33 / Issue #128、カタログ登録の受け入れ条件。`Display` カテゴリの
    /// 6 件目、`progress_bar` エントリと対）。
    #[test]
    fn with_builtins_registers_spinner_in_display_category() {
        let catalog = WidgetCatalog::with_builtins();

        let entry = catalog
            .find("spinner")
            .expect("spinner エントリが登録されているべき");
        assert_eq!(entry.category, CatalogCategory::Display);

        let display_entries = catalog.entries_in(CatalogCategory::Display);
        assert!(display_entries.iter().any(|e| e.name == "spinner"));
    }

    /// [`progress_bar_preview_screen`]（[`preview_progress_bar`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `ProgressBar<S>`（`Node::Widget` 経由）を保持して
    /// いることを固定化する（TASK-8.3-S33 / Issue #128）。`render_headless` は
    /// `Node::Widget` を不透過に扱う契約（`preview_progress_bar` doc 参照）のため出力比較
    /// だけでは実 API 接続を検証できない。`Screen::visit` で `Node::Widget(widget)` を
    /// 取り出し、`Widget::accessibility_role` が [`AccessibilityRole::ProgressIndicator`]
    /// （`progress.rs` 参照、`ProgressBar` / `Spinner` 共用ロール）であることを検証する。
    #[test]
    fn progress_bar_preview_widget_is_backed_by_real_progress_bar() {
        let screen = progress_bar_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(
                    widget.accessibility_role(),
                    AccessibilityRole::ProgressIndicator
                );
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "ProgressBar<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    /// [`spinner_preview_screen`]（[`preview_spinner`] が実際に利用する画面）が
    /// `Widget<S>` トレイト経由で本物の `Spinner<S>`（`Node::Widget` 経由）を保持している
    /// ことを固定化する（TASK-8.3-S33 / Issue #128、[`progress_bar_preview_widget_is_backed_by_real_progress_bar`]
    /// と同じ検証様式）。
    #[test]
    fn spinner_preview_widget_is_backed_by_real_spinner() {
        let screen = spinner_preview_screen();
        let mut widget_count = 0;
        let available = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let mut measured = Measurement {
            width: -1.0,
            height: -1.0,
        };
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                widget_count += 1;
                assert_eq!(
                    widget.accessibility_role(),
                    AccessibilityRole::ProgressIndicator
                );
                measured = widget.measure(available);
            }
        });
        assert_eq!(
            widget_count, 1,
            "Spinner<S> は Node::Widget として 1 件現れるべき"
        );
        assert!(measured.width > 0.0);
        assert!(measured.height > 0.0);
    }

    #[test]
    fn gallery_state_formats_entries_with_category_label() {
        let catalog = WidgetCatalog::with_builtins();
        let gallery_state = catalog.gallery_state();

        assert_eq!(gallery_state.entry_lines.len(), 22);
        assert!(gallery_state.entry_lines[0].contains("text（表示）"));
        assert!(gallery_state.entry_lines[2].contains("button（アクション）"));
        assert!(gallery_state.entry_lines[4].contains("radio_button（入力）"));
        assert!(gallery_state.entry_lines[5].contains("toggle_switch（入力）"));
        assert!(gallery_state.entry_lines[6].contains("image（表示）"));
        assert!(gallery_state.entry_lines[7].contains("icon（表示）"));
        assert!(gallery_state.entry_lines[8].contains("scroll_view（コンテナ）"));
        assert!(gallery_state.entry_lines[9].contains("stack（コンテナ）"));
        assert!(gallery_state.entry_lines[10].contains("grid（コンテナ）"));
        assert!(gallery_state.entry_lines[11].contains("dialog（オーバーレイ）"));
        assert!(gallery_state.entry_lines[12].contains("menu（オーバーレイ）"));
        assert!(gallery_state.entry_lines[13].contains("tabs（オーバーレイ）"));
        assert!(gallery_state.entry_lines[14].contains("tooltip（オーバーレイ）"));
        assert!(gallery_state.entry_lines[15].contains("label（表示）"));
        assert!(gallery_state.entry_lines[16].contains("checkbox（入力）"));
        assert!(gallery_state.entry_lines[17].contains("slider（入力）"));
        assert!(gallery_state.entry_lines[18].contains("progress_bar（表示）"));
        assert!(gallery_state.entry_lines[19].contains("spinner（表示）"));
        assert!(gallery_state.entry_lines[20].contains("card（コンテナ）"));
        assert!(gallery_state.entry_lines[21].contains("separator（表示）"));
    }

    #[test]
    fn gallery_screen_renders_title_and_entry_lines_headless() {
        let catalog = WidgetCatalog::with_builtins();
        let gallery_state = catalog.gallery_state();
        let screen = catalog.gallery_screen();

        let rendered = render_headless(&screen, &gallery_state);

        assert_eq!(rendered[0], "# ウィジェットカタログ");
        assert!(rendered.contains(&"登録済みウィジェット一覧".to_string()));
        for line in &gallery_state.entry_lines {
            assert!(rendered.contains(line));
        }
    }
}
