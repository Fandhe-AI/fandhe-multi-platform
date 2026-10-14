//! `TextInput<S>` / `TextFieldWidget<S>`（テキストフィールドウィジェット）の結合テスト
//! （TASK-8.3-S15 / REQ-8、Issue #110）。
//!
//! 親 issue #95（TASK-8.3）の一部。`crates/fandhe-core/src/ui/catalog.rs` の
//! `text_field` エントリの接続先はレガシー `Node::TextField`（`Screen::text_field`
//! ビルダー）ではなく `TextInput<S>`（`text_input.rs` 参照、Issue #108）
//! （`catalog.rs` の `builtin_entries` / `text_field_preview_screen` /
//! `preview_text_field` doc 参照）。
//!
//! `tests/button_widget.rs` が確立した 4 軸構成を踏襲する。ただし `TextInput<S>`
//! （Issue #108）と `TextFieldWidget<S>`（Issue #109）は型統合されておらず自己完結の
//! 2 型のまま並存する（`text_field.rs` モジュール doc §スコープ外参照）ため、軸 3 は
//! 両型それぞれの契約を固定化する:
//!
//! 1. `WidgetHarness` 契約（S5 / #100）: `Node::Widget(TextInput)` を含む画面でも
//!    フレームループのバージョン再利用・初回ライフサイクル順
//!    （Declared → Mounted → Layout → Paint）が壊れないこと。
//! 2. `render_headless` プレースホルダ契約: `Node::Widget` は未配線のまま安全側
//!    （無出力）であること。
//! 3. `Widget` トレイト直接呼び出し + イベント処理（クレート外部・公開 API 経由の契約
//!    固定化）: `TextInput<S>` の measure / paint / hit_test / accessibility_role /
//!    sync_from_state、`TextFieldWidget<S>` のフォーカス → 入力（制御文字除去・
//!    マルチバイト文字）→ Commit → `StateStore::dispatch` の E2E、disabled・非フォーカス中
//!    の入力無視、blur-commit、Cancel の巻き戻し、Debug マスク（`text_field.rs` /
//!    `text_input.rs` 内ユニットテストと重複する軸は代表ケースに絞る）。
//! 4. カタログ経由の到達性: `WidgetCatalog::with_builtins()` から `find("text_field")` で
//!    エントリが取得でき、`category == Input`・preview が panic せずタイトル行を返すこと。

use fandhe_core::{
    AccessibilityRole, CatalogCategory, Event, ImeEvent, ImePurpose, ImeRequest, InteractionInput,
    LifecyclePhase, Measurement, Node, PaintCommand, Screen, StateStore, TextFieldInput,
    TextFieldWidget, TextInput, Widget, WidgetCatalog, WidgetHarness, WidgetId, render_headless,
};

/// フォーム状態。`note` フィールドをテキストフィールドが読み書きする想定。
#[derive(Default)]
struct FormState {
    note: String,
}

fn get_note(state: &FormState) -> String {
    state.note.clone()
}

fn set_note(state: &mut FormState, value: String) {
    state.note = value;
}

fn text_input_screen() -> Screen<FormState> {
    Screen::new("フォーム画面").text_input(WidgetId::from_raw(1), "メモ", get_note, set_note)
}

fn sized(width: f32, height: f32) -> Measurement {
    Measurement { width, height }
}

/// `TextFieldWidget<S>` をフォーカス状態にする（`text_field.rs` 内ユニットテストの
/// `focus` ヘルパーと同じ操作を公開 API 経由で行う）。
fn focus(field: &mut TextFieldWidget<FormState>) {
    field.apply_input(InteractionInput::FocusGained);
}

// --- (1) WidgetHarness 契約 -------------------------------------------------

/// (1) `.text_input()` を含む `Screen` を `WidgetHarness` で駆動しても、S5 契約
/// （バージョン不変ならスナップショットを再利用する）が壊れないことを検証する。
#[test]
fn run_frame_reuses_snapshot_when_version_is_unchanged_for_text_input_screen() {
    let mut harness = WidgetHarness::new(text_input_screen(), FormState::default());

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 0);

    harness.run_frame();
    assert_eq!(harness.frames_rendered(), 1);
    assert_eq!(harness.frames_reused(), 1);
}

