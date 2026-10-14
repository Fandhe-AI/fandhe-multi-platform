//! PoC-6 DSL との整合確認・ユニットテスト（TASK-13.1-S4 / REQ-13）。
//!
//! `docs/spec/03-poc/ai-optimization/dsl/src/lib.rs`（PoC-6）が実証した
//! 「画面構築 → ヘッドレス走査 → ボタン押下 → 状態変化」という一連の挙動を、
//! `fandhe_core` の公開 API のみを用いて再現し、PoC-6 との API 面・
//! 挙動の整合を回帰検知可能にする。`crates/fandhe-core/tests/sample_screen.rs`
//! （代表 1 画面サンプルの結合テスト）とは独立に、PoC-6 の対応表（実装計画の
//! 「現状の API 対応表」）を項目単位で網羅する構成にしている。
//!
//! 対応状況（実装計画より抜粋）:
//! - `Node::List::items: fn(&S) -> Vec<String>`（PoC-6）は
//!   `expand: fn(&S) -> Vec<Node<S>>` に一般化されている（子に任意ノードを許容する
//!   意図的な設計拡張）。本テストではその差分を踏まえ、`Node::Text` へ変換する
//!   `expand` を用いて PoC-6 の `items` 相当の挙動を再現する。
//! - 書き込み側アクセサ（`fn(&mut S, T)` を置き換える型）は `state::WriteFn`
//!   （Issue #30、#307）へ統合されている。

use fandhe_core::state::ReadAccessor;
use fandhe_core::{Node, Screen, WidgetId, click_button, render_headless};

/// PoC-6 のフォーム画面（テキスト入力欄 1・ボタン 1）相当の状態。
#[derive(Default)]
struct FormState {
    memo: String,
}

fn get_memo(state: &FormState) -> String {
    state.memo.clone()
}

fn set_memo(state: &mut FormState, value: String) {
    state.memo = value;
}

fn clear_memo(state: &mut FormState) {
    state.memo.clear();
}

/// PoC-6 のリスト画面（動的リスト 1・ボタン 1）相当の状態。
#[derive(Default)]
struct ListState {
    items: Vec<String>,
}

fn add_item(state: &mut ListState) {
    let label = format!("item-{}", state.items.len() + 1);
    state.items.push(label);
}

/// PoC-6 の `Node::List::items: fn(&S) -> Vec<String>` に相当する展開関数。
/// `fandhe_core` の `expand: fn(&S) -> Vec<Node<S>>` は子に任意ノードを許容する一般化
/// であるため、ここでは各項目を `Node::Text` に変換して PoC-6 と同等の
/// 文字列一覧表現を得る（対応表の意図的な差分に対する再現）。
fn expand_items(state: &ListState) -> Vec<Node<ListState>> {
    state.items.iter().cloned().map(Node::Text).collect()
}

// PoC-6 の `render_headless` 相当のヘッドレス描画は、`fandhe_core` の公開 API
// `fandhe_core::render_headless`（Issue #18）が担う。PoC-6 の出力書式のうち `# タイトル` /
// テキストそのまま / `[ラベル: 値]` / `(ラベル)` は踏襲するが、`List` の項目書式
// （PoC-6: `- {item}`）は踏襲しない。`fandhe_core` の `expand` は `Vec<Node<S>>` を返す
// 一般化がされており、展開された子は `Node::Text` として `visit_expanded` により
// 後続訪問され、`Text` アームでプレフィックスなしのそのままの文字列として積まれる
// （`List` 自身は行を出力しない）。以下のテストは公開 API 切り替え後もアサーション
// を変更しておらず、挙動互換の回帰検証として機能する。

/// PoC-6 のフォーム画面 A（テキスト・テキスト入力欄・ボタン）を、`fandhe_core` の
/// `Screen::new` + `with_node` / `button` で構築できることを確認する。
#[test]
fn form_screen_builds_with_text_field_and_button() {
    let screen: Screen<FormState> = Screen::new("フォーム画面")
        .with_node(Node::Text("メモを入力してください".to_string()))
        .with_node(Node::TextField {
            label: "メモ".to_string(),
            get: get_memo,
            set: set_memo,
        })
        .button(WidgetId::from_raw(1), "クリア", clear_memo);

    assert_eq!(screen.title(), "フォーム画面");
    assert_eq!(screen.len(), 3);
}

