//! Web（DOM `compositionevent`）経由の日本語入力シナリオ再現テスト
//! （TASK-8.5-S21 / REQ-8 / M3、Issue #202、親 #181）。
//!
//! `fandhe-winit-ime/tests/linux_japanese_input.rs` 等（S9・S12・S14）が確立した
//! 「確定済みイベント列を変換関数 → `TextFieldWidget::apply_ime` へ流す」検証方式を、
//! Web の `compositionevent`（`fandhe_web_dom::ime::ime_event_from_composition`）へ
//! 適用したもの。`ime_event_from_composition` は `web-sys::CompositionEvent`
//! （`wasm32` 専用型）に依存しないターゲット非依存の変換関数のため、本ファイルは
//! native（`cargo test`、ホスト環境）で実行できる（`ime.rs` モジュール doc 参照）。
//!
//! `docs/architecture/ime-architecture.md` §4.5「Web（`compositionevent`、S21〜S22）」の
//! 対応表と対になる（本ファイルはその対応表を実行可能な形で固定化する）。
//!
//! # 本ファイルが確認する契約
//!
//! - 日本語入力の標準列（start → update × n → end）で `buffer` へ原子的に挿入され、
//!   composition が正しくクリアされること
//! - キャンセル列（start → update → end("")）で `buffer` が不変のまま composition が
//!   クリアされること（IME キャンセル時、`compositionend.data` が空文字列になりうる、
//!   `ime.rs` モジュール doc「ブラウザ差異」参照）
//! - `cursor: None` の `Preedit` 列（DOM はカーソル範囲を公開しない、§4.6）を既存の
//!   状態モデルがそのまま許容すること（Web 固有制約の固定化）
//! - buffer 中間位置（anchor）での合成挿入
//! - 非フォーカス中に届いた `Preedit`/`Commit` が破棄されること（フォーカスガード、
//!   `text_field.rs` の既存契約）
//! - `compositionend.data` に含まれる制御文字が `apply_ime` の Commit 契約により
//!   サニタイズされること
//! - `compositionend` 後に後続 `input`（`isComposing:false`）が届いても、`mount.rs` の
//!   配線は同一値の再代入・再描画に留まり、二重適用（buffer への二重挿入）にはならない
//!   ことの説明的確認。ただし `apply_ime(Commit)` 自体は「合成断片の挿入」であり、
//!   同一フラグメントを 2 回連続適用すれば意図どおり buffer へ再挿入される
//!   （冪等ではない、Bugbot レビュー #456 指摘）。この二重適用回避は `TextFieldWidget`
//!   側の性質ではなく、`mount.rs` が `compositionend` と後続 `input` のいずれも
//!   `apply_ime` を経由させず `input.value()`（全文）で置換する配線設計に依存する。
//!   本ファイルでは「`apply_ime` の `Commit` は非冪等である」ことを実際に確認し、
//!   だからこそ `mount.rs` が全文置換方式を選んでいる、という契約を固定化する

use fandhe_core::{Event, ImeEvent, InteractionInput, TextFieldWidget, WidgetId};
use fandhe_web_dom::ime::{composition_phase_from_event_type, ime_event_from_composition};

/// `TextFieldWidget` をフォーカス状態にする（`fandhe-winit-ime` の各シナリオテストと
/// 同一操作を公開 API 経由で行う）。
fn focused_field(initial: &str) -> TextFieldWidget<()> {
    let mut field = TextFieldWidget::new_static(WidgetId::from_raw(1), "メモ", initial);
    field.apply_input(InteractionInput::FocusGained);
    field
}

/// DOM イベント型名 + `data` を `ime_event_from_composition` 経由でウィジェットへ適用する
/// テストヘルパー（本ファイル全体で使う「DOM → コア」経路の共通処理）。
/// 実配線（`mount.rs`）が経由するのと同じ 2 段変換（型名解釈 → `ImeEvent` 変換）を
/// そのまま再現する。
fn apply(field: &mut TextFieldWidget<()>, event_type: &str, data: &str) -> Option<Event> {
    let phase =
        composition_phase_from_event_type(event_type).expect("テスト対象のイベント型名は既知");
    field.apply_ime(ime_event_from_composition(phase, data))
}

