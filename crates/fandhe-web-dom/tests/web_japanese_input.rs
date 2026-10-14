//! Web（DOM `compositionevent`）経由の日本語入力の追加シナリオ再現テスト
//! （TASK-8.5-S22 / REQ-8 / M3、Issue #203、親 #181）。
//!
//! `tests/web_composition_ime.rs`（S21・#202）が固定化した 7 シナリオ（標準入力・
//! キャンセル・`cursor: None`・anchor 挿入・フォーカスガード・制御文字サニタイズ・
//! 非冪等性の説明的確認）に対し、本ファイルは S21 の検証記録
//! （実ブラウザチェックリスト）が申し送った項目（Issue #203）のうち、**シーケンス再現
//! 結合テストで固定化できる観点**（実ブラウザ操作を伴わないもの）を追加する
//! （層 B）。
//!
//! `web_composition_ime.rs` と同じヘルパー様式（`focused_field` / `apply`）を踏襲する。

use fandhe_core::{InteractionInput, TextFieldWidget, WidgetId};
use fandhe_web_dom::ime::{composition_phase_from_event_type, ime_event_from_composition};

/// `TextFieldWidget` をフォーカス状態にする（`web_composition_ime.rs` と同一操作）。
fn focused_field(initial: &str) -> TextFieldWidget<()> {
    let mut field = TextFieldWidget::new_static(WidgetId::from_raw(1), "メモ", initial);
    field.apply_input(InteractionInput::FocusGained);
    field
}

/// DOM イベント型名 + `data` を `ime_event_from_composition` 経由で適用する
/// （`web_composition_ime.rs` の `apply` と同一の 2 段変換）。
fn apply(field: &mut TextFieldWidget<()>, event_type: &str, data: &str) {
    let phase =
        composition_phase_from_event_type(event_type).expect("テスト対象のイベント型名は既知");
    field.apply_ime(ime_event_from_composition(phase, data));
}

// --- シナリオ 1: 文節ごとの連続確定 ---------------------------------------------

/// 「わたしは」確定直後に新規コンポジション「がくせい」を開始・確定する、
/// 文節区切りでの連続入力（日本語入力の典型パターン）が buffer へ順に
/// 連結されることを確認する。
#[test]
fn sequential_bunsetsu_commits_are_appended_in_order() {
    let mut field = focused_field("");

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "わたしは");
    apply(&mut field, "compositionend", "わたしは");
    assert_eq!(field.buffer(), "わたしは");

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "がくせい");
    apply(&mut field, "compositionend", "がくせい");
    assert_eq!(field.buffer(), "わたしはがくせい");
    assert_eq!(field.cursor(), 8);
}

// --- シナリオ 2: preedit 途中の縮退（変換候補の後退） ---------------------------

/// 変換候補選択中に文字が縮退する列（「かんじ」→「かん」→「か」、Backspace や
/// 候補戻しに相当）を経ても、`compositionupdate` は buffer を変化させず、最終
/// `compositionend` の `data` のみが確定に反映されることを確認する（§4.3、
/// 確定前は buffer へ反映しない契約の維持）。
#[test]
fn preedit_shrinking_sequence_does_not_affect_buffer_until_commit() {
    let mut field = focused_field("");

    apply(&mut field, "compositionstart", "");
    for partial in ["かんじ", "かん", "か"] {
        apply(&mut field, "compositionupdate", partial);
        assert_eq!(field.buffer(), "", "縮退列の途中では buffer は不変");
    }
    apply(&mut field, "compositionend", "火");

    assert_eq!(field.buffer(), "火");
}

// --- シナリオ 3: composition 中に FocusLost へ到達する ---------------------------

