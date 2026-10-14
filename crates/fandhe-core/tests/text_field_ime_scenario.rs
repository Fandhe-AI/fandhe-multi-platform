//! `TextFieldWidget<S>` × IME のライフサイクルを横断する結合テスト
//! （TASK-8.5-S28 / REQ-8、Issue #209）。
//!
//! 親 issue #181（TASK-8.5「IME 統合実装」）の sub-issue 28/30。
//! `docs/architecture/ime-architecture.md` §1 の分担表で、本 issue は
//! 「S7〜S24 の統合をテストする」と定義されている。
//!
//! ## 既存テストとの分担（重複回避の判断根拠）
//!
//! - `crates/fandhe-core/src/ui/text_field.rs` 内 `#[cfg(test)] mod tests`（S14・S27）は
//!   状態モデル・コンポジション単体の**ユニットテスト**（§4.3 遷移表・§5 不変条件）を
//!   網羅済み。
//! - `crates/fandhe-core/tests/text_input_widget.rs`（S15）は `ime_composition_lifecycle_
//!   reaches_state_store_via_public_api` で単一フィールドの E2E（フォーカス → Preedit →
//!   Commit → blur-commit → `StateStore::dispatch`）を既に固定化済み。
//! - `crates/fandhe-winit-ime/tests/` は S10〜S25 のプラットフォーム別**イベント列の
//!   再現**（macOS/Windows/Linux/iOS/Android・韓国語中国語）を担当する。
//!
//! 本ファイルは上記いずれとも異なる軸、すなわち「複数の公開 API 呼び出しを跨ぐ
//! 遷移の結合」（複数フィールド間のフォーカス遷移・Password purpose 切替との
//! 相互作用・Enabled/Disabled 再サイクル耐性・disabled との干渉）に集中する。
//! 単一フィールドのハッピーパス（シナリオ 1）は E2E 経路の存在確認として
//! 代表ケースのみ再掲し、詳細な分岐網羅は上記既存テストに委ねる。
//!
//! ## 前提（複数フィールド間の IME 調停の扱い）
//!
//! 複数フィールド間の IME 調停（S24、Issue #205）専用の調停 API は存在しないため、
//! シナリオ 4 は各フィールドの `TextFieldWidget<S>` per-widget 公開 API のみを用いて、
//! アプリ側が両フィールドの `apply_input` / `take_ime_requests` を呼び分ける
//! 現状の契約を固定化する（調停 API が追加された場合はその受け入れ条件側で
//! 別途結合テストを検討する、`.claude/rules/out-of-scope-tracking.md` 準拠）。

use fandhe_core::ui::{ImeCursorAreaParams, ime_cursor_area};
use fandhe_core::{
    Event, ImeEvent, ImePurpose, ImeRequest, InteractionInput, PaintCommand, Screen, StateStore,
    TextFieldInput, TextFieldWidget, Widget, WidgetId,
};

/// フォーム状態。`StateStore::dispatch` まで貫通させるシナリオ（1）でのみ使う
/// （シナリオ 4 の複数フィールドは `StateStore` を介さず widget 単体で比較するため
/// 2 つ目のフィールド用の状態は不要）。
#[derive(Default)]
struct FormState {
    name: String,
}

fn get_name(state: &FormState) -> String {
    state.name.clone()
}

fn set_name(state: &mut FormState, value: String) {
    state.name = value;
}

/// `TextFieldWidget<S>` をフォーカス状態にする（`text_input_widget.rs` の
/// `focus` ヘルパーと同じ操作を公開 API 経由で行う）。
fn focus(field: &mut TextFieldWidget<FormState>) {
    field.apply_input(InteractionInput::FocusGained);
}

// --- シナリオ 1: E2E ハッピーパス（状態ストアまで貫通） ---------------------

