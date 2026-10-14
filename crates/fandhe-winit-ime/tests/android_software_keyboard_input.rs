//! Android ソフトウェアキーボード・IME 統合のシミュレーション結合テスト
//! （TASK-8.5-S19 / REQ-8 / M3、Issue #200、親 #181）。
//!
//! winit 0.30.13
//! Android バックエンド・android-activity 0.6.1 のソースレベル検証が確定させた
//! 「`Ime::*` は Android（NativeActivity 方式）で一切発火せず、`InputEvent::KeyEvent`
//! 由来の per-key `KeyboardInput` のみが届く」契約を、本クレートの変換レイヤ
//! （[`fandhe_winit_ime::text_field_input_from_key`] /
//! [`fandhe_winit_ime::apply_ime_request`]）→ `fandhe-core` の `TextFieldWidget` までの
//! 経路として再現する。iOS（S16、`ios_software_keyboard_input.rs`）と同型のシミュレーション
//! であり、本ファイルはそのミラー構成を踏襲する。
//!
//! 実行環境は Linux（Android 実機/エミュレータでの検証は S20・#201 のスコープ）である
//! ため、本ファイルは**ソース照合済みの契約のシミュレーション**であり、実機（日本語
//! ソフトウェアキーボード・候補バー確定・IME 切替アニメーション等）での確認は本テストの
//! スコープ外とし、
//! 実機チェックリスト（S20・人間担当）へ委ねる。

use fandhe_core::ui::ImePurpose;
use fandhe_core::{Event, ImeRequest, InteractionInput, TextFieldWidget, Widget, WidgetId};
use fandhe_winit_ime::{apply_ime_request, text_field_input_from_key, ImeWindow};
use std::cell::RefCell;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::ElementState;
use winit::keyboard::{Key, NamedKey};

/// ヘッドレステスト用の `ImeWindow` モック（呼び出し引数を記録するのみ）。
/// `fandhe_winit_ime::lib.rs` 内の同名モックは `#[cfg(test)] mod tests` に閉じており
/// 本結合テスト（別クレートとしてコンパイルされる）からは参照できないため、
/// 同じ役割の最小実装をここに独立して定義する（`ios_software_keyboard_input.rs` と同じ方針）。
#[derive(Default)]
struct MockWindow {
    scale_factor: f64,
    set_ime_allowed_calls: RefCell<Vec<bool>>,
    set_ime_cursor_area_calls: RefCell<Vec<(PhysicalPosition<f64>, PhysicalSize<u32>)>>,
    set_ime_purpose_calls: RefCell<Vec<winit::window::ImePurpose>>,
}

impl MockWindow {
    fn with_scale_factor(scale_factor: f64) -> Self {
        Self {
            scale_factor,
            ..Default::default()
        }
    }
}

impl ImeWindow for MockWindow {
    fn set_ime_allowed(&self, allowed: bool) {
        self.set_ime_allowed_calls.borrow_mut().push(allowed);
    }

    fn set_ime_cursor_area(&self, position: PhysicalPosition<f64>, size: PhysicalSize<u32>) {
        self.set_ime_cursor_area_calls
            .borrow_mut()
            .push((position, size));
    }

    fn set_ime_purpose(&self, purpose: winit::window::ImePurpose) {
        self.set_ime_purpose_calls.borrow_mut().push(purpose);
    }

    fn scale_factor(&self) -> f64 {
        self.scale_factor
    }
}

/// Android の `InputEvent::KeyEvent`（`KeyAction::Down` → `KeyAction::Up`）が
/// per-key で送出する Pressed/Released 対（s19 検証記録 §2）を模した補助。
/// `text_field_input_from_key` を通し、`Some` の場合のみ `handle_input` へ適用する
/// （呼び出し側で `None` を捨てる契約、`lib.rs` doc 参照。iOS 版と同一の変換関数を再利用）。
fn insert_char_via_android_key_event(field: &mut TextFieldWidget<()>, ch: char) -> Option<Event> {
    let mut buf = [0u8; 4];
    let text = ch.encode_utf8(&mut buf);
    let key = Key::Character(text.into());

    // KeyAction::Down -> ElementState::Pressed（text あり）-> InsertText へ変換される。
    let pressed = text_field_input_from_key(ElementState::Pressed, &key, Some(text));
    let event = pressed.and_then(|input| field.handle_input(input));

    // KeyAction::Up -> ElementState::Released（text なし）-> None（二重挿入防止、無視される）。
    let released = text_field_input_from_key(ElementState::Released, &key, None);
    assert_eq!(
        released, None,
        "Released 側（KeyAction::Up 相当）は変換されず None になる（二重挿入防止）"
    );

    event
}