/// (1) 初回 `run_frame` は `Mounted` を、状態バージョンが変わらない限り以降は
/// 再利用のみで新規フェーズを記録しないことを検証する（`button_widget.rs` の
/// ボタン向けテストと同じ検証軸を、`Node::Widget` を含む画面に対して確認する）。
#[test]
fn text_input_screen_records_declared_and_mounted_phases_on_first_frame() {
    let mut harness = WidgetHarness::new(text_input_screen(), FormState::default());
    harness.run_frame();

    assert_eq!(
        harness.phases(),
        &[
            LifecyclePhase::Declared,
            LifecyclePhase::Mounted,
            LifecyclePhase::Layout,
            LifecyclePhase::Paint
        ]
    );
}

// --- (2) render_headless プレースホルダ契約 ---------------------------------

/// (2) `render_headless` スナップショットに `Node::Widget`（`TextInput`）の行が
/// 現れないことを固定化する（未配線契約、widget.rs / render.rs 参照）。
///
/// `sync_from_state`（`&mut self` 要求）を呼べる経路が `render_headless` に存在しない
/// ため、`paint()` への委譲は行わず安全側（無出力）に倒す契約を、テキストフィールドに
/// ついても回帰検知できるようにする。
#[test]
fn render_headless_snapshot_does_not_contain_text_input_line() {
    let screen = text_input_screen();
    let state = FormState {
        note: "書きかけのメモ".to_string(),
    };

    let snapshot = render_headless(&screen, &state);

    // タイトル行のみで、テキストフィールドの行（ラベル・値のいずれも）は出力されない。
    assert_eq!(snapshot, vec!["# フォーム画面".to_string()]);
}

/// `.text_input()` 混在時も他バリアント（`.text()` 等）の描画・走査に影響しないことを
/// 確認する（既存 4 バリアントの後方互換性、`Node::Widget` 追加時の回帰防止）。
#[test]
fn text_input_does_not_affect_rendering_of_other_node_variants() {
    let screen: Screen<FormState> = Screen::new("混在画面").text("見出し").text_input(
        WidgetId::from_raw(1),
        "メモ",
        get_note,
        set_note,
    );
    let state = FormState {
        note: "値".to_string(),
    };

    let snapshot = render_headless(&screen, &state);

    assert_eq!(
        snapshot,
        vec!["# 混在画面".to_string(), "見出し".to_string()]
    );
}

// --- (3a) TextInput<S>: Widget トレイト直接呼び出し（公開 API 経由の契約固定化） ---

/// `measure` がラベル + 値の文字数近似から寸法を導出し、利用可能領域へクランプすることを、
/// クレート外部（公開 API のみ）から固定化する。
#[test]
fn text_input_measure_approximates_label_and_value_and_clamps_to_available_space() {
    let mut field: TextInput<FormState> =
        TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
    field.sync_from_state(&FormState {
        note: "こんにちは".to_string(),
    });

    let generous = field.measure(sized(1000.0, 1000.0));
    assert!(generous.width > 0.0);
    assert!(generous.height > 0.0);

    let clamped = field.measure(sized(5.0, 5.0));
    assert_eq!(clamped.width, 5.0);
    assert_eq!(clamped.height, 5.0);
}

/// `paint` が `[Rect, Text(label), Text(value)]` の順序・種別・内容そのものを、値が設定済み
/// の場合に返すことを、公開 API 経由で固定化する（`text_input.rs` 内ユニットテスト
/// `paint_returns_rect_label_and_value_when_value_present` と同じ観点をクレート外部の公開
/// API のみで再確認する）。
#[test]
fn text_input_paint_returns_rect_label_and_value_via_public_api() {
    let mut field: TextInput<FormState> =
        TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
    field.sync_from_state(&FormState {
        note: "値".to_string(),
    });

    let commands = field.paint((0.0, 0.0), field.measure(sized(800.0, 600.0)));
    assert_eq!(
        commands,
        vec![
            PaintCommand::Rect,
            PaintCommand::Text("メモ".to_string()),
            PaintCommand::Text("値".to_string()),
        ]
    );
}

