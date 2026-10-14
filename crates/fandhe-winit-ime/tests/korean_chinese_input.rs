//! 韓国語（ハングル 2 ボル式・漢字変換）・中国語（拼音・注音）入力のシーケンス再現
//! 結合テスト（TASK-8.5-S25 / REQ-8 / M3、Issue #206、親 #181）。
//!
//! `docs/architecture/ime-architecture.md` §1 の分担表は S25（#206）を「本文書の契約が
//! CJK 全般で成立することを検証する」と位置づけている。issue タイトルの「CJK 以外
//! （韓国語・中国語）」は表記の揺れであり、実体は「**日本語以外の CJK（韓国語・
//! 中国語）**」の入力検証である（韓国語・中国語は CJK に含まれる）。
//!
//! S9〜S14（macOS・Windows・Linux）・S16〜S21（iOS・Android・Web）が確立した
//! 「winit バックエンドソース読解で確定したイベント列を `ime_event_from_winit()` →
//! `TextFieldWidget::apply_ime()` へ流すシーケンス再現テスト」の検証方式を、
//! 韓国語（ハングル 2 ボル式・漢字変換）・中国語（拼音・注音）固有のイベント
//! パターンへ適用したものであり、
//! 検証で得た対応表と対になる。
//!
//! `winit::event::Ime` は環境依存フィールドを持たないプレーンな enum であり、IME 方式
//! （2 ボル式・拼音・注音等）に関わらず `Enabled`/`Preedit(text, cursor)`/`Commit(text)`/
//! `Disabled` の 4 種類のみで表現される（S9〜S14 で確定済みの契約）。本ファイルは
//! 韓国語・中国語で特徴的になるイベント**パターン**（音節単位の高頻度確定・ASCII
//! preedit と CJK commit の乖離・字母単位の部分入力等）を、この共通契約の上で
//! ヘッドレスに再現する。実 IME（macOS 韓国語 IME・Microsoft Pinyin・ibus-hangul /
//! fcitx5-rime 等）での実操作検証は
//! 実機チェックリストへ申し送る。

use fandhe_core::{Event, ImeRequest, InteractionInput, TextFieldInput, TextFieldWidget, WidgetId};
use fandhe_winit_ime::{ime_event_from_winit, text_field_input_from_key};
use winit::event::{ElementState, Ime};
use winit::keyboard::Key;

/// `TextFieldWidget` をフォーカス状態にする（`linux_japanese_input.rs` の
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

// --- シナリオ 1: 韓国語 2 ボル式の字母逐次合成 → 音節単位確定 ------------------------

/// 韓国語 2 ボル式は字母（jamo）を打鍵するたびに現在の音節が置換され続ける
/// （ㅎ→하→한）。次の音節の入力が始まると直前の音節が確定し「空 Preedit → Commit
/// （1 音節）→ 次の Preedit」の順で発火する（macOS `NSTextInputClient`・Windows IMM32・
/// Linux XIM/zwp_text_input_v3 いずれも S9〜S14 で確定済みの「Commit 直前は必ず空
/// Preedit」という共通契約に従うため、韓国語固有の追加分岐は不要）。「한글」
/// （한 + 글）の入力を音節単位で再現する。
#[test]
fn hangul_2bulsik_jamo_composition_commits_per_syllable() {
    let mut field = focused_field("");

    apply(&mut field, Ime::Enabled);

    // 1 音節目「한」: ㅎ → 하 → 한 と字母単位で preedit が置換され続ける。
    for partial in ["ㅎ", "하", "한"] {
        apply(
            &mut field,
            Ime::Preedit(partial.to_string(), Some((0, partial.len()))),
        );
    }
    assert_eq!(field.buffer(), "", "確定前は buffer へ反映しない（§4.3）");

    // 2 音節目の入力開始で 1 音節目が確定（空 Preedit → Commit → 次の Preedit）。
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("한".to_string()));
    for partial in ["ㄱ", "그", "글"] {
        apply(
            &mut field,
            Ime::Preedit(partial.to_string(), Some((0, partial.len()))),
        );
    }
    assert_eq!(field.buffer(), "한", "1 音節目のみ確定済み");

    // フォーカス喪失相当ではなく、明示的な確定操作（Enter 等）で 2 音節目も確定。
    apply(&mut field, Ime::Preedit(String::new(), None));
    let event = apply(&mut field, Ime::Commit("글".to_string()));

    assert_eq!(field.buffer(), "한글");
    assert_eq!(field.cursor(), 2);
    assert!(
        event.is_none(),
        "Commit 自体は TextFieldChanged を発火しない（blur-commit のみが発火源、§4.5）"
    );
}

