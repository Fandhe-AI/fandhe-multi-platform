//! Linux（X11/XIM・Wayland/`zwp_text_input_v3`）での日本語入力（ibus/fcitx 経由）
//! シナリオ再現テスト（TASK-8.5-S14 / REQ-8 / M3、Issue #195、親 #181）。
//!
//! `docs/architecture/ime-architecture.md` §4.4 は Linux バックエンドについて
//! 「ibus/fcitx への委譲も winit 内部で完結し、`fandhe-core` 契約には影響しない」と
//! している。本ファイルは S9（#190、macOS）・S12（#193、Windows）が確立した
//! 「winit バックエンドソースの読解で確定したイベント列を実際に
//! `ime_event_from_winit()` → `TextFieldWidget::apply_ime()` へ流す」検証方式を
//! Linux（X11・Wayland 両バックエンド）へ適用したもので、
//! 検証で得た対応表を実行可能な形で固定化する。
//!
//! 参照した winit 0.30.13（Cargo.lock 固定）の Linux バックエンドソース:
//! - X11: `src/platform_impl/linux/x11/ime/{mod.rs,context.rs,callbacks.rs}`
//!   （XIM `preedit_start`/`preedit_draw`/`preedit_done` コールバック、
//!   `Ime::create_context` / `set_ime_allowed` での `Enabled`/`Disabled` 同期発火、
//!   `xim_destroy_callback` / `replace_im` での IM 再起動時の再サイクル）、
//!   `src/platform_impl/linux/x11/event_processor.rs`（`ImeEvent` → `Ime::*` 変換、
//!   `Ime::Preedit` のカーソルが常に `(position, position)` の単一点になる変換、
//!   `lookup_utf8` による通常キー入力経由の `Commit` 送出）
//! - Wayland: `src/platform_impl/linux/wayland/seat/text_input/mod.rs`
//!   （`zwp_text_input_v3` の `Enter`/`Leave`/`PreeditString`/`CommitString`/`Done` ハンドラ、
//!   `Done` での「空 Preedit → Commit → 次 Preedit」バッチ適用順序、
//!   `set_content_type_by_purpose` が Wayland でのみ実際に `ContentHint`/`ContentPurpose`
//!   へマップされる点）、
//!   `src/platform_impl/linux/wayland/window/mod.rs`（`set_ime_allowed` が
//!   `Window::set_ime_allowed` 呼び出しに同期して即座に `Ime::Enabled`/`Ime::Disabled` を
//!   発火する経路）
//!
//! # winit Linux バックエンドが実際に発火するイベント列（ソース照合で確定）
//!
//! - **`Enabled`/`Disabled` の発火タイミング**: X11・Wayland いずれも、macOS/Windows
//!   （コンポジション開始/終了まで遅延発火）とは**逆方向に、同期的・即時**に発火する。
//!   - X11: `Ime::create_context`（`ime/mod.rs` L134）が IM コンテキスト生成時に
//!     即座に `Enabled`/`Disabled` を送出し、`set_ime_allowed`（同 L197-213）は
//!     要求された allowed 状態が現在と異なればコンテキストを作り直して同様に送出する
//!     （フォーカス獲得の遅延を待たない）。
//!   - Wayland: `Window::set_ime_allowed`（`window/mod.rs` L613-621）は
//!     `window_state.set_ime_allowed()` の呼び出しに続けてその場で `Ime::Enabled`/
//!     `Ime::Disabled` を events sink へ積む。加えて `Enter`/`Leave`（同ファイル
//!     `text_input/mod.rs` L65-102）でもフォーカス変化に同期して発火する。
//!
//!   これは Windows（変換確定ごとに `Disabled`→`Enabled` が高頻度に再サイクルする、S12 所見）
//!   とも異なる第三のパターンであり、`TextFieldWidget::apply_ime` の `Disabled` 処理が
//!   冪等（`text_field.rs` §4.6）であることに変わりはないため、この同期発火パターンも
//!   既存の状態モデルのまま許容できることを本ファイルで確認する。
//! - **X11 の preedit カーソルは常に単一点**: `event_processor.rs` L112-114
//!   （`ImeEvent::Update(text, position) => Ime::Preedit(text, Some((position, position)))`）
//!   により、X11 経由では `cursor.0 == cursor.1` の選択範囲なしカーソルのみが渡る
//!   （Windows・Wayland は範囲選択を表現しうる点と対照的）。
//! - **X11 の `Commit` は 2 経路**: 変換確定時は `preedit_done_callback` による
//!   `ImeEvent::End`（空 `Preedit` のみ、`context.rs` L48-64）と、実際のキー入力処理
//!   （`XLookupUtf8`、`event_processor.rs` L992-1011）による「空 Preedit → Commit」が
//!   連続して発火しうる。`ime_event_from_winit` は 1 イベントずつ独立処理するため、
//!   空 `Preedit` が連続しても `TextFieldWidget` 側の冪等性で問題ない
//!   （既存 `apply_ime` の空 Preedit 処理を参照）。
//! - **Wayland の `Done` バッチ適用順序**: `text_input/mod.rs` L118-150 により、
//!   1 回の `Done` で「（保留 commit があるか、次の preedit がなければ）空 Preedit →
//!   Commit → 次の Preedit」の順に最大 3 イベントが連続送出されうる。
//! - **`set_ime_purpose`（`SetPurpose` 要求）の非対称性**: X11 は macOS・Windows と
//!   同様に no-op（`Window::set_ime_purpose` 公式 doc「Unsupported: ... X11 ...」）だが、
//!   **Wayland のみ** `set_content_type_by_purpose`（`text_input/mod.rs` L160-173）経由で
//!   実際に `ContentHint::SensitiveData` / `ContentPurpose::Password` へマップされる
//!   （デスクトップ 3 プラットフォーム中で唯一の有効経路）。
//!
//! `winit::event::Ime` は環境依存のフィールドを持たないプレーンな enum のため、
//! 実ウィンドウ・実 IME コンテキスト（XIM/ibus・zwp_text_input_v3/fcitx）なしで
//! 値を直接構築してテストできる（ウィンドウハンドルは `ImeWindow` トレイト経由の
//! `apply_ime_request` 側でのみ必要）。