/// `hit_test` が `measure` の返す確定寸法全域をカバーすることを、クレート外部から
/// 固定化する（`TextInput<S>` は disabled 切り替えの公開 API を持たないため、S13 時点の
/// 有効状態での包含判定のみをここで確認する。disabled 分岐は `text_input.rs` 内
/// ユニットテスト `hit_test_returns_false_when_disabled_even_if_point_inside_rect` が
/// 固定化済み）。
#[test]
fn text_input_hit_test_covers_measured_size() {
    let mut field: TextInput<FormState> =
        TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
    field.sync_from_state(&FormState {
        note: "値".to_string(),
    });

    let available = sized(800.0, 600.0);
    let measured = field.measure(available);

    assert!(field.hit_test((0.0, 0.0), (0.0, 0.0), measured));
    assert!(field.hit_test(
        (measured.width - 1.0, measured.height - 1.0),
        (0.0, 0.0),
        measured
    ));
    assert!(!field.hit_test((measured.width, measured.height), (0.0, 0.0), measured));
}

/// `accessibility_role()` が [`AccessibilityRole::TextInput`] を返すことを固定化する
/// （`WidgetCatalog` 側の回帰テストが依拠するロール、`catalog.rs` の
/// `text_field_preview_widget_is_backed_by_real_text_input` 参照）。
#[test]
fn text_input_accessibility_role_is_text_input() {
    let field: TextInput<FormState> =
        TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
    assert_eq!(field.accessibility_role(), AccessibilityRole::TextInput);
}

/// `sync_from_state` が状態の変化に追随して `value()` を更新することを、クレート外部から
/// 固定化する。
#[test]
fn text_input_sync_from_state_tracks_state_changes_via_public_api() {
    let mut field: TextInput<FormState> =
        TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

    field.sync_from_state(&FormState {
        note: "一回目".to_string(),
    });
    assert_eq!(field.value(), "一回目");

    field.sync_from_state(&FormState {
        note: "二回目".to_string(),
    });
    assert_eq!(field.value(), "二回目");
}

// --- (3b) TextFieldWidget<S>: 入力・イベント処理（公開 API 経由の契約固定化） -----

/// フォーカス中に文字列を挿入し `Commit` すると `Event::TextFieldChanged` が発火し、
/// `StateStore::dispatch` を経由してレガシー `.text_field(label, get, set)` ブリッジ
/// （`state::StateStore::dispatch_text_field_changed`）経由で状態が更新されることを固定化する
/// （S14 のブリッジ設計の E2E 検証、`text_field.rs` の
/// `state_store_dispatch_applies_commit_event_through_declarative_node` と同じ観点をクレート
/// 外部の公開 API のみで再確認する）。マルチバイト文字列（char 単位カーソル）で panic
/// しないことも併せて確認する。
#[test]
fn dispatching_text_field_changed_event_updates_state_via_legacy_bridge() {
    let screen: Screen<FormState> = Screen::new("form").text_field("メモ", get_note, set_note);
    let mut store = StateStore::new(FormState::default());
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(1), "メモ", "");
    focus(&mut field);
    field.handle_input(TextFieldInput::InsertText("こんにちは".to_string()));

    let event = field
        .handle_input(TextFieldInput::Commit)
        .expect("編集内容が変更されているため Commit は Some を返す");
    let handled = store.dispatch(&screen, &event);

    assert!(handled);
    assert_eq!(store.state().note, "こんにちは");
    assert_eq!(store.version(), 1);
}

/// 制御文字（改行含む）を含む挿入テキストからは制御文字が除去されることを、クレート外部
/// から固定化する（単一行フィールドの意味論・OWASP A03 系の防御的入力検証、
/// `.claude/rules/security.md`）。
#[test]
fn insert_text_strips_control_characters_via_public_api() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(2), "メモ", "");
    focus(&mut field);

    field.handle_input(TextFieldInput::InsertText("a\nb\tc".to_string()));

    assert_eq!(field.buffer(), "abc");
}