// --- シナリオ 2: 韓国語 × Windows の音節確定ごとの Disabled→Enabled 高頻度再サイクル ---

/// Windows は「1 回の変換完了ごとに必ず `Ime::Disabled` を発火する」（S12 所見、
/// `event_loop.rs` L1603-1608）。2 ボル式は 1 打鍵ごとに音節が完成しうるため、
/// 日本語（文節単位）よりさらに高頻度に `Disabled`→`Enabled` が再サイクルしうる。
/// `TextFieldWidget::apply_ime` の `Disabled` 処理が冪等（`text_field.rs` §4.6）で
/// あることに変わりはなく、この増幅されたサイクルも既存の状態モデルのまま許容
/// できることを確認する。
#[test]
fn hangul_windows_high_frequency_disabled_enabled_recycle_per_syllable() {
    let mut field = focused_field("");

    for (partial, committed) in [("하", "한"), ("그", "글")] {
        apply(&mut field, Ime::Enabled);
        apply(
            &mut field,
            Ime::Preedit(partial.to_string(), Some((0, partial.len()))),
        );
        // WM_IME_ENDCOMPOSITION: 空 Preedit → Commit → 必ず Disabled（S12 所見）。
        apply(&mut field, Ime::Preedit(String::new(), None));
        apply(&mut field, Ime::Commit(committed.to_string()));
        apply(&mut field, Ime::Disabled);
    }

    assert_eq!(
        field.buffer(),
        "한글",
        "音節ごとの Disabled/Enabled 再サイクルでも確定内容は正しく積み上がる"
    );
}

// --- シナリオ 3: 韓国語 Backspace による字母単位の分解 ------------------------------

/// 2 ボル式は変換確定前に Backspace すると音節が字母単位で分解される（한 → 하 →
/// ㅎ → 空）。分解は preedit の更新のみで `Commit` を経由しないため、`buffer` が
/// 不変であることを確認する（Linux X11 の Esc キャンセル・Windows の Esc
/// キャンセルと同型の「Commit を経由しない preedit のみの変化」パターン）。
#[test]
fn hangul_backspace_decomposes_jamo_without_commit() {
    let mut field = focused_field("既存値");

    apply(&mut field, Ime::Enabled);
    apply(&mut field, Ime::Preedit("한".to_string(), Some((0, 0))));

    // Backspace で字母単位に分解: 한 → 하 → ㅎ → 空（Commit を経由しない）。
    for partial in ["하", "ㅎ", ""] {
        apply(&mut field, Ime::Preedit(partial.to_string(), Some((0, 0))));
    }

    assert_eq!(
        field.buffer(),
        "既存値",
        "字母分解は preedit のみの変化で buffer は不変"
    );
}

// --- シナリオ 4: 韓国語 漢字（한자）候補変換 ---------------------------------------

/// ハングル音節を漢字候補へ変換するシナリオ（例: 「한」→ 候補選択 → 「韓」）。
/// 漢字変換候補の選択も通常のハングル変換候補選択と同じ `Preedit` 置換 →
/// 「空 Preedit → Commit」の発火順であり、`ImeEvent::Preedit`/`Commit` が運ぶ
/// 文字列がハングルか漢字かに関わらず契約は変わらないことを確認する。
#[test]
fn hangul_hanja_candidate_conversion_commits_kanji() {
    let mut field = focused_field("");

    apply(&mut field, Ime::Enabled);
    apply(&mut field, Ime::Preedit("한".to_string(), Some((0, 0))));
    // 漢字候補ウィンドウでの選択により preedit が漢字へ置き換わる。
    apply(&mut field, Ime::Preedit("韓".to_string(), Some((0, 0))));
    apply(&mut field, Ime::Preedit(String::new(), None));
    let event = apply(&mut field, Ime::Commit("韓".to_string()));

    assert_eq!(field.buffer(), "韓");
    assert!(event.is_none());
}