// --- シナリオ 1: 標準的な日本語入力（かんじ → 漢字） ---------------------------

/// `compositionstart` → `compositionupdate` × n（「か」→「かん」→「漢字」）→
/// `compositionend`("漢字") で buffer へ原子的に挿入され、composition がクリアされる
/// ことを確認する（§4.5 対応表の標準列）。
#[test]
fn standard_japanese_input_commits_atomically_and_clears_composition() {
    let mut field = focused_field("");

    assert_eq!(apply(&mut field, "compositionstart", ""), None);

    for partial in ["か", "かん", "漢字"] {
        apply(&mut field, "compositionupdate", partial);
        assert_eq!(field.buffer(), "", "確定前は buffer へ反映しない（§4.3）");
    }

    let event = apply(&mut field, "compositionend", "漢字");

    assert_eq!(field.buffer(), "漢字");
    assert_eq!(field.cursor(), 2);
    assert!(
        event.is_none(),
        "Commit 自体は TextFieldChanged を発火しない（blur-commit のみが発火源）"
    );
}

// --- シナリオ 2: 変換キャンセル（Esc 相当） -------------------------------------

/// `compositionstart` → `compositionupdate` → `compositionend`("")（Esc キャンセル
/// 相当、`ime.rs` モジュール doc「ブラウザ差異」参照）で buffer が不変のまま
/// composition がクリアされることを確認する。
#[test]
fn cancelled_composition_leaves_buffer_unchanged() {
    let mut field = focused_field("既存値");

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "にほんご");
    let event = apply(&mut field, "compositionend", "");

    assert_eq!(field.buffer(), "既存値", "キャンセルで buffer は変化しない");
    assert!(event.is_none());
}

// --- シナリオ 3: cursor: None の Preedit 列（Web 固有制約） ----------------------

/// DOM の compositionevent はカーソル範囲を公開しないため、`ime_event_from_composition`
/// が返す `Preedit.cursor` は常に `None` になる（§4.6）。この Web 固有の制約を
/// 既存の状態モデル（`TextFieldWidget`）がそのまま許容し、確定時には正しく
/// `buffer` へ反映されることを固定化する。
#[test]
fn preedit_with_none_cursor_is_accepted_by_existing_state_model() {
    let mut field = focused_field("");
    apply(&mut field, "compositionstart", "");

    let phase = composition_phase_from_event_type("compositionupdate").unwrap();
    let ime_event = ime_event_from_composition(phase, "てすと");
    assert_eq!(
        ime_event,
        ImeEvent::Preedit {
            text: "てすと".to_string(),
            cursor: None,
        },
        "Web の compositionupdate はカーソル範囲を公開しないため cursor は常に None"
    );
    field.apply_ime(ime_event);

    apply(&mut field, "compositionend", "テスト");
    assert_eq!(field.buffer(), "テスト");
}

// --- シナリオ 4: buffer 中間位置（anchor）での合成挿入 --------------------------

/// カーソルを buffer 中間へ移動した状態で合成挿入すると、その位置（anchor）を
/// 起点に確定文字列が挿入されることを確認する（`text_field.rs` の anchor 契約）。
#[test]
fn composition_inserts_at_anchor_in_middle_of_buffer() {
    use fandhe_core::TextFieldInput;

    let mut field = focused_field("あいう");
    field.handle_input(TextFieldInput::MoveCursorToStart);
    field.handle_input(TextFieldInput::MoveCursorRight);

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "え");
    apply(&mut field, "compositionend", "え");

    assert_eq!(field.buffer(), "あえいう");
    assert_eq!(field.cursor(), 2);
}

// --- シナリオ 5: 非フォーカス中の Preedit/Commit は破棄される -------------------

