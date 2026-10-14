//! macOS 日本語入力（かな漢字変換）動作のシミュレーション結合テスト
//! （TASK-8.5-S10 / REQ-8 / M3、Issue #191）。
//!
//! 親 issue #181（TASK-8.5「IME 統合実装」）配下、S9（#190）が winit
//! macOS バックエンドのソースコードレベルで確定させた実イベント列・実挙動を、
//! 本クレートの変換レイヤ（[`fandhe_winit_ime::ime_event_from_winit`] /
//! [`fandhe_winit_ime::apply_ime_request`]）→ `fandhe-core` のコンポジション状態モデル
//! （[`fandhe_core::TextFieldWidget::apply_ime`] / [`fandhe_core::ui::ime_cursor_area`]）
//! までの経路として再現する。
//!
//! 実行環境は Linux（macOS 実機は検証環境に無い）であるため、本ファイルは
//! **ソース照合済みの契約のシミュレーション**であり、実機（日本語 IME・ライブ変換 on/off・
//! 実際の候補ウィンドウ位置）での確認は本テストのスコープ外とし、
//! 実機チェックリスト（人間担当）へ委ねる。
//!
//! 各テストケースの冒頭コメントに、対応する S9 検証の項目を明記する。

use fandhe_core::ui::{ime_cursor_area, ImeCursorAreaParams};
use fandhe_core::{
    ImeEvent, ImePurpose, ImeRequest, InteractionInput, PaintCommand, TextFieldWidget, Widget,
    WidgetId,
};
use fandhe_winit_ime::{apply_ime_request, ime_event_from_winit, ImeWindow};
use std::cell::RefCell;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::Ime;

/// ヘッドレステスト用の `ImeWindow` モック（呼び出し引数を記録するのみ）。
///
/// `fandhe_winit_ime::lib.rs` 内の同名モックは `#[cfg(test)] mod tests` に閉じており
/// 本結合テスト（`tests/` 配下、別クレートとしてコンパイルされる）からは参照できないため、
/// 同じ役割の最小実装をここに独立して定義する。
#[derive(Default)]
struct MockWindow {
    scale_factor: f64,
    set_ime_allowed_calls: RefCell<Vec<bool>>,
    set_ime_cursor_area_calls: RefCell<Vec<(PhysicalPosition<f64>, PhysicalSize<u32>)>>,
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

    fn set_ime_purpose(&self, _purpose: winit::window::ImePurpose) {
        // 本テストでは SetPurpose 経路は対象外
        // （S9 §3 表 #1: macOS では no-op、S23/S24 スコープ）。
    }

    fn scale_factor(&self) -> f64 {
        self.scale_factor
    }
}

/// テスト対象フィールドを構築し、フォーカスを与える（`ImeRequest::SetPurpose` →
/// `ImeRequest::SetEnabled(true)` の発行を確認したうえでキューを空にする、S23 / #204。
/// 以降のアサーションを単純化するための補助）。
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

/// `Widget::paint` が返す `PaintCommand::CompositionText` を取り出す（Composing 中のみ
/// `Some`）。`TextFieldWidget` は composition の内容を直接公開しないため、公開 API
/// （`Widget<S>::paint`）経由でのみ検証する（クレート外部から観測可能な契約のみを
/// 固定化する、`tests/text_input_widget.rs` と同じ方針）。
fn composition_paint_command<S: 'static>(field: &TextFieldWidget<S>) -> Option<PaintCommand> {
    let size = fandhe_core::Measurement {
        width: 200.0,
        height: 20.0,
    };
    field
        .paint((0.0, 0.0), size)
        .into_iter()
        .find(|command| matches!(command, PaintCommand::CompositionText { .. }))
}

// --- 1. かな漢字変換の基本列 -------------------------------------------------