use fandhe_core::{Event, ImeRequest, InteractionInput, TextFieldWidget, WidgetId};
use fandhe_winit_ime::ime_event_from_winit;
use winit::event::Ime;

/// `TextFieldWidget` をフォーカス状態にする（`windows_japanese_input.rs` の
/// `focused_field` ヘルパーと同一操作を公開 API 経由で行う）。
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

// --- シナリオ 1: X11(XIM) 経由の「にほんご」ローマ字入力 → 変換 → 確定 -------------

/// X11/XIM（ibus・fcitx が XIM サーバーとして連携する経路）での「にほんご」ローマ字
/// 入力 → スペース変換（「日本語」）→ Enter 確定シナリオ。
/// `preedit_start_callback` → `Enabled`（`ime/mod.rs` L134 で IM コンテキスト生成時に
/// 既に `Enabled` 済みの前提）→ `preedit_draw_callback` 経由の逐次 `Preedit` 更新
/// （常に単一点カーソル、`event_processor.rs` L112-114）→ `preedit_done_callback` に
/// よる空 `Preedit` → キー入力経由の空 `Preedit` + `Commit`（`event_processor.rs`
/// L997-1009）という、X11 ソース照合で確定した発火順に沿って `apply_ime` へ流す。
#[test]
fn x11_nihongo_romaji_input_converts_and_commits() {
    let mut field = focused_field("");

    // IM コンテキスト生成時に同期発火する Enabled（ime/mod.rs L134）。
    assert_eq!(apply(&mut field, Ime::Enabled), None);

    // ローマ字タイプ中の逐次 Preedit 更新。X11 の cursor は常に単一点
    // （event_processor.rs L112-114 の `Some((position, position))`）。
    for partial in ["に", "にほ", "にほん", "にほんご"] {
        let end = partial.len();
        apply(
            &mut field,
            Ime::Preedit(partial.to_string(), Some((end, end))),
        );
    }
    assert_eq!(field.buffer(), "", "確定前は buffer へ反映しない（§4.3）");

    // スペースキーによる変換候補選択（引き続き preedit_draw_callback 経由）。
    let kanji = "日本語";
    apply(
        &mut field,
        Ime::Preedit(kanji.to_string(), Some((kanji.len(), kanji.len()))),
    );

    // Enter 確定: preedit_done_callback による空 Preedit（context.rs L48-64）に続き、
    // キー入力処理（lookup_utf8）による空 Preedit → Commit が発火する
    // （event_processor.rs L997-1009）。空 Preedit の連続送出も冪等に扱える。
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Preedit(String::new(), None));
    let event = apply(&mut field, Ime::Commit(kanji.to_string()));

    assert_eq!(field.buffer(), "日本語");
    assert_eq!(field.cursor(), 3);
    assert!(
        event.is_none(),
        "Commit 自体は TextFieldChanged を発火しない（blur-commit のみが発火源、§4.5）"
    );
}

