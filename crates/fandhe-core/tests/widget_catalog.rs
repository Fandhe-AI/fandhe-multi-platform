//! `ui::WidgetCatalog`（TASK-8.3-S6 / REQ-8 / Issue #101）の結合テスト。
//!
//! `crates/fandhe-core/src/ui/catalog.rs` が公開する登録・列挙 API とサンプル
//! ギャラリー画面について、「組み込みエントリの構成」「ギャラリーのヘッドレス描画」
//! 「重複登録エラー」「カテゴリ絞り込み」を、クレート外部（`tests/`）から公開 API のみを
//! 用いて検証する（親 issue #95、ウィジェットセット新規開発）。
//! `progress_bar` / `spinner` エントリは `ProgressBar<S>` / `Spinner<S>`（Issue #126）を
//! `Screen::progress_bar` / `Screen::spinner` 経由で登録する。

use fandhe_core::render_headless;
use fandhe_core::ui::{CatalogCategory, CatalogEntry, CatalogError, WidgetCatalog};

/// 組み込みカタログが Text / TextField / Button / List / RadioButton / ToggleSwitch /
/// Image / Icon / ScrollView / Stack / Grid / Dialog / Menu / Tabs / Tooltip / Label /
/// Checkbox / Slider / ProgressBar / Spinner / Card / Separator の 22 エントリをこの順で
/// 保持することを確認する。`stack` / `grid` は `Stack<S>` / `Grid<S>`（Issue #138）へ
/// `Screen::stack` / `Screen::grid` 経由で接続するエントリ。`dialog` は `Dialog<S>`
/// （Issue #141）へ `Screen::dialog` 経由で接続するエントリ。`menu` は `Menu<S>`
/// （Issue #144）へ `Screen::menu` 経由で接続するエントリ。`tabs` は `Tabs<S>`
/// （Issue #147）へ `Screen::tabs` 経由で接続するエントリ。`tooltip` は `Tooltip<S>`
/// （Issue #150）へ `Screen::tooltip` 経由で接続するエントリ。`label` は `Label<S>`
/// （Issue #105）へ `Screen::label` 経由で接続するエントリ（`catalog.rs` の
/// `with_builtins` doc 参照）。`checkbox` は `Checkbox<S>`（Issue #114）へ
/// `Screen::checkbox` 経由で接続するエントリ。`slider` は `Slider<S>`（Issue #123）へ
/// `Screen::slider` 経由で接続するエントリ（状態・イベント処理は Issue #124 のスコープ、
/// `catalog.rs` の `builtin_entries` doc 参照）。`progress_bar` / `spinner` は
/// `ProgressBar<S>` / `Spinner<S>`（Issue #126）へ `Screen::progress_bar` /
/// `Screen::spinner` 経由で接続するエントリ。`card` / `separator` は `Card<S>` /
/// `Separator<S>`（Issue #153）へ `Screen::card` / `Screen::separator` 経由で接続する
/// エントリ（`catalog.rs` の `with_builtins` doc 参照）。`button` の接続先はレガシー
/// `Node::Button`（`Screen::button` ビルダー）ではなく `Widget<S>` 実装型 `Button<S>`
/// （Issue #102、#103）。`name` はキーとして登録順・件数を規定する（`catalog.rs` の
/// `builtin_entries` doc 参照）。
#[test]
fn with_builtins_contains_twenty_two_entries_in_order() {
    let catalog = WidgetCatalog::with_builtins();
    let names: Vec<&str> = catalog.entries().iter().map(|entry| entry.name).collect();

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

/// 組み込みカタログのギャラリー画面をヘッドレス描画すると、タイトル行と
/// 22 エントリすべての表示行が含まれることを確認する。
#[test]
fn gallery_screen_headless_render_includes_title_and_all_entries() {
    let catalog = WidgetCatalog::with_builtins();
    let state = catalog.gallery_state();
    let screen = catalog.gallery_screen();

    let rendered = render_headless(&screen, &state);

    assert_eq!(rendered[0], "# ウィジェットカタログ");
    assert!(rendered.iter().any(|line| line.contains("text（表示）")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("text_field（入力）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("button（アクション）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("list（コンテナ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("radio_button（入力）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("toggle_switch（入力）"))
    );
    assert!(rendered.iter().any(|line| line.contains("image（表示）")));
    assert!(rendered.iter().any(|line| line.contains("icon（表示）")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("scroll_view（コンテナ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("stack（コンテナ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("grid（コンテナ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("dialog（オーバーレイ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("menu（オーバーレイ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("tabs（オーバーレイ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("tooltip（オーバーレイ）"))
    );
    assert!(rendered.iter().any(|line| line.contains("label（表示）")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("checkbox（入力）"))
    );
    assert!(rendered.iter().any(|line| line.contains("slider（入力）")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("progress_bar（表示）"))
    );
    assert!(rendered.iter().any(|line| line.contains("spinner（表示）")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("card（コンテナ）"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("separator（表示）"))
    );
}

/// 各組み込みエントリの `preview` 関数が空でない出力を返すことを確認する
/// （S7 以降の登録エントリが倣うテンプレートとして、preview 契約が機能することの
/// 最小確認）。
#[test]
fn builtin_entry_previews_are_non_empty() {
    let catalog = WidgetCatalog::with_builtins();

    for entry in catalog.entries() {
        let lines = (entry.preview)();
        assert!(
            !lines.is_empty(),
            "{} の preview は空であってはならない",
            entry.name
        );
    }
}

/// 同名エントリの登録は `CatalogError::DuplicateName` を返し、カタログの状態を
/// 変更しないことを確認する（panic を API 契約にしない、coding-rust.md）。
#[test]
fn register_duplicate_name_returns_error_without_mutating_catalog() {
    let mut catalog = WidgetCatalog::new();
    catalog
        .register(CatalogEntry {
            name: "custom-widget",
            category: CatalogCategory::Display,
            description: "テスト用エントリ",
            preview: || vec!["dummy".to_string()],
        })
        .expect("初回登録は成功するはず");

    let result = catalog.register(CatalogEntry {
        name: "custom-widget",
        category: CatalogCategory::Action,
        description: "別のテスト用エントリ",
        preview: || vec!["dummy2".to_string()],
    });

    assert_eq!(result, Err(CatalogError::DuplicateName("custom-widget")));
    assert_eq!(catalog.entries().len(), 1);
}

/// `entries_in` がカテゴリごとに正しく絞り込むことを確認する。組み込みカタログは
/// `Input` に `text_field` / `radio_button` / `toggle_switch` / `checkbox` / `slider` の
/// 5 件、`Display` に `text` / `image` / `icon` / `label` / `progress_bar` / `spinner` /
/// `separator` の 7 件、`Container` に `list` / `scroll_view` / `stack` / `grid` /
/// `card` の 5 件、`Overlay` に `dialog` / `menu` / `tabs` / `tooltip` の 4 件、
/// 残る `Action` はちょうど 1 件（`button`）属する。
#[test]
fn entries_in_filters_builtin_catalog_by_category() {
    let catalog = WidgetCatalog::with_builtins();

    assert_eq!(catalog.entries_in(CatalogCategory::Display).len(), 7);
    assert_eq!(catalog.entries_in(CatalogCategory::Input).len(), 5);
    assert_eq!(catalog.entries_in(CatalogCategory::Action).len(), 1);
    assert_eq!(catalog.entries_in(CatalogCategory::Container).len(), 5);
    assert_eq!(catalog.entries_in(CatalogCategory::Overlay).len(), 4);
}

/// `label` エントリが `Display` カテゴリで一意に検索できることを確認する
/// （TASK-8.3-S12 / Issue #107、カタログ登録の受け入れ条件）。
#[test]
fn with_builtins_registers_label_findable_in_display_category() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("label")
        .expect("label エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Display);
    assert_eq!(entry.name, "label");
}

/// `checkbox` エントリが `Input` カテゴリで一意に検索できることを確認する
/// （TASK-8.3-S21 / Issue #116、カタログ登録の受け入れ条件）。
#[test]
fn with_builtins_registers_checkbox_findable_in_input_category() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("checkbox")
        .expect("checkbox エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Input);
    assert_eq!(entry.name, "checkbox");
}

/// `slider` エントリが `Input` カテゴリで一意に検索できることを確認する
/// （TASK-8.3-S30 / Issue #125、カタログ登録の受け入れ条件）。
#[test]
fn with_builtins_registers_slider_findable_in_input_category() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("slider")
        .expect("slider エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Input);
    assert_eq!(entry.name, "slider");
}

/// `progress_bar` エントリが `Display` カテゴリで一意に検索できることを確認する
/// （TASK-8.3-S33 / Issue #128、カタログ登録の受け入れ条件）。
#[test]
fn with_builtins_registers_progress_bar_findable_in_display_category() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("progress_bar")
        .expect("progress_bar エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Display);
    assert_eq!(entry.name, "progress_bar");
}

/// `spinner` エントリが `Display` カテゴリで一意に検索できることを確認する
/// （TASK-8.3-S33 / Issue #128、カタログ登録の受け入れ条件）。
#[test]
fn with_builtins_registers_spinner_findable_in_display_category() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("spinner")
        .expect("spinner エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Display);
    assert_eq!(entry.name, "spinner");
}
