//! iOS 日本語入力動作検証のシーケンス再現結合テスト
//! （TASK-8.5-S17 / REQ-8 / M3、Issue #198、親 #181）。
//!
//! S16（#197、`tests/ios_software_keyboard_input.rs`）が確立した「iOS では `Ime::*` が
//! 一切発火せず、`insertText:`/`deleteBackward` 由来の per-char `KeyboardInput` のみが
//! `text_field_input_from_key` を経由して `TextFieldInput::InsertText`/`DeleteBackward`
//! へ変換される」契約を土台に、S16 の 6 テストが扱っていない次の観点を追加で固定化する。
//!
//! - かな漢字変換で確定した複数文字列が **1 回の `insertText:` 呼び出し**（1 つの
//!   `KeyboardInput`）にまとまって届いた場合の原子的挿入（候補バー確定の典型パターン。
//!   S16 のテストは 1 文字ずつ個別の `insertText_char` 呼び出しを模しており、この
//!   「複数文字が 1 回で届く」経路は未検証だった）。
//! - 複数文字を原子的に挿入した直後の `deleteBackward` が、挿入ブロック単位ではなく
//!   文字単位で末尾 1 文字のみを削除すること。
//! - iOS には preedit（未確定合成）が存在しないため、フォーカス喪失（`resignFirstResponder`
//!   相当）が発生しても **破棄されるべき未確定入力が存在せず**、`InsertText`/`DeleteBackward`
//!   で確定済みの内容がそのまま blur-commit されること（macOS/Windows/Linux の
//!   `ImeEvent::Preedit` 経路にある「フォーカス喪失時に未確定文字列を破棄する」契約と
//!   対比した iOS 固有の振る舞い）。
//!
//! 実行環境は Linux（iOS 実機は検証環境に無い）であるため、本ファイルは
//! **ソース照合済みの契約のシミュレーション**であり、実機（ソフトウェアキーボードでの
//! 実際のかな漢字変換・候補バー表示位置等）の確認は
//! 実機チェックリスト（人間担当）へ委ねる。

use fandhe_core::{Event, InteractionInput, TextFieldWidget, WidgetId};
use fandhe_winit_ime::text_field_input_from_key;
use winit::event::ElementState;
use winit::keyboard::Key;

/// テスト対象フィールドを構築し、フォーカスを与える
/// （`s16` の `focused_field` と同じ役割。結合テストは別クレートとしてコンパイルされる
/// ためファイル間で共有できず、同じ最小実装をここに独立して用意する）。
fn focused_field() -> TextFieldWidget<()> {
    let mut field: TextFieldWidget<()> =
        TextFieldWidget::new_static(WidgetId::from_raw(1), "note", "");
    field.apply_input(InteractionInput::FocusGained);
    field.take_ime_requests();
    field
}

// --- 1. 候補バー確定文字列が 1 回の insertText: にまとまって届く場合の原子的挿入 -----

/// `text_field_input_from_key` は `text: Option<&str>` を丸ごと 1 つの
/// `TextFieldInput::InsertText` へ変換する（`src/lib.rs` L166-178）。S16 のシミュレーションは
/// 「1 文字ずつの `insertText:` 呼び出し」を模していたが、実機の変換候補バー確定は
/// 「確定文字列全体が 1 回の `insertText:` 呼び出しで届く」実装（`UIKeyInput.insertText`
/// への 1 回の呼び出しに対応する 1 つの `KeyboardInput`）も許容される契約であるため、
/// この経路でも文字列が分割されず原子的に挿入されることを確認する。
#[test]
fn multi_char_commit_delivered_in_single_insert_text_call_is_inserted_atomically() {
    let mut field = focused_field();

    // 「日本語」が 1 回の insertText: 呼び出し（1 つの Pressed KeyboardInput）として届く。
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
        "1 回の insertText: 呼び出しは文字列全体を 1 つの InsertText へ変換する（分割しない）"
    );
    let event = pressed.and_then(|input| field.handle_input(input));
    assert_eq!(
        event, None,
        "InsertText は Commit イベントを返さない契約（buffer 反映のみ、S16 §2 と同型）"
    );
    assert_eq!(field.buffer(), "日本語");

    // Released 側は無視される（二重挿入防止、S16 §2 #2 と同じ回帰ガード）。
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

