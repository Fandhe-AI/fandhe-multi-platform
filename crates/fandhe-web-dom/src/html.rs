//! `DomNode` → HTML 文字列のシリアライズ（SEO / SSR 用途・native テスト用、TASK-4.4-S2 / REQ-4）。
//!
//! `mount` モジュール（wasm32 専用の実 DOM 構築）とは独立した経路であり、
//! native ターゲットでも走らせられる（結合テスト・SSR 用途を想定）。
//!
//! # セキュリティ（OWASP A03 インジェクション対策、最重要）
//!
//! `Node::Text` / `TextField::label` / `get(state)` の戻り値等、外部由来になり得る
//! 文字列がテキストコンテンツ・属性値としてそのまま HTML へ展開される経路のため、
//! [`escape_html`] を必ず経由する（テキスト・属性値のいずれもエスケープする）。
//! `set_text_content` / `set_attribute` のみを使う wasm32 側の [`super::mount`] とは異なり、
//! 本モジュールは文字列連結で HTML を組み立てるため、エスケープ漏れが直接 XSS に
//! つながる。エスケープ漏れをリグレッション検知するテストを本ファイル末尾に固定化する。

use super::dom_tree::DomNode;

/// `&` `<` `>` `"` `'` の 5 文字を HTML エンティティへ置換する。
///
/// テキストコンテンツ・属性値の双方に適用可能な最小のエスケープ関数（`&` を最初に
/// 置換することで二重エスケープを避ける、標準的な実装順序）。
fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `dom` を HTML 文字列へシリアライズする。テキスト・属性値はいずれも
/// [`escape_html`] を経由するため、`Node::Text` 等に由来する文字列が
/// HTML タグ・属性として解釈されることはない（A03 対策）。
pub fn to_html(dom: &DomNode) -> String {
    let mut out = String::new();
    write_node(dom, &mut out);
    out
}

/// HTML5 で終了タグを持たない void element の集合。
///
/// `write_node` はこれらのタグに対して終了タグ（`</tag>`）を出力しない
/// （HTML パーサは終了タグを無視するため実害はないが、SSR/SEO 用途を謳う
/// 本モジュールの目的上、非準拠なマークアップを避ける）。
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

fn is_void_element(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag)
}

fn write_node(node: &DomNode, out: &mut String) {
    out.push('<');
    out.push_str(&node.tag);
    for (name, value) in &node.attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        out.push_str(&escape_html(value));
        out.push('"');
    }
    out.push('>');

    if is_void_element(&node.tag) {
        // void element はテキスト・子要素・終了タグを持たない（HTML5 仕様）。
        // 本レンダラで void element にテキスト/子が渡ることは想定していないが、
        // 渡された場合も破棄して非準拠なマークアップを出力しない。
        return;
    }

    if let Some(text) = &node.text {
        out.push_str(&escape_html(text));
    }
    for child in &node.children {
        write_node(child, out);
    }

    out.push_str("</");
    out.push_str(&node.tag);
    out.push('>');
}

#[cfg(test)]
mod tests {
    use super::super::dom_tree::{build_dom_tree, EventHook};
    use super::*;
    use fandhe_core::{Node, Screen, WidgetId};

    struct CounterState {
        note: String,
    }

    fn get_note(state: &CounterState) -> String {
        state.note.clone()
    }
    fn set_note(state: &mut CounterState, value: String) {
        state.note = value;
    }
    fn on_click(_state: &mut CounterState) {}

    #[test]
    fn simple_leaf_serializes_to_expected_tag() {
        let node = DomNode {
            tag: "p".to_string(),
            attrs: Vec::new(),
            text: Some("本文".to_string()),
            children: Vec::new(),
            event_hook: None,
        };

        assert_eq!(to_html(&node), "<p>本文</p>");
    }

    #[test]
    fn attribute_value_is_escaped() {
        let node = DomNode {
            tag: "input".to_string(),
            attrs: vec![("value".to_string(), "\"onmouseover=alert(1)".to_string())],
            text: None,
            children: Vec::new(),
            event_hook: Some(EventHook::Input { handler_index: 0 }),
        };

        let html = to_html(&node);
        assert_eq!(html, "<input value=\"&quot;onmouseover=alert(1)\">");
        // 属性境界を抜け出す生の `"` が残っていないことを固定化する。
        assert!(!html.contains("value=\"\"onmouseover"));
    }

    #[test]
    fn void_element_omits_closing_tag_even_with_text_and_children() {
        // void element（`input` 等）は HTML5 上テキスト・子要素・終了タグを持たない。
        // 本レンダラで想定しない入力（テキスト/子付きの void element）が渡っても
        // 非準拠な `</input>` を出力しないことを固定化する。
        let node = DomNode {
            tag: "input".to_string(),
            attrs: Vec::new(),
            text: Some("無視されるはずのテキスト".to_string()),
            children: vec![DomNode {
                tag: "p".to_string(),
                attrs: Vec::new(),
                text: Some("無視されるはずの子要素".to_string()),
                children: Vec::new(),
                event_hook: None,
            }],
            event_hook: None,
        };

        let html = to_html(&node);
        assert_eq!(html, "<input>");
    }

    #[test]
    fn text_content_script_tag_is_escaped() {
        let node = DomNode {
            tag: "p".to_string(),
            attrs: Vec::new(),
            text: Some("<script>alert(1)</script>".to_string()),
            children: Vec::new(),
            event_hook: None,
        };

        let html = to_html(&node);
        assert_eq!(html, "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>");
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn ampersand_is_escaped_first_to_avoid_double_escaping() {
        let node = DomNode {
            tag: "p".to_string(),
            attrs: Vec::new(),
            text: Some("&lt;既にエスケープ済み&gt;".to_string()),
            children: Vec::new(),
            event_hook: None,
        };

        // 二重エスケープしない（`&` を最初にエンティティ化するのみ）ことを確認する。
        assert_eq!(to_html(&node), "<p>&amp;lt;既にエスケープ済み&amp;gt;</p>");
    }

    #[test]
    fn state_derived_malicious_label_is_escaped_end_to_end() {
        // Screen<S> → build_dom_tree → to_html の経路全体で、状態由来の悪意ある
        // 文字列（label・get(state) の戻り値）がエスケープされることを検証する。
        let screen: Screen<CounterState> = Screen::new("<img src=x onerror=alert(1)>").text_field(
            "<b>メモ</b>",
            get_note,
            set_note,
        );
        let state = CounterState {
            note: "\"><script>alert(2)</script>".to_string(),
        };

        let dom = build_dom_tree(&screen, &state, 0);
        let html = to_html(&dom);

        assert!(!html.contains("<img"));
        assert!(!html.contains("<b>メモ</b>"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;img"));
        assert!(html.contains("&quot;&gt;&lt;script&gt;"));
    }

    #[test]
    fn button_and_nested_list_render_expected_structure() {
        fn expand_items(state: &CounterState) -> Vec<Node<CounterState>> {
            vec![Node::Text(state.note.clone())]
        }

        let screen: Screen<CounterState> = Screen::new("画面")
            .button(WidgetId::from_raw(1), "押す", on_click)
            .list(expand_items);
        let state = CounterState {
            note: "項目".to_string(),
        };

        let dom = build_dom_tree(&screen, &state, 0);
        let html = to_html(&dom);

        assert!(html.starts_with(
            "<div><h1>画面</h1><button>押す</button><ul><li><p>項目</p></li></ul></div>"
        ));
    }
}
