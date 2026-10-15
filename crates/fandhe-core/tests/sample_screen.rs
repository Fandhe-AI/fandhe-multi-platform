//! `sample_screen`（TASK-1.1-S6）の結合テスト。
//!
//! `crates/fandhe-core/src/ui/sample.rs` が公開する代表 1 画面サンプルについて、
//! 「画面構成の網羅」「イベント発火による状態更新」「更新後の表示値への反映」
//! 「未知のイベントでの状態不変」を、クレート外部（`tests/`）から公開 API のみを
//! 用いて検証する。REQ-1 の受け入れ基準（宣言的 UI 記述・代表 1 画面）に対応する。

use fandhe_core::ui::{SampleState, get_memo};
use fandhe_core::{Node, Screen, click_button, render_headless, sample_screen};

/// 画面がテキスト・テキスト入力・ボタン・リストの各要素を期待どおりの件数で含み、
/// 挿入順が保たれていることを確認する。`src/ui/sample.rs` 側にあった同趣旨のユニット
/// テストと重複していたため、件数検証をこちら（結合テスト・公開 API のみ利用）へ
/// 統合した（Review 指摘対応）。
#[test]
fn sample_screen_includes_text_button_and_list() {
    let screen = sample_screen();
    assert_eq!(screen.len(), 6);

    let mut text_count = 0;
    let mut dynamic_text_count = 0;
    let mut text_field_count = 0;
    let mut button_count = 0;
    let mut list_count = 0;

    screen.visit(|node| match node {
        Node::Text(_) => text_count += 1,
        Node::DynamicText { .. } => dynamic_text_count += 1,
        // `sample_screen()` は `.error_text()` を使わない（Issue #812 のスコープ外）。
        // 到達しないはずの分岐だが、網羅性のために明示する。
        Node::StyledText { .. } => {}
        Node::TextField { .. } => text_field_count += 1,
        Node::Button { .. } => button_count += 1,
        // `sample_screen()` は `IndexedButton`（動的生成子の対話ノード）を使わない
        // （Issue #491 のスコープ）。到達しないはずの分岐だが、網羅性のために明示する。
        Node::IndexedButton { .. } => {}
        Node::List { .. } => list_count += 1,
        // `sample_screen()` は `VirtualList` を使わない（TASK-1.1-S6 由来の代表画面の
        // スコープ外）。到達しないはずの分岐として明示するが、`unreachable!()` には
        // せず件数を増やさないだけに留める（panic を API 契約にしない、
        // coding-rust.md のエラーハンドリング方針）。
        Node::VirtualList { .. } => {}
        // `sample_screen()` は `Widget` を使わない（個別ウィジェット実装は
        // Issue #97 のスコープ）。到達しないはずの分岐だが、網羅性のために明示する。
        Node::Widget(_) => {}
        Node::RgbaImage { .. } => {}
    });

    assert_eq!(text_count, 1, "テキスト要素は 1 件のはず");
    assert_eq!(
        dynamic_text_count, 1,
        "動的テキスト（カウント値表示）は 1 件のはず（Issue #581）"
    );
    assert_eq!(text_field_count, 1, "テキスト入力欄は 1 件のはず");
    assert_eq!(button_count, 2, "ボタンは 2 件のはず");
    assert_eq!(list_count, 1, "リストは 1 件のはず（項目一覧）");
}

/// `screen` が実際に組み立てた `Node::List` から `expand` 関数ポインタを取り出し、
/// `Node::Text` の並びを `String` の一覧へ変換する。`sample_screen()` が構築した画面
/// から直接取り出すことで、`.list()` の配線（`expand_items` 以外へ誤って差し替わって
/// いないか）ごと検証する（`text_field_get_set_round_trip_through_state` と同様の方針。
/// Review 指摘対応: 展開関数を直接呼ぶだけでは配線の検証にならないため）。
fn expand_to_texts(screen: &Screen<SampleState>, state: &SampleState) -> Vec<String> {
    let expand = screen
        .iter()
        .find_map(|node| match node {
            Node::List { expand, .. } => Some(*expand),
            _ => None,
        })
        .expect("画面はリストを含むべき");

    expand(state)
        .into_iter()
        .map(|node| match node {
            Node::Text(text) => text,
            other => panic!("Text バリアントであるべき: {other:?}"),
        })
        .collect()
}