/// S9 §3 表 #5・#6 の組み合わせ: 「にほんご」→ 変換候補「日本語」→ 空 Preedit → Commit の
/// 一連の流れで、preedit が確定バッファへ混入せず、最終的に確定文字列のみが buffer へ
/// 反映されることを検証する。
#[test]
fn kana_kanji_conversion_reaches_commit_without_preedit_leak() {
    let mut field = focused_field();

    // ひらがな入力途中（IME 側で 1 文字ずつ Preedit が届く。winit の実イベント列では
    // `insertText`/`setMarkedText` 経由でこの粒度がそのまま届く、S9 §3 表 #2）。
    for partial in ["に", "にほ", "にほんご"] {
        let event = ime_event_from_winit(Ime::Preedit(partial.to_string(), None));
        assert_eq!(field.apply_ime(event), None);
        let command = composition_paint_command(&field).expect("Composing 中は preedit を描画する");
        match command {
            PaintCommand::CompositionText { preedit, .. } => assert_eq!(preedit, partial),
            other => panic!("unexpected paint command: {other:?}"),
        }
        assert_eq!(field.buffer(), "", "preedit は確定バッファに混入しない");
    }

    // 変換候補「日本語」を選択（カーソル範囲付き）。winit の Preedit カーソルは UTF-8
    // バイトオフセットであり、「日本語」の各文字は 3 バイトなので全選択は (0, 9) となる
    // （Bugbot 指摘対応: (0, 3) では先頭 1 文字分しか選択されず S9 §3 表 #6 を再現できない）。
    let event = ime_event_from_winit(Ime::Preedit("日本語".to_string(), Some((0, 9))));
    field.apply_ime(event);
    assert_eq!(field.buffer(), "");

    // S9 §3 表 #5: Commit 直前に必ず空 Preedit が届く契約（winit ソース照合済み）。
    let clear = ime_event_from_winit(Ime::Preedit(String::new(), None));
    field.apply_ime(clear);
    assert!(
        composition_paint_command(&field).is_none(),
        "空 Preedit でコンポジションがクリアされ、通常描画へ戻る"
    );

    let commit = ime_event_from_winit(Ime::Commit("日本語".to_string()));
    let result = field.apply_ime(commit);
    assert_eq!(
        result, None,
        "apply_ime 自体は Event を返さない契約（Commit は widget 側で処理）"
    );
    assert_eq!(field.buffer(), "日本語");
}

// --- 2. Enabled 遅延発火（S9 §3 表 #2） -------------------------------------

/// macOS では `Enabled` が `SetEnabled(true)` の直後ではなく最初の `setMarkedText`
/// （コンポジション開始）時に遅延発火する（S9 §3 表 #2）。コア状態モデルは
/// `ImeEvent::Enabled` を受理のみ（no-op）として扱うため、`Enabled` が届く前に
/// `Preedit` から始まっても composition 状態遷移そのものは正しく進行する必要がある。
#[test]
fn enabled_event_may_arrive_late_or_be_skipped_before_first_preedit() {
    let mut field = focused_field();

    // macOS 実装では SetEnabled(true) 発行直後に Enabled が届くとは限らない
    // （フォーカス直後は届かず、最初の setMarkedText 時に遅延発火する）。
    // Enabled を経ずに直接 Preedit が来ても composition が開始されることを確認する。
    let preedit = ime_event_from_winit(Ime::Preedit("あ".to_string(), None));
    field.apply_ime(preedit);
    assert!(
        composition_paint_command(&field).is_some(),
        "Enabled 未着でも Preedit 単独で composition が開始する"
    );

    // 遅延して Enabled が届いても composition 状態は破壊されない（受理のみ）。
    let enabled = ime_event_from_winit(Ime::Enabled);
    let result = field.apply_ime(enabled);
    assert_eq!(result, None);
    assert!(composition_paint_command(&field).is_some());
}

// --- 3. 入力ソース切替の Disabled 再サイクル（S9 §3 表 #4） ------------------