/// テスト対象フィールドを構築し、フォーカスを与える（`ImeRequest::SetPurpose` →
/// `ImeRequest::SetEnabled(true)` の発行を確認したうえでキューを空にする、S23 / #204）。
fn focused_field() -> TextFieldWidget<()> {
    let mut field: TextFieldWidget<()> =
        TextFieldWidget::new_static(WidgetId::from_raw(1), "note", "");
    field.apply_input(InteractionInput::FocusGained);
    assert_eq!(
        field.take_ime_requests(),
        vec![
            ImeRequest::SetPurpose(ImePurpose::Normal),
            ImeRequest::SetEnabled(true),
        ],
        "フォーカス獲得で入力目的ヒントと IME 有効化を要求する（`ime-architecture.md` §4.7）"
    );
    field
}

// --- 1. SetEnabled = ソフトウェアキーボードの表示/非表示（s19 §2） -----------

/// `ImeRequest::SetEnabled` は winit 経由で `AndroidApp::show_soft_input`/
/// `hide_soft_input` に委譲され、Android では**ソフトウェアキーボードの表示/非表示
/// そのもの**を意味する（iOS の `becomeFirstResponder`/`resignFirstResponder` と同型、
/// s19 §2）。フォーカス獲得/喪失に伴う `SetEnabled` の発行がそのままキーボードの
/// 表示制御経路になることを確認する。
#[test]
fn focus_gain_and_loss_drive_software_keyboard_visibility_via_set_enabled() {
    let mut field = focused_field();
    let window = MockWindow::with_scale_factor(2.0);

    // フォーカス獲得時の SetEnabled(true) を委譲するとキーボードが表示される
    // （show_soft_input 相当）。
    apply_ime_request(&window, &ImeRequest::SetEnabled(true));
    assert_eq!(window.set_ime_allowed_calls.borrow().as_slice(), &[true]);

    // 何か入力してからフォーカスを外す。
    insert_char_via_android_key_event(&mut field, 'a');
    let event = field.apply_input(InteractionInput::FocusLost);
    assert_eq!(
        event,
        Some(Event::TextFieldChanged {
            label: "note".to_string(),
            value: "a".to_string(),
        }),
        "blur-commit 契約: フォーカス喪失時に編集済み内容が TextFieldChanged で確定する"
    );
    let requests = field.take_ime_requests();
    assert!(
        requests.contains(&ImeRequest::SetEnabled(false)),
        "フォーカス喪失で IME 無効化（hide_soft_input 相当）を要求する"
    );

    apply_ime_request(&window, &ImeRequest::SetEnabled(false));
    assert_eq!(
        window.set_ime_allowed_calls.borrow().as_slice(),
        &[true, false],
        "SetEnabled(false) の委譲でキーボードが非表示になる（hide_soft_input 相当）"
    );
}

// --- 2. 確定文字列のみの commit 型入力（s19 §2） -----------------------------

/// 「こんにちは」を per-key `KeyboardInput`（KeyEvent 経路、dead-key 結合済み）として
/// 1 文字ずつ流し、buffer へ反映されることを確認する（Android の NativeActivity 方式は
/// `InputConnection` を実装せず、`Ime::*` も発火しないため、ソフトウェアキーボードが
/// KeyEvent 経路で届けた確定文字列を per-key で挿入する commit 型入力になる、s19 §2）。
#[test]
fn confirmed_string_from_soft_keyboard_is_inserted_char_by_char() {
    let mut field = focused_field();

    for ch in "こんにちは".chars() {
        let event = insert_char_via_android_key_event(&mut field, ch);
        assert_eq!(
            event, None,
            "InsertText は Commit イベントを返さない契約（buffer 反映のみ）"
        );
    }
    assert_eq!(field.buffer(), "こんにちは");

    // blur-commit で TextFieldChanged が発火する。
    let event = field.apply_input(InteractionInput::FocusLost);
    assert_eq!(
        event,
        Some(Event::TextFieldChanged {
            label: "note".to_string(),
            value: "こんにちは".to_string(),
        })
    );
}