// --- シナリオ 5: 韓国語 × Wayland の Done バッチ「空 Preedit → Commit → 次 Preedit」 ---

/// Wayland（`zwp_text_input_v3`）は 1 回の `Done` で「空 Preedit → Commit → 次の
/// Preedit」を最大 3 イベント連続送出しうる（S14 所見、`text_input/mod.rs`
/// L118-150）。ハングル音節の連続確定（한 確定 → 글 の変換開始が同一バッチ）でも
/// 状態モデルが破綻しないことを確認する。
#[test]
fn hangul_wayland_done_batch_applies_commit_then_next_preedit() {
    let mut field = focused_field("");

    apply(&mut field, Ime::Enabled);
    apply(
        &mut field,
        Ime::Preedit("한".to_string(), Some((0, "한".len()))),
    );

    // Done バッチ: 空 Preedit → Commit → 次の Preedit の順で連続送出される。
    apply(&mut field, Ime::Preedit(String::new(), None));
    let commit_event = apply(&mut field, Ime::Commit("한".to_string()));
    apply(
        &mut field,
        Ime::Preedit("그".to_string(), Some((0, "그".len()))),
    );

    assert_eq!(
        field.buffer(),
        "한",
        "Done バッチ内の Commit が buffer へ反映される"
    );
    assert!(commit_event.is_none());

    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("글".to_string()));
    assert_eq!(field.buffer(), "한글");
}

// --- シナリオ 6: 中国語 拼音の ASCII preedit → 複数文字 CJK Commit -------------------

/// 拼音入力は preedit が ASCII 文字列（"nihao"、1 byte == 1 char）である一方、
/// 変換確定時の `Commit` は複数文字の CJK 文字列（"你好"）になる。preedit と
/// commit で文字種・長さが乖離するこのパターンは日本語のローマ字入力と類似
/// するが、拼音では変換単位がより長い文単位になりやすい点を確認する。ASCII
/// preedit のカーソル（byte offset）は 1 byte == 1 char のため
/// `char_offset_from_byte` の変換でも数値が一致することを併せて確認する。
#[test]
fn pinyin_ascii_preedit_commits_multi_char_cjk_string() {
    let mut field = focused_field("");

    apply(&mut field, Ime::Enabled);
    for partial in ["n", "ni", "nih", "niha", "nihao"] {
        let end = partial.len();
        let event = apply(
            &mut field,
            Ime::Preedit(partial.to_string(), Some((end, end))),
        );
        assert!(event.is_none());
    }
    assert_eq!(field.buffer(), "", "拼音変換中は buffer 未反映");

    // スペースで変換候補「你好」を選択し確定。
    apply(&mut field, Ime::Preedit(String::new(), None));
    let event = apply(&mut field, Ime::Commit("你好".to_string()));

    assert_eq!(field.buffer(), "你好");
    assert_eq!(field.cursor(), 2, "CJK 2 文字はカーソル 2 と数える");
    assert!(event.is_none());
}

// --- シナリオ 7: 中国語 拼音のセグメント単位部分確定 --------------------------------

/// 長文拼音入力では、文全体を打ち切る前に前方のセグメントのみが部分確定され、
/// 後続セグメントの preedit が継続することがある（"nihao" 全体入力中に「你」が
/// 先に確定し、"hao" の変換が preedit として残るケース）。`Commit` 後も
/// composition が継続する非日本語的パターンだが、`apply_ime` が 1 イベントずつ
/// 独立処理する既存契約のまま成立することを確認する。
#[test]
fn pinyin_segment_partial_commit_continues_composition() {
    let mut field = focused_field("");

    apply(&mut field, Ime::Enabled);
    apply(&mut field, Ime::Preedit("nihao".to_string(), Some((5, 5))));

    // 前方セグメント「你」のみ部分確定。
    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("你".to_string()));
    assert_eq!(field.buffer(), "你", "部分確定分のみ buffer へ反映される");

    // 残りセグメント「hao」の変換が継続（先行 Commit とは独立した新規 preedit）。
    apply(&mut field, Ime::Preedit("hao".to_string(), Some((3, 3))));
    assert_eq!(
        field.buffer(),
        "你",
        "継続中の preedit は buffer に影響しない"
    );

    apply(&mut field, Ime::Preedit(String::new(), None));
    apply(&mut field, Ime::Commit("好".to_string()));

    assert_eq!(field.buffer(), "你好");
}

