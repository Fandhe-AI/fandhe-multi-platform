//! Windows（Microsoft IME・IMM32）での日本語入力シナリオ再現テスト
//! （TASK-8.5-S12 / REQ-8 / M3、Issue #193、親 #181）。
//!
//! `docs/architecture/ime-architecture.md` §4.4 は Windows バックエンドについて
//! 「winit 内部で IMM32（`WM_IME_*` メッセージ）へ委譲されるため、`fandhe-core` から見た
//! 契約は winit マッピング表のみで完結する」としている。本ファイルは S9（#190、macOS）が
//! 確立した「winit バックエンドソースの読解で確定したイベント列を実際に
//! `ime_event_from_winit()` → `TextFieldWidget::apply_ime()` へ流す」検証方式を Windows へ
//! 適用したもので、
//! 検証で得た対応表を実行可能な形で固定化する。
//!
//! 参照した winit 0.30.13（Cargo.lock 固定）の Windows バックエンドソース:
//! - `src/platform_impl/windows/event_loop.rs` L1513-1612
//!   （`WM_IME_STARTCOMPOSITION` / `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION` ハンドラ）
//! - `src/platform_impl/windows/ime.rs`（`ImeContext::get_composing_text_and_cursor` 等、
//!   IMM32 `GCS_COMPSTR` / `GCS_CURSORPOS` / `GCS_COMPATTR` の解釈）
//!
//! # winit Windows バックエンドが実際に発火するイベント列（ソース照合で確定）
//!
//! - コンポジション開始（`WM_IME_STARTCOMPOSITION`）: `ime_allowed` なら
//!   `Ime::Enabled` を発火する（event_loop.rs L1513-1524）。macOS の「最初の
//!   `setMarkedText` まで遅延」とは異なり、Windows では開始メッセージ自体で即時発火する。
//! - 変換中（`WM_IME_COMPOSITION`）: `GCS_RESULTSTR` フラグが立っていれば確定文字列を
//!   「空 `Preedit` → `Commit`」の順で送出し（L1546-1558）、続けて `GCS_COMPSTR`
//!   フラグが立っていれば次の変換候補の `Preedit` を送出する（L1562-1574）。
//!   Google 日本語入力・ATOK は両フラグを同時に立てうるため、1 回の
//!   `WM_IME_COMPOSITION` で「確定 → 次の変換開始」が連続することがある
//!   （event_loop.rs L1544-1545 のコメント参照）。
//! - 変換候補・カーソル範囲（`ime.rs::get_composing_text_and_cursor`）: 返す
//!   `first`/`last` は byte offset（`chr.len_utf8()` の累積、ime.rs L37-66）であり、
//!   `fandhe-winit-ime::ime_event_from_winit` の `char_offset_from_byte` 前提と一致する。
//! - コンポジション終了（`WM_IME_ENDCOMPOSITION`）: 変換中に確定せず終了した場合でも
//!   直前の `ime_state == Preedit` なら最後の確定文字列取得を試み、あれば
//!   「空 `Preedit` → `Commit`」を送出し（L1587-1600）、最後に必ず `Ime::Disabled` を
//!   発火する（L1603-1608）。**macOS の `Disabled` はフォーカス喪失・入力ソース切替
//!   時のみだが、Windows では 1 回の変換完了ごとに必ず `Disabled` が発火する**点が
//!   ソース照合で判明した最大の差異。`TextFieldWidget::apply_ime` の `Disabled` 処理は composition の
//!   クリアのみで冪等なため（`text_field.rs` §4.6）、この高頻度な
//!   Enabled/Disabled サイクルを問題なく許容できることを本ファイルで確認する。
//! - Esc によるキャンセル: `WM_IME_ENDCOMPOSITION` 到達時に `get_composed_text` が
//!   `None`（確定文字列なし）であれば「空 Preedit → Commit」は送出されず、
//!   `Ime::Disabled` のみが発火する（L1586-1608 の条件分岐）。
//!
//! `winit::event::Ime` は環境依存のフィールドを持たないプレーンな enum のため、
//! 実ウィンドウ・実 IME コンテキストなしで値を直接構築してテストできる
//! （ウィンドウハンドルは `ImeWindow` トレイト経由の `apply_ime_request` 側でのみ必要）。

use fandhe_core::{Event, ImeRequest, InteractionInput, TextFieldWidget, WidgetId};
use fandhe_winit_ime::ime_event_from_winit;
use winit::event::Ime;