/// フォーカス獲得 → 多段 Preedit → 空 Preedit → Commit → フォーカス喪失（blur-commit）
/// → `StateStore::dispatch` までの一気通貫の流れを、クレート外部の公開 API のみで
/// 固定化する。単一フィールドの E2E 経路自体は `text_input_widget.rs` の
/// `ime_composition_lifecycle_reaches_state_store_via_public_api` と同一観点だが、
/// 本ファイルの以降のシナリオが前提とする基本経路であることを明示するため、
/// 多段 Preedit（かな → 漢字変換）を伴う代表ケースとして再掲する。
#[test]
fn happy_path_reaches_state_store_after_focus_preedit_commit_and_blur() {
    let screen: Screen<FormState> = Screen::new("form").text_field("name", get_name, set_name);
    let mut store = StateStore::new(FormState::default());
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(1), "name", "");

    focus(&mut field);
    assert_eq!(
        field.take_ime_requests(),
        vec![
            ImeRequest::SetPurpose(ImePurpose::Normal),
            ImeRequest::SetEnabled(true),
        ],
        "フォーカス獲得は SetPurpose → SetEnabled(true) の順で要求する"
    );

    // 多段変換: 「にほんご」→「日本語」。
    field.apply_ime(ImeEvent::Preedit {
        text: "にほんご".to_string(),
        cursor: Some((0, 4)),
    });
    assert!(field.is_composing());
    field.apply_ime(ImeEvent::Preedit {
        text: "日本語".to_string(),
        cursor: Some((0, 3)),
    });
    assert_eq!(field.buffer(), "", "preedit は確定バッファへ混入しない");

    // 空 Preedit はコンポジションのクリアを表す（確定は伴わない）。
    field.apply_ime(ImeEvent::Preedit {
        text: String::new(),
        cursor: None,
    });
    assert!(!field.is_composing());

    field.apply_ime(ImeEvent::Commit("日本語".to_string()));
    assert_eq!(field.buffer(), "日本語");
    assert!(!field.is_composing());

    let event = field.apply_input(InteractionInput::FocusLost);
    assert_eq!(
        field.take_ime_requests(),
        vec![ImeRequest::SetEnabled(false)],
        "フォーカス喪失で IME 無効化のみを要求する"
    );

    let event = event.expect("確定挿入により dirty のため blur-commit が発火する");
    assert!(store.dispatch(&screen, &event));
    assert_eq!(store.state().name, "日本語");
    assert_eq!(store.version(), 1);
}

// --- シナリオ 2: 確定済みテキストとコンポジションの合成表示 -------------------

/// 既存 buffer の中間位置にカーソルを置いた状態で Preedit を適用すると、
/// `paint` が `PaintCommand::CompositionText { before, preedit, after, .. }` として
/// `buffer[..anchor] ⊕ preedit ⊕ buffer[anchor..]` の合成を返し、`measure` が
/// panic しないことを固定化する（`text-input-state-model.md` §4.4、
/// `composition_display.rs` の配線を `TextFieldWidget` 経由で確認する）。
#[test]
fn composition_synthesizes_display_between_committed_prefix_and_suffix() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(2), "name", "ABCDE");
    focus(&mut field);
    field.handle_input(TextFieldInput::MoveCursorToStart);
    field.handle_input(TextFieldInput::MoveCursorRight);
    field.handle_input(TextFieldInput::MoveCursorRight);

    field.apply_ime(ImeEvent::Preedit {
        text: "xyz".to_string(),
        cursor: Some((0, 3)),
    });

    let available = fandhe_core::Measurement {
        width: 1000.0,
        height: 100.0,
    };
    let measured = field.measure(available);
    assert!(measured.width > 0.0, "measure は panic せず正の幅を返す");

    let commands = field.paint((0.0, 0.0), measured);
    let composition = commands
        .into_iter()
        .find_map(|command| match command {
            PaintCommand::CompositionText {
                before,
                preedit,
                after,
                ..
            } => Some((before, preedit, after)),
            _ => None,
        })
        .expect("paint はコンポジション中に CompositionText を含む");

    assert_eq!(
        composition,
        ("AB".to_string(), "xyz".to_string(), "CDE".to_string()),
        "確定済みプレフィックス・preedit・確定済みサフィックスの合成"
    );
}

// --- シナリオ 3: コンポジション中のフォーカス喪失 ---------------------------