// --- シナリオ 2: 変換中の Esc キャンセル ---------------------------------------

/// 変換中に Esc でキャンセルした場合、X11 では `preedit_done_callback`
/// （`ImeEvent::End` → 空 `Preedit`、`context.rs` L48-64）のみが発火し、
/// `lookup_utf8` によるキー入力経路は走らないため `Commit` は送出されない。
/// `buffer` が不変であることを確認する。
#[test]
fn x11_escape_during_composition_cancels_without_commit() {
    let mut field = focused_field("既存値");

    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("にほんご".to_string(), Some((0, 0))),
    );

    // preedit_done_callback: 確定文字列を伴わない空 Preedit のみ。
    let event = apply(&mut field, Ime::Preedit(String::new(), None));

    assert_eq!(field.buffer(), "既存値", "キャンセルで buffer は変化しない");
    assert!(event.is_none());
}

// --- シナリオ 3: 複文節変換でのマルチバイト境界（X11 単一点カーソル） -----------------

/// X11 の `Ime::Preedit` カーソルは常に `(position, position)` の単一点
/// （`event_processor.rs` L112-114）であり、Windows/Wayland のような範囲選択を
/// 表現しない。マルチバイト文字列上でこの単一点カーソルが `char_offset_from_byte`
/// により panic せず妥当な char offset へ変換されることを確認する。
#[test]
fn x11_multi_clause_conversion_uses_single_point_cursor_across_multibyte_boundaries() {
    let mut field = focused_field("");
    apply(&mut field, Ime::Enabled);

    // 「きょうははれ」相当。文節境界（「きょう」= 3 文字 × 3 byte）上にカーソルを置く。
    let text = "きょうははれ".to_string();
    let clause_end = "きょう".len();
    apply(
        &mut field,
        Ime::Preedit(text.clone(), Some((clause_end, clause_end))),
    );
    // Tab 等で変換対象文節を後方へ移す（引き続き単一点）。
    let next_end = clause_end + "はは".len();
    apply(
        &mut field,
        Ime::Preedit(text.clone(), Some((next_end, next_end))),
    );

    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit(text.clone()));

    assert_eq!(field.buffer(), text);
    assert_eq!(field.cursor(), text.chars().count());
}

// --- シナリオ 4: Wayland `zwp_text_input_v3` の `Done` バッチ適用 -------------------

/// Wayland（ibus/fcitx がコンポジタの `zwp_text_input_v3` 実装経由で連携する経路）の
/// `Done` イベントで「空 Preedit → Commit → 次 Preedit」が 1 回のバッチとして連続
/// 送出されるケース（`text_input/mod.rs` L118-150）を再現する。前の文節の確定と
/// 次の文節の変換開始が同一フレームで連続しても、`apply_ime` が 1 イベントずつ独立に
/// 処理するため状態モデルが破綻しないことを確認する。
#[test]
fn wayland_done_batch_applies_commit_then_next_preedit_in_sequence() {
    let mut field = focused_field("");

    // Enter（フォーカス、text_input/mod.rs L65-79）で同期発火する Enabled。
    apply(&mut field, Ime::Enabled);

    // 文節 1「きょう」の変換中。
    apply(
        &mut field,
        Ime::Preedit("きょう".to_string(), Some((0, "きょう".len()))),
    );

    // Done バッチ: pending_commit が Some のため空 Preedit → Commit → 次の Preedit
    // の順で 3 イベントが連続送出される（text_input/mod.rs L124-150）。
    apply(&mut field, Ime::Preedit(String::new(), None));
    let commit_event = apply(&mut field, Ime::Commit("今日".to_string()));
    apply(
        &mut field,
        Ime::Preedit("は".to_string(), Some((0, "は".len()))),
    );

    assert_eq!(
        field.buffer(),
        "今日",
        "Done バッチ内の Commit が buffer へ反映される"
    );
    assert!(commit_event.is_none());

    // 続く「は」の変換を確定して最終状態を確認する。
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("は".to_string()));
    assert_eq!(field.buffer(), "今日は");
}