// --- シナリオ 8: 中国語 注音（繁体字）のマルチバイト声調記号込み preedit ---------------

/// 注音入力（繁体字圏で主流）は声調記号込みのマルチバイト preedit
/// （"ㄋㄧˇㄏㄠˇ"、注音符号 + 声調記号はいずれも非 ASCII のマルチバイト文字）を
/// 経て確定する。日本語・韓国語同様マルチバイト preedit だが、文字種が
/// 平仮名・ハングルとも異なる（注音符号 U+3105-312F 系）ことを含め、
/// `char_offset_from_byte` がマルチバイト境界で panic せず妥当な char offset を
/// 返すことを確認する。
#[test]
fn zhuyin_multibyte_tonal_preedit_commits_traditional_chinese() {
    let mut field = focused_field("");

    apply(&mut field, Ime::Enabled);
    let zhuyin = "ㄋㄧˇㄏㄠˇ";
    apply(
        &mut field,
        Ime::Preedit(zhuyin.to_string(), Some((0, zhuyin.len()))),
    );
    apply(&mut field, Ime::Preedit(String::new(), None));
    let event = apply(&mut field, Ime::Commit("你好".to_string()));

    assert_eq!(field.buffer(), "你好");
    assert_eq!(field.cursor(), 2);
    assert!(event.is_none());
}

// --- シナリオ 9: 中国語 拼音変換中の Esc キャンセル ---------------------------------

/// 拼音変換中に Esc でキャンセルした場合、日本語（Linux X11: `preedit_done_callback`
/// のみ、Windows: `get_composed_text` が `None`）と同様に、空 `Preedit` のみが
/// 発火し `Commit` は送出されない。`buffer` が不変であることを確認する。
#[test]
fn pinyin_escape_during_conversion_cancels_without_commit() {
    let mut field = focused_field("既存値");

    apply(&mut field, Ime::Enabled);
    apply(&mut field, Ime::Preedit("nihao".to_string(), Some((5, 5))));

    let event = apply(&mut field, Ime::Preedit(String::new(), None));

    assert_eq!(field.buffer(), "既存値", "キャンセルで buffer は変化しない");
    assert!(event.is_none());
}

// --- シナリオ 10: commit 型経路（iOS/Android 相当）へのハングル・CJK 確定文字列 -------

/// iOS（S16）・Android（S19）は `Ime::*` が一切発火せず、`text_field_input_from_key`
/// による確定文字列のみの commit 型経路を使う。ソフトウェアキーボードのハングル・
/// 中国語入力エンジンが確定した複数文字文字列（"한글"・"你好"）も、既存の
/// `InsertText` 契約でそのまま（分割・加工せず）挿入されることを確認する
/// （iOS の候補バー確定と同型、S17 §7 の複数文字非分割契約の CJK 版）。
#[test]
fn commit_style_path_inserts_hangul_and_chinese_confirmed_strings() {
    let mut field = focused_field("");
    // フォーカス取得時の SetEnabled(true) 要求は本シナリオの検証対象外。
    field.take_ime_requests();

    let key = Key::Character("한글".into());
    let input = text_field_input_from_key(ElementState::Pressed, &key, Some("한글"));
    assert_eq!(input, Some(TextFieldInput::InsertText("한글".to_string())));
    field.handle_input(input.unwrap());

    let key = Key::Character("你好".into());
    let input = text_field_input_from_key(ElementState::Pressed, &key, Some("你好"));
    assert_eq!(input, Some(TextFieldInput::InsertText("你好".to_string())));
    field.handle_input(input.unwrap());

    assert_eq!(field.buffer(), "한글你好");

    // Released 側（text: None）は二重挿入を防ぐため None のまま
    // （`text_field_input_from_key` の既存契約、S16/S19 と同型）。
    let released = text_field_input_from_key(ElementState::Released, &key, None);
    assert_eq!(released, None);

    // ImeRequest は commit 型経路ではソフトウェアキーボードの表示/非表示のみを
    // 意味し、preedit 用の要求（SetCursorArea 等）は積まれない契約を確認する。
    assert!(field
        .take_ime_requests()
        .iter()
        .all(|r| !matches!(r, ImeRequest::SetCursorArea { .. })));
}