/// composing 中に `FocusLost` すると preedit は確定されずに破棄され、確定済みの
/// dirty な編集のみが blur-commit で発火し、`take_ime_requests()` に
/// `SetEnabled(false)` が積まれることを固定化する。その後に届く遅延 `Commit` は
/// 非フォーカスガードで破棄され、`buffer` は不変のままであることも確認する
/// （`text-input-state-model.md` §4.3・§4.6 のフォーカスガード）。
#[test]
fn focus_lost_during_composition_discards_preedit_but_keeps_dirty_blur_commit() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(3), "name", "");
    focus(&mut field);
    field.take_ime_requests();
    field.handle_input(TextFieldInput::InsertText("下書き".to_string()));
    field.apply_ime(ImeEvent::Preedit {
        text: "へんかんちゅう".to_string(),
        cursor: Some((0, 7)),
    });
    assert!(field.is_composing());

    let event = field.apply_input(InteractionInput::FocusLost);

    assert!(!field.is_composing(), "非確定の preedit は破棄される");
    assert_eq!(
        event,
        Some(Event::TextFieldChanged {
            label: "name".to_string(),
            value: "下書き".to_string(),
        }),
        "確定済みの dirty な編集のみが blur-commit として発火する"
    );
    assert_eq!(
        field.take_ime_requests(),
        vec![ImeRequest::SetEnabled(false)]
    );

    // 遅延到着する Commit（Windows の WM_IME_ENDCOMPOSITION 相当）は非フォーカス
    // ガードで破棄され、buffer は変化しない。
    let delayed = field.apply_ime(ImeEvent::Commit("へんかんちゅう".to_string()));
    assert_eq!(delayed, None);
    assert_eq!(field.buffer(), "下書き");
}

// --- シナリオ 4: 複数フィールド間のフォーカス遷移（S24 相当の現状契約） -------

/// フィールド A（composing 中）→ A へ `FocusLost`、B へ `FocusGained` という
/// 典型的なタブ移動シナリオで、各フィールドの `take_ime_requests()` を連結した
/// 要求列が「A: `SetEnabled(false)` → B: `SetPurpose` → B: `SetEnabled(true)`」の
/// 順になること（アプリ側が両ウィジェットを順に処理する現状の契約）、
/// A 宛の遅延 `Commit` が B の buffer に混入しないこと、Normal → Password の
/// purpose 切替が正しく行われることを固定化する。
///
/// S24（#205）の専用調停 API は存在しないため、per-widget API のみで構成する
/// （モジュール doc「前提」参照）。
#[test]
fn focus_transition_between_two_fields_sequences_requests_and_isolates_delayed_commit() {
    let mut field_a: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(4), "name", "");
    let mut field_b: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(5), "password", "");
    // 非フォーカス中の purpose 設定はキューに積まれるが、本シナリオでは
    // FocusGained が改めて SetPurpose を要求するため、事前設定分は破棄しておく
    // （非フォーカス中の set_ime_purpose の要求内容自体はシナリオ 6 で固定化済み）。
    field_b.set_ime_purpose(ImePurpose::Password);
    field_b.take_ime_requests();

    focus(&mut field_a);
    field_a.take_ime_requests();
    field_a.handle_input(TextFieldInput::InsertText("あ".to_string()));
    field_a.apply_ime(ImeEvent::Preedit {
        text: "compose".to_string(),
        cursor: Some((0, 7)),
    });
    assert!(field_a.is_composing());

    // タブ移動: A の FocusLost → B の FocusGained をこの順でアプリ側が処理する。
    field_a.apply_input(InteractionInput::FocusLost);
    focus(&mut field_b);

    let mut requests = field_a.take_ime_requests();
    requests.extend(field_b.take_ime_requests());
    assert_eq!(
        requests,
        vec![
            ImeRequest::SetEnabled(false),
            ImeRequest::SetPurpose(ImePurpose::Password),
            ImeRequest::SetEnabled(true),
        ],
        "A の無効化要求 → B の purpose 通知 → B の有効化要求という順序を固定化する"
    );
    assert!(!field_a.is_composing());

    // A 宛の遅延 Commit は A の非フォーカスガードで破棄され、B の buffer には
    // 一切影響しない（ウィジェットごとに独立した状態を持つことの確認）。
    let delayed = field_a.apply_ime(ImeEvent::Commit("compose".to_string()));
    assert_eq!(delayed, None);
    assert_eq!(field_a.buffer(), "あ");
    assert_eq!(field_b.buffer(), "");

    // B（Password）は通常どおり入力できる。
    field_b.apply_ime(ImeEvent::Commit("secret".to_string()));
    assert_eq!(field_b.buffer(), "secret");
    assert_eq!(field_b.ime_purpose(), ImePurpose::Password);
}

// --- シナリオ 5: Enabled/Disabled 再サイクル耐性 -----------------------------