// --- 3. KEYCODE_DEL（NamedKey::Backspace）による末尾削除（s19 §2） -----------

/// Android の `KEYCODE_DEL` が winit で `Key::Named(NamedKey::Backspace)` へ正規化され、
/// `TextFieldInput::DeleteBackward` へ変換されて末尾 1 文字が削除されることを確認する。
#[test]
fn keycode_del_key_event_removes_last_character() {
    let mut field = focused_field();

    for ch in "abc".chars() {
        insert_char_via_android_key_event(&mut field, ch);
    }
    assert_eq!(field.buffer(), "abc");

    let backspace = Key::Named(NamedKey::Backspace);
    let pressed = text_field_input_from_key(ElementState::Pressed, &backspace, None);
    assert_eq!(pressed, Some(fandhe_core::TextFieldInput::DeleteBackward));
    field.handle_input(
        pressed.expect("Pressed の Backspace（KEYCODE_DEL）は DeleteBackward に変換される"),
    );

    // Released 側（KeyAction::Up 相当）は無視される（二重削除防止）。
    let released = text_field_input_from_key(ElementState::Released, &backspace, None);
    assert_eq!(released, None);

    assert_eq!(field.buffer(), "ab");
}

// --- 4. 絵文字（複数コードポイント）の破壊防止 -------------------------------

/// 絵文字がソフトウェアキーボード経由の KeyEvent で 1 イベント 1 文字列として渡されても、
/// 加工されず 1 個の文字として挿入され、buffer が破壊されないことを確認する
/// （`char` 単位の分割挿入で UTF-8 境界が破壊されないことを担保する。iOS 版と同じ検証観点）。
///
/// **TASK-8.5-S26（#207）契約更新**: `DeleteBackward` は char 1 個ではなく
/// grapheme cluster 単位で削除する（`fandhe-core` の `handle_input` 是正）。単一
/// コードポイントの絵文字（🎉）では挙動に差が出ないため、下の
/// `emoji_with_skin_tone_modifier_is_deleted_as_whole_cluster` で複数 char クラスタの
/// 削除契約を追加検証する。
#[test]
fn emoji_input_is_not_corrupted_by_char_by_char_insertion() {
    let mut field = focused_field();

    for ch in "🎉".chars() {
        insert_char_via_android_key_event(&mut field, ch);
    }
    assert_eq!(field.buffer(), "🎉");

    // 絵文字を削除しても panic せず、grapheme cluster 単位（本ケースは 1 char のため
    // 従来の char 単位削除と同じ結果）で削除される。
    let backspace = Key::Named(NamedKey::Backspace);
    let pressed = text_field_input_from_key(ElementState::Pressed, &backspace, None)
        .expect("Backspace は DeleteBackward に変換される");
    field.handle_input(pressed);
    assert_eq!(field.buffer(), "");
}

/// 肌色修飾子つき絵文字（👍🏽 = 2 char・1 grapheme cluster）が KeyEvent 経由の
/// per-char 挿入で分割されて届いても、`DeleteBackward` 1 回でクラスタ全体が削除され、
/// 修飾子だけが残留しないことを確認する（TASK-8.5-S26 / #207、grapheme cluster 対応）。
#[test]
fn emoji_with_skin_tone_modifier_is_deleted_as_whole_cluster() {
    let mut field = focused_field();

    for ch in "👍🏽".chars() {
        insert_char_via_android_key_event(&mut field, ch);
    }
    assert_eq!(field.buffer(), "👍🏽");

    let backspace = Key::Named(NamedKey::Backspace);
    let pressed = text_field_input_from_key(ElementState::Pressed, &backspace, None)
        .expect("Backspace は DeleteBackward に変換される");
    field.handle_input(pressed);

    assert_eq!(field.buffer(), "");
}

// --- 5. Ime::* 経路が存在しないため preedit 表示が決して現れない -------------