/// 絵文字（ZWJ 連結・肌色修飾子を含む grapheme cluster）を挿入 → 削除 → Commit する
/// 代表シナリオを、クレート外部の公開 API のみで固定化する（TASK-8.5-S26 / #207）。
/// `DeleteBackward` が grapheme cluster 単位で作用し、Commit で確定した
/// `Event::TextFieldChanged` の値が破壊されていないことを確認する。
#[test]
fn emoji_insert_delete_commit_end_to_end_via_public_api() {
    let screen: Screen<FormState> = Screen::new("form").text_field("メモ", get_note, set_note);
    let mut store = StateStore::new(FormState::default());
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(20), "メモ", "");
    focus(&mut field);

    // 肌色修飾子つき絵文字（👍🏽 = 2 char・1 cluster）と ZWJ 連結絵文字
    // （👨‍👩‍👧‍👦 = 7 char・1 cluster）を挿入する。
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    field.handle_input(TextFieldInput::InsertText(format!("👍🏽{family}")));
    assert_eq!(field.buffer(), format!("👍🏽{family}"));

    // 末尾の family クラスタを DeleteBackward 1 回で丸ごと削除する。
    field.handle_input(TextFieldInput::DeleteBackward);
    assert_eq!(field.buffer(), "👍🏽");

    let event = field
        .handle_input(TextFieldInput::Commit)
        .expect("編集内容が変更されているため Commit は Some を返す");
    let handled = store.dispatch(&screen, &event);

    assert!(handled);
    assert_eq!(store.state().note, "👍🏽");
}

/// disabled 中・非フォーカス中はすべての編集入力が無視されることを、クレート外部から
/// 固定化する。
#[test]
fn handle_input_ignored_when_disabled_or_not_focused() {
    let mut not_focused: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(3), "メモ", "初期値");
    let event = not_focused.handle_input(TextFieldInput::InsertText("追加".to_string()));
    assert_eq!(event, None);
    assert_eq!(not_focused.buffer(), "初期値");

    let mut disabled: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(4), "メモ", "初期値");
    focus(&mut disabled);
    disabled.set_disabled(true);
    let event = disabled.handle_input(TextFieldInput::InsertText("追加".to_string()));
    assert_eq!(event, None);
    assert_eq!(disabled.buffer(), "初期値");
}

/// フォーカス喪失時、ダーティな編集があれば blur-commit として
/// `Event::TextFieldChanged` を発火することを、クレート外部から固定化する。
#[test]
fn focus_lost_commits_dirty_buffer_via_public_api() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(5), "メモ", "");
    focus(&mut field);
    field.handle_input(TextFieldInput::InsertText("下書き".to_string()));

    let event = field.apply_input(InteractionInput::FocusLost);

    assert_eq!(
        event,
        Some(Event::TextFieldChanged {
            label: "メモ".to_string(),
            value: "下書き".to_string(),
        })
    );
}

/// `Cancel` は編集内容を確定値へ巻き戻すことを、クレート外部から固定化する。
#[test]
fn cancel_reverts_buffer_to_committed_value_via_public_api() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(6), "メモ", "確定値");
    focus(&mut field);
    field.handle_input(TextFieldInput::InsertText("編集中".to_string()));
    assert_eq!(field.buffer(), "確定値編集中");

    field.handle_input(TextFieldInput::Cancel);

    assert_eq!(field.buffer(), "確定値");
}

/// `Debug` 出力が編集バッファ・確定値の内容を含まないことを、クレート外部から固定化する
/// （機微情報の露出防止、`.claude/rules/security.md`「ログにシークレットを出力しない」の
/// 回帰防止。`text_field.rs` の `debug_output_does_not_expose_buffer_content` と同じ観点を
/// クレート外部の公開 API のみで再確認する）。
#[test]
fn text_field_widget_debug_output_does_not_expose_buffer_content_via_public_api() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(7), "メモ", "秘密の値");
    focus(&mut field);

    let output = format!("{field:?}");

    assert!(!output.contains("秘密の値"));
}

/// `TextInput<S>` の `Debug` 出力も同様に値の内容を含まないことを固定化する
/// （`text_input.rs` の `debug_output_does_not_contain_cached_value_content` と同じ観点）。
#[test]
fn text_input_debug_output_does_not_expose_cached_value_via_public_api() {
    let mut field: TextInput<FormState> =
        TextInput::bound(WidgetId::from_raw(8), "メモ", get_note, set_note);
    field.sync_from_state(&FormState {
        note: "秘密の値".to_string(),
    });

    let output = format!("{field:?}");

    assert!(!output.contains("秘密の値"));
}

// --- (3c) IME 統合配線（TASK-8.5-S8 / REQ-8、Issue #189） -------------------