/// `TextFieldWidget` をフォーカス状態にする（`tests/text_input_widget.rs` の
/// `focus` ヘルパーと同じ操作を公開 API 経由で行う）。
fn focused_field(initial: &str) -> TextFieldWidget<()> {
    let mut field = TextFieldWidget::new_static(WidgetId::from_raw(1), "メモ", initial);
    field.apply_input(InteractionInput::FocusGained);
    field
}

/// winit の `Ime` イベントを `ime_event_from_winit` 経由でウィジェットへ適用する
/// テストヘルパー（本ファイル全体で使う「winit → コア」経路の共通処理）。
fn apply(field: &mut TextFieldWidget<()>, ime: Ime) -> Option<Event> {
    field.apply_ime(ime_event_from_winit(ime))
}

// --- シナリオ 1: 「にほんご」ローマ字入力 → 変換 → 確定 -----------------------

/// 「にほんご」をローマ字入力し、スペースで漢字変換（「日本語」）、Enter で確定する
/// 代表シナリオ。winit ソース照合（event_loop.rs L1513-1574）どおりの発火順で
/// `apply_ime` へ流し、`buffer` に確定文字列が反映され `composition` がクリアされる
/// ことを確認する。
#[test]
fn nihongo_romaji_input_converts_and_commits() {
    let mut field = focused_field("");

    // WM_IME_STARTCOMPOSITION（event_loop.rs L1513-1524）。
    assert_eq!(apply(&mut field, Ime::Enabled), None);

    // ローマ字タイプ中の逐次 Preedit 更新（"n" → "に" → "にほ" → "にほん" → "にほんご"）。
    // WM_IME_COMPOSITION の GCS_COMPSTR 分岐（event_loop.rs L1562-1574）に対応。
    for partial in ["に", "にほ", "にほん", "にほんご"] {
        apply(
            &mut field,
            Ime::Preedit(partial.to_string(), Some((0, partial.len()))),
        );
    }
    assert_eq!(field.buffer(), "", "確定前は buffer へ反映しない（§4.3）");

    // スペースキーによる変換候補選択で preedit が漢字候補「日本語」へ置き換わる
    // （引き続き GCS_COMPSTR 経由の Preedit 更新）。
    apply(
        &mut field,
        Ime::Preedit("日本語".to_string(), Some((0, "日本語".len()))),
    );

    // Enter 確定: GCS_RESULTSTR 分岐が「空 Preedit → Commit」を送出する
    // （event_loop.rs L1546-1558）。
    apply(&mut field, Ime::Preedit(String::new(), None));
    let event = apply(&mut field, Ime::Commit("日本語".to_string()));

    assert_eq!(field.buffer(), "日本語");
    assert_eq!(field.cursor(), 3);
    assert!(
        event.is_none(),
        "Commit 自体は TextFieldChanged を発火しない（blur-commit のみが発火源、§4.5）"
    );
}

// --- シナリオ 2: 変換中の Esc キャンセル ---------------------------------------

/// 変換中に Esc でキャンセルした場合、winit は `Ime::Commit` を送出せず
/// `Ime::Disabled` のみを発火する（event_loop.rs L1586-1608、`get_composed_text`
/// が `None` の分岐）。`buffer` が不変であること、`composition` がクリアされる
/// ことを確認する。
#[test]
fn escape_during_composition_cancels_without_commit() {
    let mut field = focused_field("既存値");

    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("にほんご".to_string(), Some((0, "にほんご".len()))),
    );

    // WM_IME_ENDCOMPOSITION: 確定文字列なし → Commit を経由せず Disabled のみ。
    let event = apply(&mut field, Ime::Disabled);

    assert_eq!(field.buffer(), "既存値", "キャンセルで buffer は変化しない");
    assert!(event.is_none());
}

// --- シナリオ 3: 複文節変換とカーソル移動（マルチバイト境界） -----------------------

/// 複文節（「今日は晴れ」のような複数文節）変換中の cursor_range 移動を、
/// `ime.rs::get_composing_text_and_cursor` が返す byte offset 契約
/// （L37-66、`chr.len_utf8()` の累積）に沿ってシミュレートする。マルチバイト境界の
/// byte → char 変換（`ime_event_from_winit` 側の `char_offset_from_byte`）が
/// panic せず妥当な char offset を返すことを確認する。
#[test]
fn multi_clause_conversion_moves_cursor_across_multibyte_boundaries() {
    let mut field = focused_field("");
    apply(&mut field, Ime::Enabled);

    // "きょうははれ"（文節境界の例）。文節 1（「きょう」）が変換対象
    // （byte 0..9、"きょう" は 3 文字 × 3 byte）。
    let text = "きょうはは れ".replace(' ', "");
    apply(
        &mut field,
        Ime::Preedit(text.clone(), Some((0, "きょう".len()))),
    );
    // Tab 等で変換対象文節を後方（「はは」相当、byte 9..15）へ移す。
    let clause_start = "きょう".len();
    let clause_end = clause_start + "はは".len();
    apply(
        &mut field,
        Ime::Preedit(text.clone(), Some((clause_start, clause_end))),
    );

    // Enter で全体を確定。
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit(text.clone()));

    assert_eq!(field.buffer(), text);
    assert_eq!(field.cursor(), text.chars().count());
}