/// Android（NativeActivity 方式）は `Ime::*` が一切発火しないため（s19 §2 事実 1・2）、
/// `text_field_input_from_key` 経由の入力では `TextFieldWidget::apply_ime` が一度も
/// 呼ばれず、preedit 描画（`PaintCommand::CompositionText`）が発生しないことを確認する
/// （`ime-architecture.md` §9「Android インライン preedit 未対応」の裏付け）。
#[test]
fn composition_display_never_appears_without_ime_event_path() {
    let mut field = focused_field();

    for ch in "test".chars() {
        insert_char_via_android_key_event(&mut field, ch);
    }

    let size = fandhe_core::Measurement {
        width: 200.0,
        height: 20.0,
    };
    let has_composition_paint = field
        .paint((0.0, 0.0), size)
        .into_iter()
        .any(|command| matches!(command, fandhe_core::PaintCommand::CompositionText { .. }));
    assert!(
        !has_composition_paint,
        "apply_ime を経由しない入力では preedit 描画が一切発生しない"
    );
    assert_eq!(field.buffer(), "test");
}

// --- 6. SetCursorArea / SetPurpose は Android では no-op（委譲自体は行われる） ---

/// `ImeRequest::SetCursorArea` / `SetPurpose` は Android では no-op（s19 §2、
/// `mod.rs` L916・L926 の空実装）だが、`apply_ime_request` はプラットフォーム側の
/// 対応有無を判定せず契約どおり委譲する（本クレートの責務は変換・委譲のみであり、
/// no-op 判定はプラットフォーム側 API の実装詳細。iOS 版と同じ検証観点）。
/// `MockWindow` への委譲呼び出し自体が行われることを確認する。
#[test]
fn cursor_area_and_purpose_requests_are_still_delegated_though_android_ignores_them() {
    let window = MockWindow::with_scale_factor(2.0);

    apply_ime_request(
        &window,
        &ImeRequest::SetCursorArea {
            origin: (10.0, 20.0),
            size: (30.0, 40.0),
        },
    );
    assert_eq!(window.set_ime_cursor_area_calls.borrow().len(), 1);

    apply_ime_request(&window, &ImeRequest::SetPurpose(ImePurpose::Password));
    assert_eq!(
        window.set_ime_purpose_calls.borrow().as_slice(),
        &[winit::window::ImePurpose::Password]
    );
}

// --- 7. Released 側 KeyboardInput（text: None）が二重挿入・二重削除にならない ---

/// Android 固有の観点: `InputEvent::KeyEvent` の `KeyAction::Up`（`ElementState::Released`）
/// は winit 側で `text: None` として届く。Pressed 側（`KeyAction::Down`）で既に
/// 反映済みの内容に対し、Released 側の処理が buffer を変化させないことを確認する
/// （s19 §4 対応表「二重挿入・二重削除にならない」）。
#[test]
fn released_key_event_with_no_text_does_not_mutate_buffer() {
    let mut field = focused_field();

    // 'a' を挿入（Pressed で InsertText、Released は None のため buffer 不変）。
    let key = Key::Character("a".into());
    let pressed = text_field_input_from_key(ElementState::Pressed, &key, Some("a"));
    field.handle_input(pressed.expect("Pressed は InsertText に変換される"));
    assert_eq!(field.buffer(), "a");

    let released = text_field_input_from_key(ElementState::Released, &key, None);
    assert_eq!(released, None, "Released 側は変換されない契約");
    // released が None のため handle_input へは渡さない（呼び出し側契約どおり）。
    assert_eq!(
        field.buffer(),
        "a",
        "Released 側の処理で buffer が変化しない"
    );

    // Backspace も同様（Pressed で DeleteBackward、Released は None）。
    let backspace = Key::Named(NamedKey::Backspace);
    let pressed_del = text_field_input_from_key(ElementState::Pressed, &backspace, None);
    field.handle_input(pressed_del.expect("Pressed の Backspace は DeleteBackward に変換される"));
    assert_eq!(field.buffer(), "");

    let released_del = text_field_input_from_key(ElementState::Released, &backspace, None);
    assert_eq!(released_del, None);
    assert_eq!(
        field.buffer(),
        "",
        "Released 側の Backspace で二重削除にならない"
    );
}