/// PoC-6 のリスト画面 B（ボタン・動的リスト）を、`fandhe_core` の `Screen::new` +
/// `button` / `list` で構築できることを確認する。
#[test]
fn list_screen_builds_with_button_and_list() {
    let screen: Screen<ListState> = Screen::new("リスト画面")
        .button(WidgetId::from_raw(1), "項目を追加", add_item)
        .list(expand_items);

    assert_eq!(screen.title(), "リスト画面");
    assert_eq!(screen.len(), 2);
}

/// ヘッドレス描画整合: `render_headless` 相当のローカル関数が、PoC-6 と同等の
/// テキスト表現（見出し・テキスト・TextField 現在値・Button ラベル・List 展開結果）
/// を生成することを確認する。
#[test]
fn render_headless_matches_poc6_text_representation() {
    let screen: Screen<FormState> = Screen::new("フォーム画面")
        .with_node(Node::Text("メモを入力してください".to_string()))
        .with_node(Node::TextField {
            label: "メモ".to_string(),
            get: get_memo,
            set: set_memo,
        })
        .button(WidgetId::from_raw(1), "クリア", clear_memo);
    let state = FormState {
        memo: "下書き".to_string(),
    };

    let rendered = render_headless(&screen, &state);

    assert_eq!(
        rendered,
        vec![
            "# フォーム画面".to_string(),
            "メモを入力してください".to_string(),
            "[メモ: 下書き]".to_string(),
            "(クリア)".to_string(),
        ]
    );
}

/// ヘッドレス描画整合（リスト画面）: `List` の展開結果が挿入順で反映されることを
/// 確認する。PoC-6 の `- {item}` 書式は `Node::Text` の値そのままで代替する
/// （`fandhe_core` の `List` が任意ノードへ一般化されているための差分）。
#[test]
fn render_headless_reflects_expanded_list_items() {
    let screen: Screen<ListState> = Screen::new("リスト画面")
        .button(WidgetId::from_raw(1), "項目を追加", add_item)
        .list(expand_items);
    let state = ListState {
        items: vec!["item-1".to_string(), "item-2".to_string()],
    };

    let rendered = render_headless(&screen, &state);

    assert_eq!(
        rendered,
        vec![
            "# リスト画面".to_string(),
            "(項目を追加)".to_string(),
            "item-1".to_string(),
            "item-2".to_string(),
        ]
    );
}

/// イベント整合: `click_button` によるボタン押下でリスト画面の状態（項目一覧）が
/// 更新されることを確認する（PoC-6 の `click_button` 挙動の再現）。
#[test]
fn click_button_appends_item_to_list_state() {
    let screen: Screen<ListState> = Screen::new("リスト画面")
        .button(WidgetId::from_raw(1), "項目を追加", add_item)
        .list(expand_items);
    let mut state = ListState::default();

    let handled = click_button(&screen, "項目を追加", &mut state);

    assert!(handled);
    assert_eq!(state.items, vec!["item-1".to_string()]);
}

/// イベント整合: 存在しないラベルを指定すると `click_button` が `false` を返し、
/// 状態が変化しないことを確認する（PoC-6 の未検出時の挙動の再現）。
#[test]
fn click_button_returns_false_for_unknown_label_without_mutating_state() {
    let screen: Screen<ListState> = Screen::new("リスト画面")
        .button(WidgetId::from_raw(1), "項目を追加", add_item)
        .list(expand_items);
    let mut state = ListState {
        items: vec!["既存項目".to_string()],
    };

    let handled = click_button(&screen, "存在しないボタン", &mut state);

    assert!(!handled);
    assert_eq!(state.items, vec!["既存項目".to_string()]);
}