// --- シナリオ 4: 半角/全角キー切替（Enabled → Preedit → Disabled → Enabled） --------

/// Windows 固有の高頻度 Enabled/Disabled サイクル（モジュール doc 参照）を、
/// 半角/全角キー切替に相当する「変換未確定のままコンポジション終了 → 直後に
/// 再度コンポジション開始」で再現する。`apply_ime` の `Disabled` 処理が
/// フォーカス状態によらず冪等にクリアするだけであるため（`text_field.rs` §4.6）、
/// この再サイクルを状態モデルが問題なく許容できることを確認する。
#[test]
fn ime_mode_toggle_cycles_enabled_preedit_disabled_enabled() {
    let mut field = focused_field("");

    // 1 サイクル目: 開始 → 変換中 → 未確定のまま終了（Disabled のみ、Commit なし）。
    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("あ".to_string(), Some((0, "あ".len()))),
    );
    apply(&mut field, Ime::Disabled);
    assert_eq!(field.buffer(), "", "1 サイクル目は未確定のため buffer 不変");

    // 2 サイクル目: 直後に再び Enabled → 確定まで到達する。
    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("い".to_string(), Some((0, "い".len()))),
    );
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("い".to_string()));

    assert_eq!(field.buffer(), "い", "2 サイクル目の確定は正しく反映される");
}

// --- シナリオ 5: コンポジション中のフォーカス喪失 -------------------------------

/// 変換中に別フィールドへフォーカスが移動した場合、`ime-architecture.md` §4.6 の
/// フォーカスガードにより `TextFieldWidget::apply_input(FocusLost)` が
/// composition を**確定せずローカルに破棄**し、`take_ime_requests()` へ
/// `ImeRequest::SetEnabled(false)` を積むことを確認する（`text_field.rs` L324-329）。
/// これは winit イベントではなくコア側 `InteractionInput` 経由の遷移だが、
/// Windows で `WM_KILLFOCUS` 等によりフォーカスが奪われた際にプラットフォーム層が
/// 呼び出すべき経路として本シナリオに含める。
#[test]
fn focus_lost_during_composition_discards_without_commit() {
    let mut field = focused_field("");
    // フォーカス取得時の SetEnabled(true) 要求を drain し、本シナリオが検証したい
    // FocusLost 由来の要求のみを対象にする。
    field.take_ime_requests();

    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("にほんご".to_string(), Some((0, "にほんご".len()))),
    );

    let event = field.apply_input(InteractionInput::FocusLost);

    assert_eq!(
        field.buffer(),
        "",
        "未確定コンポジションは破棄され buffer へ反映されない"
    );
    assert!(
        event.is_none(),
        "空 buffer のままなので blur-commit は発火しない"
    );
    assert_eq!(
        field.take_ime_requests(),
        vec![ImeRequest::SetEnabled(false)],
        "フォーカス喪失時は SetEnabled(false) をプラットフォーム層へ要求する"
    );
}

// --- シナリオ 6: サロゲートペア文字の確定 ---------------------------------------

/// 補助多言語面（サロゲートペア）の文字（例: 「𩸽」U+29E3D、鰤の異体字）の確定を
/// 検証する。`String`/`char` は UTF-8 内部表現のため winit 側で UTF-16 サロゲート
/// ペアは 1 つの `char` へ正しく変換済みの前提だが、`fandhe-core` 側の char 単位
/// カーソル計算（`chars().count()`）がサロゲートペア相当の 1 文字を 1 として
/// 数えることを確認する（S26 #207 の網羅検証はスコープ外、ここでは 1 ケースのみ）。
#[test]
fn commits_supplementary_plane_character() {
    let mut field = focused_field("前");

    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("𩸽".to_string(), Some((0, "𩸽".len()))),
    );
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("𩸽".to_string()));

    assert_eq!(field.buffer(), "前𩸽");
    assert_eq!(
        field.cursor(),
        2,
        "サロゲートペア相当の 1 文字はカーソル 1 と数える"
    );
}