/// 入力ソース切替（かな→ABC 等）でフォーカスが継続していても `Disabled` が発火し、
/// 次のコンポジション開始まで `Enabled` は来ない（S9 §3 表 #4）。composition 途中で
/// `Disabled` を受けた場合、未確定文字列は Commit されずに破棄され（§4.6）、
/// その後の新規 `Preedit` で composition が正しく再開できることを確認する。
#[test]
fn input_source_switch_discards_composition_and_allows_recycle() {
    let mut field = focused_field();

    field.apply_ime(ime_event_from_winit(Ime::Preedit("かな".to_string(), None)));
    assert!(composition_paint_command(&field).is_some());

    // 入力ソース切替: Disabled が届く（Commit を経ない未確定文字列の破棄）。
    let disabled = ime_event_from_winit(Ime::Disabled);
    let result = field.apply_ime(disabled);
    assert_eq!(result, None);
    assert!(
        composition_paint_command(&field).is_none(),
        "Disabled は composition を確定せずにクリアする（§4.6）"
    );
    assert_eq!(
        field.buffer(),
        "",
        "破棄された preedit は buffer へ反映されない"
    );

    // 再度 Enabled → Preedit の順で新しい composition が開始できる（再サイクル）。
    field.apply_ime(ime_event_from_winit(Ime::Enabled));
    field.apply_ime(ime_event_from_winit(Ime::Preedit("あ".to_string(), None)));
    assert!(
        composition_paint_command(&field).is_some(),
        "Disabled 後も新規 Preedit で composition を再開できる"
    );
}

// --- 4. フォーカス外イベントの破棄 ------------------------------------------

/// 非フォーカス中に届いた `Preedit` / `Commit` は破棄される（フォーカスガード、
/// `text-input-state-model.md` §4.3・§4.5 手順 0）。macOS でフォーカスが別ウィンドウ・
/// 別フィールドへ移った直後に旧フィールド宛のイベントが遅延到達するケースを模す。
#[test]
fn preedit_and_commit_are_discarded_while_unfocused() {
    let mut field: TextFieldWidget<()> =
        TextFieldWidget::new_static(WidgetId::from_raw(2), "note", "");
    // フォーカスを一切与えない（Idle・非フォーカス状態）。

    field.apply_ime(ime_event_from_winit(Ime::Preedit(
        "無視される".to_string(),
        None,
    )));
    assert!(composition_paint_command(&field).is_none());
    assert_eq!(field.buffer(), "");

    let result = field.apply_ime(ime_event_from_winit(Ime::Commit("無視される".to_string())));
    assert_eq!(result, None);
    assert_eq!(
        field.buffer(),
        "",
        "非フォーカス中の Commit は破棄され buffer は変化しない"
    );
}

// --- 5. Pinyin 等の範囲外オフセット（S9 §3 表 #6 / S10 申し送り 3） ----------