// --- 2. 原子的挿入直後の deleteBackward は文字単位で末尾 1 文字のみ削除する -----------

/// 複数文字を 1 回で挿入した直後に `deleteBackward` が届いても、挿入ブロック全体では
/// なく文字単位（`char` 境界）で末尾 1 文字のみが削除されることを確認する
/// （`fandhe-core` 側の `TextFieldInput::DeleteBackward` 契約への委譲を iOS 経路でも
/// 再確認する回帰ガード）。
#[test]
fn delete_backward_after_atomic_multi_char_insert_removes_only_last_character() {
    let mut field = focused_field();

    let pressed = text_field_input_from_key(
        ElementState::Pressed,
        &Key::Character("日本語".into()),
        Some("日本語"),
    )
    .expect("複数文字の insertText: は InsertText へ変換される");
    field.handle_input(pressed);
    assert_eq!(field.buffer(), "日本語");

    let backspace = Key::Named(winit::keyboard::NamedKey::Backspace);
    let delete = text_field_input_from_key(ElementState::Pressed, &backspace, None)
        .expect("Pressed の Backspace は DeleteBackward に変換される");
    field.handle_input(delete);

    assert_eq!(
        field.buffer(),
        "日本",
        "挿入ブロック全体ではなく末尾 1 文字（「語」）のみが削除される"
    );
}

// --- 3. iOS には preedit が存在しないため、フォーカス喪失で何も破棄されない ---------

/// macOS/Windows/Linux では `ImeEvent::Preedit` 経路の未確定合成中にフォーカスを失うと
/// `composition` がローカルに破棄される契約がある（`text_field.rs` L290-299、
/// `apply_input` の `FocusLost` 処理）。iOS では `Ime::*` が一切発火しないため
/// `composition` は常に `None` であり、`InsertText` で逐次確定済みの内容がそのまま
/// blur-commit されること（＝破棄される未確定入力が存在しないこと）を確認する。
#[test]
fn focus_loss_on_ios_path_never_discards_input_because_no_composition_exists() {
    let mut field = focused_field();

    for ch in "かんじへんかん".chars() {
        let key = Key::Character(ch.to_string().into());
        let mut buf = [0u8; 4];
        let text = ch.encode_utf8(&mut buf);
        let pressed = text_field_input_from_key(ElementState::Pressed, &key, Some(text))
            .expect("Pressed + text は InsertText へ変換される");
        field.handle_input(pressed);
    }
    assert_eq!(field.buffer(), "かんじへんかん");

    // フォーカス喪失: iOS 経路では composition が存在しないため、破棄される内容はなく
    // 入力済みの全文字列がそのまま blur-commit される。
    let event = field.apply_input(InteractionInput::FocusLost);
    assert_eq!(
        event,
        Some(Event::TextFieldChanged {
            label: "note".to_string(),
            value: "かんじへんかん".to_string(),
        }),
        "iOS 経路には未確定合成（composition）が存在しないため、入力済み全文字列が \
         破棄されずそのまま blur-commit される"
    );
    assert_eq!(
        field.buffer(),
        "かんじへんかん",
        "blur-commit 後も buffer 自体は変化しない（macOS/Windows/Linux と共通の契約）"
    );

    // 再フォーカスしても composition 起因の巻き戻しは発生せず、buffer は保持されたまま
    // 追記入力を続けられる。
    field.apply_input(InteractionInput::FocusGained);
    let key = Key::Character("!".into());
    let pressed = text_field_input_from_key(ElementState::Pressed, &key, Some("!"))
        .expect("Pressed + text は InsertText へ変換される");
    field.handle_input(pressed);
    assert_eq!(field.buffer(), "かんじへんかん!");
}