// --- シナリオ 5: ibus 再起動相当の Disabled → Enabled 再サイクル --------------------

/// ibus デーモン再起動相当のシナリオ。X11 では IM サーバー切断時に
/// `xim_destroy_callback`（`callbacks.rs` L184-206）が発火し、フォールバック IM への
/// `replace_im` で全コンテキストが再生成される（`callbacks.rs` L91-155）。
/// この间、変換中であった composition は失われる（サーバー側のリソースが破棄される
/// ため）。本シナリオは「変換中に Disabled が届き、その後 Enabled で再開する」という
/// 呼び出し側から見た契約を、コンポジション状態が問題なく破棄・再開されることで確認する。
#[test]
fn ibus_restart_disabled_enabled_recycle_discards_pending_composition() {
    let mut field = focused_field("");

    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("へんかんちゅう".to_string(), Some((0, 0))),
    );
    assert_eq!(field.buffer(), "", "変換中は buffer 未反映");

    // ibus 再起動により IM サーバーが破棄され、xim_destroy_callback 経由で
    // コンテキストが失われる（Commit を経由しない Disabled のみ）。
    apply(&mut field, Ime::Disabled);
    assert_eq!(
        field.buffer(),
        "",
        "サーバー再起動で未確定の変換は破棄され buffer は不変"
    );

    // replace_im によるフォールバック IM 再接続後、新規コンテキスト生成時の
    // Enabled（ime/mod.rs L134）で再開する。
    apply(&mut field, Ime::Enabled);
    apply(&mut field, Ime::Preedit("あ".to_string(), Some((0, 0))));
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("あ".to_string()));

    assert_eq!(field.buffer(), "あ", "再接続後の変換は正しく確定される");
}

// --- シナリオ 6: 変換中のフォーカス喪失 -----------------------------------------

/// 変換中に別フィールドへフォーカスが移動した場合、`ime-architecture.md` §4.6 の
/// フォーカスガードにより `TextFieldWidget::apply_input(FocusLost)` が composition を
/// **確定せずローカルに破棄**し、`take_ime_requests()` へ `ImeRequest::SetEnabled(false)`
/// を積むことを確認する（`windows_japanese_input.rs` のシナリオ 5 と同型。Wayland では
/// これに加えて実際の `Leave` イベントでも `Ime::Disabled` が同期発火するが
/// （`text_input/mod.rs` L83-101）、フォーカス遷移そのものはコア側 `InteractionInput`
/// 経由であり、winit イベントの発火有無に関わらずこのガードが成立する必要がある）。
#[test]
fn focus_lost_during_composition_discards_without_commit() {
    let mut field = focused_field("");
    // フォーカス取得時の SetEnabled(true) 要求を drain し、本シナリオが検証したい
    // FocusLost 由来の要求のみを対象にする。
    field.take_ime_requests();

    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("にほんご".to_string(), Some((0, 0))),
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

// --- シナリオ 7: サロゲートペア文字の確定 ---------------------------------------

/// 補助多言語面（サロゲートペア）の文字（例: 「𩸽」U+29E3D、鰤の異体字）の確定を
/// Linux 経路（Wayland の `CommitString`、X11 の `lookup_utf8` いずれも同一の
/// `Ime::Commit(String)` として届く）で検証する。`String`/`char` は UTF-8 内部表現の
/// ため、winit 側で UTF-16 サロゲートペア相当は 1 つの `char` へ正しく変換済みの前提だが、
/// `fandhe-core` 側の char 単位カーソル計算（`chars().count()`）がこれを 1 として数える
/// ことを確認する（S26 #207 の網羅検証はスコープ外、ここでは代表 1 ケースのみ）。
#[test]
fn commits_supplementary_plane_character() {
    let mut field = focused_field("前");

    apply(&mut field, Ime::Enabled);
    apply(&mut field, Ime::Preedit("𩸽".to_string(), Some((0, 0))));
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("𩸽".to_string()));

    assert_eq!(field.buffer(), "前𩸽");
    assert_eq!(
        field.cursor(),
        2,
        "サロゲートペア相当の 1 文字はカーソル 1 と数える"
    );
}