/// フォーカス → 挿入 → Preedit（未確定文字列）→ Commit（確定）→ blur という
/// IME コンポジションの一連の流れが単一フィールドで一気通貫に動き、`StateStore::dispatch`
/// を経由して確定値**のみ**（preedit 混入なし）が `S` へ反映されることを、クレート外部の
/// 公開 API のみで固定化する（`docs/architecture/text-input-state-model.md` §4.3〜§4.6 の
/// E2E 検証、`text_field.rs` 内ユニットテストと重複する軸は代表ケースに絞る）。
#[test]
fn ime_composition_lifecycle_reaches_state_store_via_public_api() {
    let screen: Screen<FormState> = Screen::new("form").text_field("メモ", get_note, set_note);
    let mut store = StateStore::new(FormState::default());
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(9), "メモ", "");

    focus(&mut field);
    assert_eq!(
        field.take_ime_requests(),
        vec![
            ImeRequest::SetPurpose(ImePurpose::Normal),
            ImeRequest::SetEnabled(true),
        ],
        "フォーカス獲得で入力目的ヒントと IME 有効化を要求する（S23 / #204）"
    );

    field.handle_input(TextFieldInput::InsertText("こんにちは".to_string()));

    // IME が変換候補を確定する前の未確定文字列（かな漢字変換途中）。
    field.apply_ime(ImeEvent::Preedit {
        text: "せかい".to_string(),
        cursor: Some((0, 3)),
    });
    assert_eq!(
        field.buffer(),
        "こんにちは",
        "preedit は確定バッファに混入しない"
    );

    // 変換確定。
    field.apply_ime(ImeEvent::Commit("世界".to_string()));
    assert_eq!(field.buffer(), "こんにちは世界");

    let event = field.apply_input(InteractionInput::FocusLost);
    assert_eq!(
        field.take_ime_requests(),
        vec![ImeRequest::SetEnabled(false)],
        "フォーカス喪失で IME 無効化を要求する"
    );

    let event = event.expect("確定挿入により dirty のため blur-commit が発火する");
    let handled = store.dispatch(&screen, &event);

    assert!(handled);
    assert_eq!(
        store.state().note,
        "こんにちは世界",
        "S へは確定値のみが反映される"
    );
    assert_eq!(store.version(), 1);
}

// --- (4) カタログ経由の到達性 ------------------------------------------------

/// `WidgetCatalog::with_builtins()` から `text_field` エントリが取得でき、
/// `category == Input`・preview が panic せずタイトル行を返すことを確認する
/// （TASK-8.3-S15 / Issue #110、カタログ登録の受け入れ条件）。
#[test]
fn text_field_entry_is_registered_in_input_category_with_non_empty_preview() {
    let catalog = WidgetCatalog::with_builtins();

    let entry = catalog
        .find("text_field")
        .expect("text_field エントリが登録されているべき");
    assert_eq!(entry.category, CatalogCategory::Input);

    let lines = (entry.preview)();
    assert_eq!(lines, vec!["# TextField プレビュー".to_string()]);

    let input_entries = catalog.entries_in(CatalogCategory::Input);
    assert!(input_entries.iter().any(|e| e.name == "text_field"));
}

/// `Screen::text_input()`（`catalog.rs` の `text_field_preview_screen` が内部で用いるものと
/// 同じ公開ビルダー）が `Node::Widget` を 1 件だけ生成し、`TextInput<S>` として走査できる
/// ことを、クレート外部から確認する。
///
/// 注意: 本テストは `WidgetCatalog` を一切参照しない（ビルダー単体の契約固定化）。
/// カタログの `text_field` エントリ自体が実際にこのビルダー経由で構築されていることは
/// `catalog.rs`（クレート内部視点）の `text_field_preview_widget_is_backed_by_real_text_input`
/// が固定化しており、両者が揃うことでカタログ経由の到達性が二重に保証される。
#[test]
fn text_input_builder_produces_exactly_one_widget_node() {
    let screen: Screen<FormState> = Screen::new("TextField プレビュー").text_input(
        WidgetId::from_raw(1),
        "ラベル",
        get_note,
        set_note,
    );

    let mut widget_count = 0;
    screen.visit(|node| {
        if let Node::Widget(widget) = node {
            widget_count += 1;
            assert_eq!(widget.accessibility_role(), AccessibilityRole::TextInput);
        }
    });

    assert_eq!(widget_count, 1);
}