/// Pinyin 等、範囲外・非境界の `NSRange`（byte offset 換算）を送出する既知の IME
/// （alacritty/alacritty#8791 系）を想定し、マルチバイト文字列・絵文字（サロゲート
/// ペア相当）に対する範囲外オフセットが panic せずクランプされることを確認する
/// （`ime_event_from_winit` → `char_offset_from_byte` の契約）。
#[test]
fn out_of_range_and_non_boundary_cursor_offsets_are_clamped_without_panicking() {
    // "こんにちは🎉" は 6 char（絵文字 1 個含む）。絵文字は 4 byte（サロゲートペア相当）。
    let text = "こんにちは🎉";
    let byte_len = text.len();

    // 範囲外オフセット（文字列長を超える）は char 数（末尾）へクランプされる
    // （`char_offset_from_byte` の契約: byte_offset >= text.len() -> char_count）。
    let event = ime_event_from_winit(Ime::Preedit(
        text.to_string(),
        Some((byte_len + 100, byte_len + 200)),
    ));
    match event {
        ImeEvent::Preedit {
            cursor: Some((start, end)),
            ..
        } => {
            let char_count = text.chars().count();
            assert_eq!(start, char_count);
            assert_eq!(end, char_count);
        }
        other => panic!("unexpected event: {other:?}"),
    }

    // 絵文字（4 byte）の中間点（非境界）を指すオフセットも panic せず、含む char の
    // 先頭側へクランプされる。
    let emoji_start_byte = "こんにちは".len();
    let non_boundary = emoji_start_byte + 2;
    let event = ime_event_from_winit(Ime::Preedit(
        text.to_string(),
        Some((non_boundary, non_boundary)),
    ));
    match event {
        ImeEvent::Preedit {
            cursor: Some((start, end)),
            ..
        } => {
            // 絵文字は 6 文字目（0-indexed で 5）。中間点は絵文字自身の先頭（5）へ
            // クランプされる。
            assert_eq!(start, 5);
            assert_eq!(end, 5);
        }
        other => panic!("unexpected event: {other:?}"),
    }

    // これらのイベントをフィールドへ適用しても panic しないことも合わせて確認する。
    let mut field = focused_field();
    let event = ime_event_from_winit(Ime::Preedit(
        text.to_string(),
        Some((byte_len + 100, byte_len + 200)),
    ));
    field.apply_ime(event);
    assert!(composition_paint_command(&field).is_some());
}

// --- 6. 候補ウィンドウ位置要求の往復一貫性（S9 §3 表 #8） --------------------

/// `ImeRequest::SetCursorArea` の発行タイミング制御・`TextFieldWidget` への配線は
/// S8（#189）が明示的にスコープ外としており（`text_field.rs` モジュール doc「スコープ外」
/// 節参照）、`TextFieldWidget::take_ime_requests()` は現時点で `SetCursorArea` を
/// 一切発行しない（配線の不在。実装差分ではなく既知のギャップ）。
///
/// そのため本テストは、コア側の純粋計算（[`ime_cursor_area`]、S6）が返す論理ピクセル
/// 矩形を手動で `ImeRequest::SetCursorArea` へ組み立て、[`apply_ime_request`]（S7・S9）
/// が winit の物理ピクセルへ変換する経路を検証する。S9 §3 表 #8（winit
/// `set_ime_cursor_area` → `to_logical(scale_factor)` → `firstRectForCharacterRange`
/// の往復一貫性）と組み合わせ、コア計算からプラットフォーム適用までの経路全体が
/// 破綻なく合成できることを確認する。
#[test]
fn cursor_area_from_core_computation_round_trips_through_platform_layer() {
    // コンポジション中の日本語文字列（かな漢字変換の変換候補選択中を想定）。
    let params = ImeCursorAreaParams {
        field_origin: fandhe_core::Point::new(10.0, 20.0),
        field_size: fandhe_core::Size::new(200.0, 20.0),
        buffer: "こんにちは",
        anchor: 5,
        preedit: "世界",
        preedit_cursor: Some((0, 2)),
    };
    let rect = ime_cursor_area(&params);

    let window = MockWindow::with_scale_factor(2.0);
    let request = ImeRequest::SetCursorArea {
        origin: (rect.origin.x, rect.origin.y),
        size: (rect.size.width, rect.size.height),
    };
    apply_ime_request(&window, &request);

    let calls = window.set_ime_cursor_area_calls.borrow();
    assert_eq!(
        calls.len(),
        1,
        "SetCursorArea は 1 回だけ winit へ委譲される"
    );
    let (position, size) = calls[0];
    // scale_factor 2.0 での論理→物理変換（`apply_ime_request` の契約）。
    assert_eq!(position.x, f64::from(rect.origin.x) * 2.0);
    assert_eq!(position.y, f64::from(rect.origin.y) * 2.0);
    assert_eq!(size.width, (f64::from(rect.size.width) * 2.0).ceil() as u32);
    assert_eq!(
        size.height,
        (f64::from(rect.size.height) * 2.0).ceil() as u32
    );
}