/// フォーカス継続中に `Commit` → `Disabled` → `Enabled` → 次の `Preedit` という
/// ループを複数回廻しても、`buffer` が累積し `ime_enabled()` の観測値・
/// `is_composing()` が一貫することを固定化する（Windows の高頻度再サイクル契約、
/// `ime.rs` の `ImeEvent::Enabled`/`Disabled` doc 参照）。プラットフォーム別
/// テストがイベント列の再現を担うのに対し、こちらは widget 状態の一貫性を担う。
#[test]
fn enabled_disabled_recycle_while_focused_keeps_widget_state_consistent() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(6), "name", "");
    focus(&mut field);
    field.apply_ime(ImeEvent::Enabled);
    assert!(field.ime_enabled());

    let syllables = ["あ", "い", "う"];
    for syllable in syllables {
        field.apply_ime(ImeEvent::Preedit {
            text: syllable.to_string(),
            cursor: Some((0, 1)),
        });
        assert!(field.is_composing());

        field.apply_ime(ImeEvent::Commit(syllable.to_string()));
        assert!(
            !field.is_composing(),
            "Commit 後は composition が必ずクリアされる"
        );

        // Windows は変換確定のたびに Disabled → Enabled を再サイクルする契約。
        field.apply_ime(ImeEvent::Disabled);
        assert!(!field.ime_enabled());
        assert!(!field.is_composing());

        field.apply_ime(ImeEvent::Enabled);
        assert!(field.ime_enabled());
    }

    assert_eq!(
        field.buffer(),
        "あいう",
        "3 回のループ全ての確定分が累積する"
    );
    assert!(field.ime_enabled());
    assert!(!field.is_composing());
}

// --- シナリオ 6: Password purpose 切替のワンショットガード -------------------

/// composing 中に `set_ime_purpose(Password)` へ切り替えると、直後の遅延 `Commit` は
/// 1 件のみ飲み込まれ、2 件目以降の正規 `Commit` は反映されることを固定化する。
/// `take_ime_requests()` に `SetPurpose(Password)` が含まれ、`SetEnabled(false)` は
/// 含まれない（`ime.rs` の `SetPurpose` doc が定めるポリシー）。
#[test]
fn password_purpose_switch_guards_exactly_one_delayed_commit() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(7), "password", "");
    focus(&mut field);
    field.take_ime_requests();
    field.apply_ime(ImeEvent::Preedit {
        text: "secret-preview".to_string(),
        cursor: Some((0, 14)),
    });
    assert!(field.is_composing());

    field.set_ime_purpose(ImePurpose::Password);

    assert!(
        !field.is_composing(),
        "purpose 切替で進行中の preedit を破棄する"
    );
    // Password 切替は SetEnabled を要求しない（ソフトキーボードを隠さない）。
    // 完全一致の assert_eq! により、SetEnabled(false) が含まれないことも
    // 併せて固定化される。
    let requests = field.take_ime_requests();
    assert_eq!(requests, vec![ImeRequest::SetPurpose(ImePurpose::Password)]);

    // 1 件目の遅延 Commit（破棄した preedit の遅延応答）は飲み込まれる。
    let first = field.apply_ime(ImeEvent::Commit("secret-preview".to_string()));
    assert_eq!(first, None);
    assert_eq!(field.buffer(), "");

    // 2 件目以降は正規の入力として反映される。
    let second = field.apply_ime(ImeEvent::Commit("s3cr3t".to_string()));
    assert_eq!(second, None, "Commit 自体は TextFieldChanged を発火しない");
    assert_eq!(field.buffer(), "s3cr3t");
}

// --- シナリオ 7: disabled との干渉 ------------------------------------------