/// コンポジション中（`compositionstart` 後・`compositionend` 前）にフォーカスを
/// 喪失した場合、blur 時にブラウザが `compositionend` を発火する契約
/// （`ime.rs` モジュール doc「ブラウザ差異」・`s21-web-composition-ime.md` §5
/// チェックリスト項目「フォーカス移動時の `compositionend` 発火」）のうち、コア側
/// （`TextFieldWidget::apply_input(FocusLost)`）が受け持つ契約を固定化する:
/// `FocusLost` はコンポジションを確定せずローカルに破棄してから blur-commit を行う
/// （`text_field.rs` モジュール doc 参照）ため、preedit 文字列は buffer へ混入しない。
/// 実ブラウザが実際に `compositionend` を発火するかどうかの確認自体は本テストの
/// スコープ外（人間担当チェックリストへ残置、検証記録 §6 参照）。
#[test]
fn focus_lost_during_composition_discards_preedit_without_committing() {
    let mut field = focused_field("既存値");

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "みちなか");
    // compositionend を経由せず、直接 FocusLost に到達する
    // （ブラウザが compositionend を発火しなかった場合の防御的確認）。
    let event = field.apply_input(InteractionInput::FocusLost);

    assert_eq!(
        field.buffer(),
        "既存値",
        "コンポジション中の preedit は FocusLost で破棄され、buffer へ混入しない"
    );
    assert!(
        event.is_none(),
        "buffer が変化していないため TextFieldChanged は発火しない"
    );
}

// --- シナリオ 4: サロゲートペア・絵文字を含む compositionend.data ---------------

/// `compositionend.data` に絵文字（サロゲートペア、UTF-16 では 2 code unit）が
/// 含まれる場合でも、char 単位で管理される `buffer`/`cursor`（§4.6 文字インデックス
/// 正規化方針）が破綻しないことを確認する（Web は `&str`/`String` を経由するため
/// Rust 側では既に UTF-8 として正規化済みだが、`cursor` が「絵文字を 1 文字として
/// 数える」char 単位契約であることを固定化する）。
#[test]
fn commit_with_emoji_surrogate_pair_advances_cursor_by_one_char() {
    let mut field = focused_field("");

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "😀");
    apply(&mut field, "compositionend", "😀");

    assert_eq!(field.buffer(), "😀");
    assert_eq!(
        field.cursor(),
        1,
        "絵文字（サロゲートペア）は char 単位で 1 文字として数える"
    );
}

// --- シナリオ 5: 全角英数の確定 --------------------------------------------------

/// 全角英数（"ＡＢＣ１２３"）の変換確定が、半角文字列と同様に char 単位でそのまま
/// buffer へ挿入されることを確認する（全角/半角の正規化は本設計のスコープ外
/// （`ime-architecture.md` に正規化契約の記載はない）であり、`data` を加工せず
/// 素通しする既存方針の継続確認）。
#[test]
fn fullwidth_alphanumeric_commit_is_inserted_verbatim() {
    let mut field = focused_field("");

    apply(&mut field, "compositionstart", "");
    apply(&mut field, "compositionupdate", "ＡＢＣ");
    apply(&mut field, "compositionend", "ＡＢＣ１２３");

    assert_eq!(field.buffer(), "ＡＢＣ１２３");
    assert_eq!(field.cursor(), 6);
}

// --- シナリオ 6: compositionstart を伴わない compositionupdate（防御的順序異常） --

/// `compositionstart` を経由せずに `compositionupdate`/`compositionend` が届く
/// 順序異常（ブラウザの実装差異・イベント取りこぼしを想定した防御的シナリオ、
/// S21 §5 チェックリスト項目「ブラウザ別のイベント発火差異」の一部を先取りして
/// コア側契約として固定化）でも、`apply_ime` が panic せず、`compositionend` の
/// `data` がそのまま確定挿入されることを確認する（`ime_event_from_composition` は
/// フェーズを都度独立に解釈するステートレス関数であり、直前の呼び出し履歴に
/// 依存しないため、この順序異常でも安全に動作する設計であることの確認）。
#[test]
fn compositionupdate_without_preceding_compositionstart_does_not_panic() {
    let mut field = focused_field("");

    // compositionstart を送らず、いきなり compositionupdate から開始する。
    apply(&mut field, "compositionupdate", "ちゅうい");
    assert_eq!(field.buffer(), "", "確定前は buffer 不変");

    apply(&mut field, "compositionend", "注意");

    assert_eq!(
        field.buffer(),
        "注意",
        "compositionstart 欠落時でも compositionend の data はそのまま確定挿入される"
    );
}
