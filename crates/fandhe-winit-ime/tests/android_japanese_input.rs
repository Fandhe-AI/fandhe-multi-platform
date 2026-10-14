//! Android 日本語入力動作検証のシーケンス再現結合テスト
//! （TASK-8.5-S20 / REQ-8 / M3、Issue #201、親 #181）。
//!
//! S19（#200、`tests/android_software_keyboard_input.rs`）が確定させた「Android
//! （NativeActivity 方式）では `Ime::*` が一切発火せず、`InputEvent::KeyEvent` 由来の
//! per-key `KeyboardInput` のみが `text_field_input_from_key` を経由して
//! `TextFieldInput::InsertText`/`DeleteBackward` へ変換される」契約を土台に、S19 の
//! 7 テストが扱っていない次の観点を追加で固定化する。
//!
//! - `text_field_input_from_key` の契約そのもの（`text: Option<&str>` を分割せず丸ごと
//!   1 つの `InsertText` へ変換する）を、複数文字列（「日本語」）を渡すケースで直接固定化
//!   する（S19 のテストは `insert_char_via_android_key_event` ヘルパーで 1 文字ずつ個別の
//!   `KeyboardInput` を模しており、複数文字を渡す経路は未検証だった）。**ただし
//!   winit 0.30.13 の Android バックエンドの現行実装は `character_map_and_combine_key`
//!   （JNI `KeyCharacterMap` 照会 + dead-key 結合）が常に結合済みの 1 文字を `text` として
//!   返す（`s19-android-software-keyboard-ime.md` §2 事実 3）ため、この経路（複数文字が
//!   1 回の `KeyEvent` にまとまって届く）は**現行の winit 実装では発生しない**。
//!   iOS（S17・#198）が「候補バー確定文字列が 1 回の `insertText:` にまとまって届く」
//!   経路について winit の現行実装が常に 1 文字ずつ分解して送出することを確認したうえで
//!   将来の実装変更に備えた回帰ガードとして維持した（`s17-ios-japanese-input.md` §7）のと
//!   同じ位置づけで、本シナリオも将来の winit 変更・GameActivity 移行時
//!   （`ime-android-input-connection.md` §4.1、既存 issue #241・TASK-9.3）の回帰ガードとして
//!   維持する。
//! - 複数文字を原子的に挿入した直後の `deleteBackward`（`KEYCODE_DEL`）が、挿入ブロック
//!   単位ではなく文字単位で末尾 1 文字のみを削除すること。
//!
//! Android には（NativeActivity 方式では）そもそも preedit 経路が存在しないため、
//! 「フォーカス喪失時に未確定文字列を破棄しない」観点は S19 テスト 1 が既に
//! blur-commit 契約として検証済みであり、本ファイルでは重複させない。
//!
//! 実行環境は Linux（Android 実機/エミュレータは検証環境に無い）であるため、
//! 本ファイルは**ソース照合済みの契約のシミュレーション**であり、実機（日本語
//! ソフトウェアキーボードでの実際のかな漢字変換・候補確定等）の確認は
//! 実機チェックリスト（人間担当）へ委ねる。**Android のインライン preedit（`setComposingText` の反映）は
//! winit 0.30 が `InputEvent::TextEvent` を無視するため実現不可能であり、
//! 本ファイルはその前提を検証対象とはしない**（未対応の有無ではなく、経路が存在しない
//! ことそのものが S19/S20 の確定事実である）。

use fandhe_core::{Event, InteractionInput, TextFieldWidget, WidgetId};
use fandhe_winit_ime::text_field_input_from_key;
use winit::event::ElementState;
use winit::keyboard::{Key, NamedKey};

/// テスト対象フィールドを構築し、フォーカスを与える
/// （`android_software_keyboard_input.rs` の `focused_field` と同じ役割。結合テストは
/// 別クレートとしてコンパイルされるためファイル間で共有できず、同じ最小実装を
/// ここに独立して用意する）。
fn focused_field() -> TextFieldWidget<()> {
    let mut field: TextFieldWidget<()> =
        TextFieldWidget::new_static(WidgetId::from_raw(1), "note", "");
    field.apply_input(InteractionInput::FocusGained);
    field.take_ime_requests();
    field
}

// --- 1. text_field_input_from_key の複数文字非分割契約（回帰ガード） ------------------