/// composing 中の `set_disabled(true)` はコンポジションを破棄し
/// `ImeRequest::SetEnabled(false)` を発行し、以後の `ImeEvent` / `handle_input` を
/// 無視することを固定化する。`set_disabled(false)` 後の再フォーカスで正常に
/// 復帰することも確認する。
#[test]
fn set_disabled_during_composition_discards_and_blocks_until_reenabled() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(8), "name", "");
    focus(&mut field);
    field.handle_input(TextFieldInput::InsertText("途中".to_string()));
    field.apply_ime(ImeEvent::Preedit {
        text: "compose".to_string(),
        cursor: Some((0, 7)),
    });
    assert!(field.is_composing());

    let event = field.set_disabled(true);
    assert!(!field.is_composing(), "disable はコンポジションを破棄する");
    assert_eq!(
        event,
        Some(Event::TextFieldChanged {
            label: "name".to_string(),
            value: "途中".to_string(),
        }),
        "disable 前に dirty な編集を blur-commit する"
    );
    assert!(
        field
            .take_ime_requests()
            .contains(&ImeRequest::SetEnabled(false))
    );

    // disabled 中は ImeEvent も handle_input も無視される。
    let ignored_ime = field.apply_ime(ImeEvent::Commit("無視される".to_string()));
    assert_eq!(ignored_ime, None);
    let ignored_input = field.handle_input(TextFieldInput::InsertText("無視".to_string()));
    assert_eq!(ignored_input, None);
    assert_eq!(field.buffer(), "途中");

    // 再有効化 → 再フォーカスで正常復帰する。
    field.set_disabled(false);
    focus(&mut field);
    field.handle_input(TextFieldInput::InsertText("再開".to_string()));
    assert_eq!(field.buffer(), "途中再開");
}

// --- シナリオ 8: 機微情報の非露出（結合レベル） ------------------------------

/// composing 中・Commit 後の `TextFieldWidget` / `ImeEvent` の `Debug` 出力に
/// preedit・確定文字列の内容が現れないことを結合レベルで固定化する
/// （`.claude/rules/security.md` 「ログにシークレットを出力しない」）。
#[test]
fn debug_output_never_exposes_preedit_or_committed_secret_content() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(9), "password", "");
    focus(&mut field);
    field.set_ime_purpose(ImePurpose::Password);

    let preedit_event = ImeEvent::Preedit {
        text: "hunter2".to_string(),
        cursor: Some((0, 7)),
    };
    let preedit_debug = format!("{preedit_event:?}");
    assert!(!preedit_debug.contains("hunter2"));

    field.apply_ime(preedit_event);
    let composing_debug = format!("{field:?}");
    assert!(!composing_debug.contains("hunter2"));

    let commit_event = ImeEvent::Commit("hunter2".to_string());
    let commit_debug = format!("{commit_event:?}");
    assert!(!commit_debug.contains("hunter2"));

    field.apply_ime(commit_event);
    assert_eq!(field.buffer(), "hunter2");
    let committed_debug = format!("{field:?}");
    assert!(
        !committed_debug.contains("hunter2"),
        "確定後の Debug も buffer の内容を出力しない"
    );
}

// --- シナリオ 9: ime_cursor_area との整合（参考軸・最小限） -------------------

/// シナリオ 2 と同じ「中間位置へのカーソル + preedit」入力から `ime_cursor_area`
/// （純関数、`TextFieldWidget` への配線は S8 でスコープ外確定）が有限の論理ピクセル
/// 矩形を返すことを確認する（`text_field.rs::measure` の暫定計測と対応する
/// `ime_cursor_area.rs` の CHAR_WIDTH/LINE_HEIGHT 前提を、実際の `TextFieldWidget`
/// から得た値で参考程度に検証する）。
#[test]
fn ime_cursor_area_returns_finite_rect_for_mid_buffer_composition() {
    let mut field: TextFieldWidget<FormState> =
        TextFieldWidget::new_static(WidgetId::from_raw(10), "name", "ABCDE");
    focus(&mut field);
    field.handle_input(TextFieldInput::MoveCursorToStart);
    field.handle_input(TextFieldInput::MoveCursorRight);
    field.handle_input(TextFieldInput::MoveCursorRight);
    field.apply_ime(ImeEvent::Preedit {
        text: "xyz".to_string(),
        cursor: Some((1, 2)),
    });

    let params = ImeCursorAreaParams {
        field_origin: fandhe_core::ui::Point::new(10.0, 20.0),
        field_size: fandhe_core::ui::Size::new(200.0, 40.0),
        buffer: field.buffer(),
        anchor: 2,
        preedit: "xyz",
        preedit_cursor: Some((1, 2)),
    };
    let rect = ime_cursor_area(&params);

    assert!(rect.size.width.is_finite() && rect.size.width >= 0.0);
    assert!(rect.size.height.is_finite() && rect.size.height >= 0.0);
    assert!(rect.origin.x.is_finite());
    assert!(rect.origin.y.is_finite());
}