/// TextField 往復: `get` で現在値取得 → `set` で書き込み → `get` で反映確認する
/// （PoC-6 の `TextField` 往復挙動の再現。画面から実際に組み立てた `get`/`set` を
/// 取り出して検証し、配線の取り違えも検知できるようにする）。
#[test]
fn text_field_get_set_round_trip() {
    let screen: Screen<FormState> = Screen::new("フォーム画面").with_node(Node::TextField {
        label: "メモ".to_string(),
        get: get_memo,
        set: set_memo,
    });
    let mut state = FormState::default();

    let (get, set) = screen
        .iter()
        .find_map(|node| match node {
            Node::TextField { get, set, .. } => Some((*get, *set)),
            _ => None,
        })
        .expect("画面はテキスト入力欄を含むべき");

    assert_eq!(get(&state), "");

    set(&mut state, "更新後".to_string());

    assert_eq!(get(&state), "更新後");
}

/// アクセサ橋渡し: `ReadAccessor::as_fn()` の戻り値を `Node::TextField::get` /
/// `Screen::list` へそのまま渡せることを型レベルで確認する（`state` 層と `ui` 層の
/// 統合、TASK-13.1-S4 の中心的な検証項目）。
#[test]
fn read_accessor_bridges_into_text_field_and_list_nodes() {
    let get_accessor: ReadAccessor<FormState, String> = ReadAccessor::new(get_memo);
    let node: Node<FormState> = Node::TextField {
        label: "メモ".to_string(),
        get: get_accessor.as_fn(),
        set: set_memo,
    };
    let state = FormState {
        memo: "橋渡し確認".to_string(),
    };

    match node {
        Node::TextField { get, .. } => assert_eq!(get(&state), "橋渡し確認"),
        _ => panic!("TextField バリアントであるべき"),
    }

    let expand_accessor: ReadAccessor<ListState, Vec<Node<ListState>>> =
        ReadAccessor::new(expand_items);
    let screen: Screen<ListState> = Screen::new("リスト画面").list(expand_accessor.as_fn());
    let list_state = ListState {
        items: vec!["item-1".to_string()],
    };

    let expand = screen
        .iter()
        .find_map(|node| match node {
            Node::List { expand, .. } => Some(*expand),
            _ => None,
        })
        .expect("画面はリストを含むべき");
    let expanded = expand(&list_state);
    assert_eq!(expanded.len(), 1);
}

/// 型エイリアス同一性: `state::ReadFn<S, String>` と `Node::TextField::get` の型が
/// 代入互換であることを確認する（S1 のテスト意図の拡張。TASK-13.1-S4 で `ui` 側の
/// フィールド型を `ReadFn` へ統合したことの検証）。
#[test]
fn read_fn_type_alias_is_assignment_compatible_with_text_field_get() {
    use fandhe_core::state::ReadFn;

    let f: ReadFn<FormState, String> = get_memo;
    let node: Node<FormState> = Node::TextField {
        label: "メモ".to_string(),
        get: f,
        set: set_memo,
    };
    let state = FormState {
        memo: "型整合確認".to_string(),
    };

    match node {
        Node::TextField { get, .. } => assert_eq!(get(&state), "型整合確認"),
        _ => panic!("TextField バリアントであるべき"),
    }
}

/// 非キャプチャクロージャの coercion 受理: PoC-6 と同じ受理側境界として、
/// 環境をキャプチャしないクロージャは `fn` ポインタへ coerce 可能であり、
/// `Node::TextField::get` / `Screen::list` の引数として受理されることを確認する。
#[test]
fn non_capturing_closures_coerce_into_fn_pointer_fields() {
    let node: Node<FormState> = Node::TextField {
        label: "メモ".to_string(),
        get: |state| state.memo.clone(),
        set: |state, value| state.memo = value,
    };
    let mut state = FormState::default();

    match node {
        Node::TextField { get, set, .. } => {
            assert_eq!(get(&state), "");
            set(&mut state, "クロージャ経由".to_string());
            assert_eq!(get(&state), "クロージャ経由");
        }
        _ => panic!("TextField バリアントであるべき"),
    }

    let screen: Screen<ListState> = Screen::new("リスト画面")
        .list(|state: &ListState| state.items.iter().cloned().map(Node::Text).collect());
    let list_state = ListState {
        items: vec!["a".to_string()],
    };
    let expand = screen
        .iter()
        .find_map(|node| match node {
            Node::List { expand, .. } => Some(*expand),
            _ => None,
        })
        .expect("画面はリストを含むべき");
    assert_eq!(expand(&list_state).len(), 1);
}