/// `text_field_input_from_key` は `text: Option<&str>` を丸ごと 1 つの
/// `TextFieldInput::InsertText` へ変換する契約（`src/lib.rs`）を、複数文字列
/// （「日本語」）を渡すケースで直接固定化する。S19 のシミュレーションは 1 文字ずつの
/// `KeyEvent` のみを模していたため、複数文字を渡す経路は未検証だった。
///
/// **現行 winit 実装での発生可否**: `character_map_and_combine_key`（JNI
/// `KeyCharacterMap` 照会 + dead-key 結合、s19 検証記録 §2 事実 3）は常に結合済みの
/// 1 文字を `text` として返すため、この経路（複数文字が 1 回の `KeyEvent` にまとまって
/// 届く）は現行の winit 0.30.13 実装では発生しない。iOS（S17）が同種の観点（候補バー
/// 確定文字列が 1 回の `insertText:` にまとまって届く経路）について winit の現行実装は
/// 常に 1 文字ずつ分解して送出することを確認したうえで将来の実装変更への回帰ガードとして
/// 維持した（`s17-ios-japanese-input.md` §7）のと同じ位置づけで、本シナリオも将来の
/// winit 変更・GameActivity 移行（`ime-android-input-connection.md` §4.1）時の
/// 回帰ガードとして維持する。
#[test]
fn multi_char_key_event_from_dead_key_combination_is_inserted_atomically() {
    let mut field = focused_field();

    // 複数文字「日本語」が 1 回の KeyEvent（1 つの Pressed KeyboardInput）として届いた
    // 場合を模す（現行 winit 実装では発生しない、上記コメント参照）。
    let pressed = text_field_input_from_key(
        ElementState::Pressed,
        &Key::Character("日本語".into()),
        Some("日本語"),
    );
    assert_eq!(
        pressed,
        Some(fandhe_core::TextFieldInput::InsertText(
            "日本語".to_string()
        )),
        "1 回の KeyEvent は文字列全体を 1 つの InsertText へ変換する（分割しない）"
    );
    let event = pressed.and_then(|input| field.handle_input(input));
    assert_eq!(
        event, None,
        "InsertText は Commit イベントを返さない契約（buffer 反映のみ、S19 と同型）"
    );
    assert_eq!(field.buffer(), "日本語");

    // Released 側（KeyAction::Up 相当）は無視される（二重挿入防止、S19 §4 対応表と同じ
    // 回帰ガード）。
    let released = text_field_input_from_key(
        ElementState::Released,
        &Key::Character("日本語".into()),
        None,
    );
    assert_eq!(released, None);
    assert_eq!(
        field.buffer(),
        "日本語",
        "Released 側の処理で buffer が変化しないこと"
    );
}

// --- 2. 原子的挿入直後の KEYCODE_DEL は文字単位で末尾 1 文字のみ削除する -----------

/// 複数文字を 1 回で挿入した直後に `KEYCODE_DEL`（`NamedKey::Backspace`）が届いても、
/// 挿入ブロック全体ではなく文字単位（`char` 境界）で末尾 1 文字のみが削除されることを
/// 確認する（`fandhe-core` 側の `TextFieldInput::DeleteBackward` 契約への委譲を
/// Android 経路でも再確認する回帰ガード）。
#[test]
fn keycode_del_after_atomic_multi_char_insert_removes_only_last_character() {
    let mut field = focused_field();

    let pressed = text_field_input_from_key(
        ElementState::Pressed,
        &Key::Character("日本語".into()),
        Some("日本語"),
    )
    .expect("複数文字の KeyEvent は InsertText へ変換される");
    field.handle_input(pressed);
    assert_eq!(field.buffer(), "日本語");

    let backspace = Key::Named(NamedKey::Backspace);
    let delete = text_field_input_from_key(ElementState::Pressed, &backspace, None)
        .expect("Pressed の KEYCODE_DEL は DeleteBackward に変換される");
    field.handle_input(delete);

    assert_eq!(
        field.buffer(),
        "日本",
        "挿入ブロック全体ではなく末尾 1 文字（「語」）のみが削除される"
    );

    // Released 側（KeyAction::Up 相当）は二重削除にならない。
    let released = text_field_input_from_key(ElementState::Released, &backspace, None);
    assert_eq!(released, None);
    assert_eq!(field.buffer(), "日本");
}

// --- 3. blur-commit 契約の回帰確認（preedit が存在しないため破棄対象がないこと） ------

/// Android（NativeActivity 方式）には preedit（未確定合成）が存在しないため（S19 §2
/// 事実 1・4）、フォーカス喪失時に破棄されるべき未確定入力が存在せず、`InsertText` で
/// 逐次確定済みの内容がそのまま blur-commit される。S19 テスト 1 は 1 文字入力後の
/// blur-commit を確認済みだが、本テストは複数文字の原子的挿入後の blur-commit を
/// 追加確認し、S19 の観点との重複を避けつつ本ファイルのシナリオ 1 と接続する。
#[test]
fn atomic_multi_char_insert_survives_focus_loss_without_discarding_because_no_composition_exists() {
    let mut field = focused_field();

    let pressed = text_field_input_from_key(
        ElementState::Pressed,
        &Key::Character("かんじへんかん".into()),
        Some("かんじへんかん"),
    )
    .expect("複数文字の KeyEvent は InsertText へ変換される");
    field.handle_input(pressed);
    assert_eq!(field.buffer(), "かんじへんかん");

    // フォーカス喪失: Android 経路では composition が存在しないため、破棄される内容は
    // なく入力済みの全文字列がそのまま blur-commit される。
    let event = field.apply_input(InteractionInput::FocusLost);
    assert_eq!(
        event,
        Some(Event::TextFieldChanged {
            label: "note".to_string(),
            value: "かんじへんかん".to_string(),
        }),
        "Android 経路には未確定合成（composition）が存在しないため、原子的に挿入済みの \
         文字列も破棄されずそのまま blur-commit される"
    );
    assert_eq!(
        field.buffer(),
        "かんじへんかん",
        "blur-commit 後も buffer 自体は変化しない"
    );
}