/// ボタン押下でハンドラ経由の状態更新が起き、リスト展開関数の返す値が
/// 更新を反映することを確認する（イベント → 状態 → 表示値の一巡）。
#[test]
fn button_click_updates_state_and_list_reflects_it() {
    let screen = sample_screen();
    let mut state = SampleState::default();

    assert_eq!(expand_to_texts(&screen, &state), Vec::<String>::new());

    let handled = click_button(&screen, "項目を追加", &mut state);

    assert!(handled);
    assert_eq!(expand_to_texts(&screen, &state), vec!["item-1".to_string()]);
}

/// カウント増加ボタンを複数回押下し、状態が期待どおり累積することを確認する。
#[test]
fn increment_button_accumulates_count() {
    let screen = sample_screen();
    let mut state = SampleState::default();

    click_button(&screen, "カウントを増やす", &mut state);
    click_button(&screen, "カウントを増やす", &mut state);

    assert_eq!(state.count, 2);
}

/// テキスト入力欄の `get`/`set` が状態を経由して一巡することを確認する。
///
/// `sample_screen()` が実際に組み立てた `Node::TextField` から `get`/`set` を
/// 取り出して呼び出す（`state.memo` を直接書き換えない）。これにより、`set` の
/// 配線が `set_memo` 以外の関数へ誤って差し替わっていた場合や、`get`/`set` が
/// 別フィールドと取り違えられていた場合にテストが失敗するようにする
/// （Review 指摘対応: 修正前は `state.memo` を直接代入しており `set_memo` の
/// 配線を一度も経由していなかった）。
#[test]
fn text_field_get_set_round_trip_through_state() {
    let screen = sample_screen();
    let mut state = SampleState::default();

    let (get, set) = screen
        .iter()
        .find_map(|node| match node {
            Node::TextField { get, set, .. } => Some((*get, *set)),
            _ => None,
        })
        .expect("画面はテキスト入力欄を含むべき");

    assert_eq!(get(&state), "");

    set(&mut state, "更新後のメモ".to_string());

    assert_eq!(get(&state), "更新後のメモ");
    assert_eq!(get_memo(&state), "更新後のメモ");
}

/// `render_headless(sample_screen(), state)` の出力が、見出し・メモ欄・ボタン 2 種・
/// リスト展開結果を期待どおりの書式・順序で含むことを確認する（TASK-1.1-S2 / #18、
/// 描画マッピングと代表 1 画面サンプルの結合動作確認）。
#[test]
fn render_headless_reflects_sample_screen_state() {
    let screen: Screen<SampleState> = sample_screen();
    let mut state = SampleState::default();

    assert_eq!(
        render_headless(&screen, &state),
        vec![
            "# サンプル画面".to_string(),
            "代表 1 画面サンプル".to_string(),
            "[メモ: ]".to_string(),
            "(カウントを増やす)".to_string(),
            "(項目を追加)".to_string(),
            "カウント: 0".to_string(),
        ]
    );

    click_button(&screen, "カウントを増やす", &mut state);
    click_button(&screen, "項目を追加", &mut state);

    assert_eq!(
        render_headless(&screen, &state),
        vec![
            "# サンプル画面".to_string(),
            "代表 1 画面サンプル".to_string(),
            "[メモ: ]".to_string(),
            "(カウントを増やす)".to_string(),
            "(項目を追加)".to_string(),
            "item-1".to_string(),
            "カウント: 1".to_string(),
        ]
    );
}

/// カウント増加ボタンを複数回押下すると、ヘッドレス描画のカウント表示行が
/// 都度の押下に追随して更新されることを確認する（Issue #581: ボタン押下が画面へ
/// 反映されない不具合の回帰防止）。
#[test]
fn render_headless_reflects_count_after_multiple_increments() {
    let screen: Screen<SampleState> = sample_screen();
    let mut state = SampleState::default();

    click_button(&screen, "カウントを増やす", &mut state);
    click_button(&screen, "カウントを増やす", &mut state);
    click_button(&screen, "カウントを増やす", &mut state);

    let lines = render_headless(&screen, &state);
    assert!(
        lines.contains(&"カウント: 3".to_string()),
        "カウント表示行が更新後の値に追随していない: {lines:?}"
    );
}

/// 存在しないラベルのイベントでは状態が不変であり、panic しないことを確認する。
#[test]
fn unknown_button_label_leaves_state_unchanged_without_panicking() {
    let screen = sample_screen();
    let mut state = SampleState {
        count: 3,
        items: vec!["既存項目".to_string()],
        ..SampleState::default()
    };

    let handled = click_button(&screen, "存在しないボタン", &mut state);

    assert!(!handled);
    assert_eq!(state.count, 3);
    assert_eq!(state.items, vec!["既存項目".to_string()]);
}