/// フォーカスガード（`text-input-state-model.md` §4.3・不変条件 8）: 非フォーカス中に
/// 届いた `Preedit`/`Commit` は buffer/composition を変化させずに破棄される。Web
/// では起こりにくいケース（フォーカスがない要素で compositionevent が発火することは
/// 通常ない）だが、コア側契約がプラットフォームによらず成立することを確認する。
#[test]
fn preedit_and_commit_are_discarded_while_unfocused() {
    let mut field: TextFieldWidget<()> =
        TextFieldWidget::new_static(WidgetId::from_raw(2), "メモ", "初期値");
    // focused_field と異なりフォーカスを与えない。

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "むし");
    apply(&mut field, "compositionend", "むし");

    assert_eq!(
        field.buffer(),
        "初期値",
        "非フォーカス中の Preedit/Commit は破棄される"
    );
}

// --- シナリオ 6: 制御文字を含む compositionend.data のサニタイズ -----------------

/// `compositionend.data` に制御文字が混入していても、`apply_ime` の Commit 契約
/// （既存 `TextFieldInput::InsertText` 系と同じサニタイズ）により除去されることを
/// 確認する（本モジュール `ime_event_from_composition` 自体は文字列を一切加工しない
/// ため、この防御はコア側契約の継承であることの確認）。
#[test]
fn control_characters_in_commit_data_are_sanitized_by_core_contract() {
    let mut field = focused_field("");
    apply(&mut field, "compositionstart", "");

    // NUL 文字混入（外部入力として扱う想定、OWASP A03 対策の継承確認）。
    apply(&mut field, "compositionend", "あ\u{0}い");

    assert!(
        !field.buffer().contains('\u{0}'),
        "制御文字はコア側 Commit 契約でサニタイズされる"
    );
}

// --- シナリオ 7: apply_ime(Commit) は非冪等であり、mount.rs は全文置換でこれを回避する --

/// `TextFieldWidget::apply_ime(ImeEvent::Commit(_))` は「合成断片の挿入」であり
/// 冪等ではない。同一フラグメント（"ねこ"）を compositionstart → compositionupdate →
/// compositionend の正規サイクルで 2 回連続適用すると、2 回目も
/// `composition.anchor`（1 回目の確定後カーソル位置）へ再挿入され、buffer は
/// "ねこねこ" になる（Bugbot レビュー #456 指摘: 旧テストは空 Commit（キャンセル）を
/// 使っており、この非冪等性を検証できていなかった）。
///
/// この実際の二重挿入があるからこそ、`mount.rs`（`attach_input`）は
/// `compositionend` およびその後続 `input`（isComposing:false）のどちらでも
/// `apply_ime` へ合成断片を渡さず、`input.value()`（全文）を読み直して
/// `input_handlers`（`Node::TextField::set`、全文置換）へ渡す配線にしている
/// （`mount.rs` モジュール doc §"attach_input" 参照）。本テストは
/// `TextFieldWidget` 単体の契約（非冪等）を固定化し、`mount.rs` 側の全文置換方式が
/// 必須の防御であることを裏付ける。
#[test]
fn repeated_commit_of_same_fragment_via_apply_ime_inserts_again() {
    let mut field = focused_field("");
    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "ねこ");
    let first = apply(&mut field, "compositionend", "ねこ");
    assert_eq!(field.buffer(), "ねこ");
    assert!(first.is_none());

    // 同一フラグメントの Commit をもう一度、正規の compositionstart〜end サイクルで
    // 適用する（cancel ではなく実際の Commit("ねこ")）。
    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "ねこ");
    apply(&mut field, "compositionend", "ねこ");

    assert_eq!(
        field.buffer(),
        "ねこねこ",
        "apply_ime(Commit) は非冪等であり、同一フラグメントの再適用は buffer へ再挿入される \
         （mount.rs 側の全文置換方式がこの二重挿入を回避する実際の防御である）"
    );
}
