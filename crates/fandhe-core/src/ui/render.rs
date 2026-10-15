//! ヘッドレス描画マッピング（TASK-1.1-S2 / REQ-1 / #18）。
//!
//! [`super::screen::Screen::visit_expanded`]（S4 の走査基盤）の上に構築した、
//! 「ノード列 → テキスト表現」の変換契約を提供する。実描画バックエンド
//! （macOS / Web 等、TASK-1.2 以降のスコープ）を持たないため、テスト・example・
//! 将来のプラットフォームレイヤ描画走査（TASK-1.2 以降）が参照する最小限の
//! テキスト表現を返す。
//!
//! 書式は `docs/spec/03-poc/ai-optimization/dsl/src/lib.rs`（PoC-6）の
//! `render_headless` を踏襲する（本モジュールは `tests/poc6_alignment.rs` に
//! あったテストローカル実装を公開 API へ昇格したもの）:
//!
//! - 先頭行は `# {タイトル}`
//! - `Node::Text` はそのままの文字列
//! - `Node::DynamicText`（Issue #581）は `get(state)` の現在値をそのまま 1 行
//!   （`Node::Text` と同じ書式。状態非依存かどうかの違いのみ）
//! - `Node::StyledText`（Issue #812）も `get(state)` の現在値をそのまま 1 行
//!   （`role` はヘッドレス出力には含めない。色解決は #813 のプラットフォームレイヤの
//!   スコープであり、本関数はテキスト表現のみを返す契約のため）
//! - `Node::TextField` は `[{label}: {get(state)}]`
//! - `Node::Button` / `Node::IndexedButton`（Issue #491）は `({label})`
//! - `Node::List` 自身は行を出力しない。子ノードは `visit_expanded` の再帰展開により
//!   本関数へ後続して渡され、各バリアントに応じたアームで処理される
//!   （ネストした `List` も深さ優先で展開される）。
//!
//! 呼び出し元はテキストを画面へ表示するデータとしてのみ扱うこと。本関数は
//! 引数の `Screen` / 状態以外の外部入力（引数・環境変数・ネットワーク）を
//! 一切取らず、テキスト値をコマンド・クエリ・マークアップへ展開しない
//! （A03 インジェクション対策・security.md 準拠）。

use super::node::Node;
use super::screen::Screen;

/// `screen` を `state` の現在値でヘッドレス描画し、行単位のテキスト表現を返す。
///
/// `List` を含む画面でも、呼び出しのたびに `visit_expanded` が `expand(state)` を
/// 呼び直すため、状態更新後に再度呼び出せば `TextField` の値・`List` の展開結果が
/// 追随する（都度評価の設計、S4 の `visit_expanded` の契約を参照）。
///
/// # Examples
///
/// ```
/// use fandhe_core::{Screen, render_headless};
///
/// struct CounterState {
///     count: i32,
/// }
///
/// fn get_count(state: &CounterState) -> String {
///     state.count.to_string()
/// }
/// fn set_count(state: &mut CounterState, value: String) {
///     state.count = value.parse().unwrap_or(state.count);
/// }
///
/// let screen: Screen<CounterState> =
///     Screen::new("カウンタ画面").text_field("カウント", get_count, set_count);
/// let state = CounterState { count: 3 };
///
/// assert_eq!(
///     render_headless(&screen, &state),
///     vec!["# カウンタ画面".to_string(), "[カウント: 3]".to_string()]
/// );
/// ```
pub fn render_headless<S>(screen: &Screen<S>, state: &S) -> Vec<String> {
    let mut out = vec![format!("# {}", screen.title())];
    screen.visit_expanded(state, |node| match node {
        Node::Text(text) => out.push(text.clone()),
        Node::DynamicText { get } => out.push(get(state)),
        // role は出力しない（Issue #812。色解決は #813 のプラットフォームレイヤが担う）。
        Node::StyledText { get, .. } => out.push(get(state)),
        Node::TextField { label, get, .. } => out.push(format!("[{}: {}]", label, get(state))),
        Node::Button { label, .. } => out.push(format!("({label})")),
        Node::IndexedButton { label, .. } => out.push(format!("({label})")),
        Node::List { .. } => {
            // `List` 自身は行を出力しない。子ノードは `visit_expanded` の再帰展開に
            // より本クロージャへ後続して渡され、各アームで処理される。
        }
        Node::VirtualList { .. } => {
            // `List` と同様、自身は行を出力しない。`visit_expanded` は `VirtualList` も
            // `0..len(state)` の全項目へ展開するため（TASK-5.5-S2、node.rs 参照）、
            // 子ノードは本クロージャへ後続して渡され Text/TextField/Button の各アームで
            // 処理される（性能経路が必要な呼び出し元は `Screen::visit_windowed` を使う）。
        }
        Node::RgbaImage { pixels, alt } => out.push(format!(
            "[画像 {}×{}: {}]",
            pixels.width(),
            pixels.height(),
            alt
        )),
        Node::Widget(_widget) => {
            // 自身は行を出力しない（`List` / `VirtualList` と同様の no-op）。
            // 子ノード（`DialogWidget::content` 等、`Widget::children(state)` が返す
            // 状態依存の子ノード列）は `visit_expanded` が `Node::expand_children`
            // 経由でこのクロージャへ後続して渡すため、Text/Button 等の各アームで
            // 処理される（Issue #739）。
            //
            // `visit_expanded` は `&Node<S>` の共有参照のみを渡すため、
            // `Widget::sync_from_state`（`&mut self` を要求、widget.rs 参照）を
            // ここから呼ぶ経路は依然として存在しない。この状態で `widget.paint()` を
            // 呼んでしまうと、ウィジェット構築時点の古いキャッシュ値を
            // 状態非依存に描画してしまう（TASK-8.3-S2 レビューで指摘、Issue #97）。
            // reconcile/sync パス（設計文書 §4.4、S3・#98 のスコープ）が
            // `Node::Widget` へ `&mut` を届ける手段を確立するまで、`paint()` への
            // 委譲は行わない。S3 でこの no-op を置き換える際は widget.rs の
            // `sync_from_state` doc コメントの契約（sync 経路が先に必要）を守ること。
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::super::widget::WidgetId;
    use super::render_headless;
    use crate::ui::{Node, Screen};

    struct CounterState {
        memo: String,
        items: Vec<String>,
    }

    fn get_memo(state: &CounterState) -> String {
        state.memo.clone()
    }
    fn set_memo(state: &mut CounterState, value: String) {
        state.memo = value;
    }
    fn no_op(_state: &mut CounterState) {}
    fn expand_items(state: &CounterState) -> Vec<Node<CounterState>> {
        state.items.iter().cloned().map(Node::Text).collect()
    }

    /// 空画面（ノードなし）はタイトル行のみを返すことを検証する。
    #[test]
    fn empty_screen_renders_title_only() {
        let screen: Screen<CounterState> = Screen::new("空画面");
        let state = CounterState {
            memo: String::new(),
            items: Vec::new(),
        };

        assert_eq!(
            render_headless(&screen, &state),
            vec!["# 空画面".to_string()]
        );
    }

    /// `Text` / `TextField` / `Button` / `List` を混在させた画面で、各行の書式が
    /// 期待どおりであることを検証する。
    #[test]
    fn mixed_screen_renders_all_variants_with_expected_format() {
        let screen: Screen<CounterState> = Screen::new("混在画面")
            .text("見出し")
            .text_field("メモ", get_memo, set_memo)
            .button(WidgetId::from_raw(1), "押す", no_op)
            .list(expand_items);
        let state = CounterState {
            memo: "下書き".to_string(),
            items: vec!["a".to_string(), "b".to_string()],
        };

        assert_eq!(
            render_headless(&screen, &state),
            vec![
                "# 混在画面".to_string(),
                "見出し".to_string(),
                "[メモ: 下書き]".to_string(),
                "(押す)".to_string(),
                "a".to_string(),
                "b".to_string(),
            ]
        );
    }

    /// `Node::DynamicText` が `get(state)` の現在値をそのまま 1 行出力し、状態更新後の
    /// 再呼び出しに追随することを検証する（Issue #581）。
    #[test]
    fn dynamic_text_renders_current_state_value_and_tracks_updates() {
        fn get_count_label(state: &CounterState) -> String {
            format!("カウント: {}", state.items.len())
        }

        let screen: Screen<CounterState> = Screen::new("画面").dynamic_text(get_count_label);
        let mut state = CounterState {
            memo: String::new(),
            items: Vec::new(),
        };

        assert_eq!(
            render_headless(&screen, &state),
            vec!["# 画面".to_string(), "カウント: 0".to_string()]
        );

        state.items.push("a".to_string());

        assert_eq!(
            render_headless(&screen, &state),
            vec!["# 画面".to_string(), "カウント: 1".to_string()]
        );
    }

    /// `Node::StyledText`（`.error_text()` ビルダー由来）は `role` を出力に混ぜず、
    /// `get(state)` の現在値のみを 1 行出力する（Issue #812 の受け入れ条件）。
    #[test]
    fn error_text_renders_current_state_value_without_role() {
        fn get_error_label(state: &CounterState) -> String {
            format!("エラー: {} 件", state.items.len())
        }

        let screen: Screen<CounterState> = Screen::new("x").error_text(get_error_label);
        let state = CounterState {
            memo: String::new(),
            items: vec!["a".to_string()],
        };

        assert_eq!(
            render_headless(&screen, &state),
            vec!["# x".to_string(), get_error_label(&state)]
        );
    }

    /// 状態を更新して再度呼び出すと、`TextField` の値・`List` の展開結果が
    /// 追随することを検証する（都度 `expand(state)` / `get(state)` を呼ぶ設計）。
    #[test]
    fn re_rendering_after_state_update_reflects_new_values() {
        let screen: Screen<CounterState> = Screen::new("画面")
            .text_field("メモ", get_memo, set_memo)
            .list(expand_items);
        let mut state = CounterState {
            memo: "初期値".to_string(),
            items: vec!["a".to_string()],
        };

        assert_eq!(
            render_headless(&screen, &state),
            vec![
                "# 画面".to_string(),
                "[メモ: 初期値]".to_string(),
                "a".to_string(),
            ]
        );

        state.memo = "更新後".to_string();
        state.items.push("b".to_string());

        assert_eq!(
            render_headless(&screen, &state),
            vec![
                "# 画面".to_string(),
                "[メモ: 更新後]".to_string(),
                "a".to_string(),
                "b".to_string(),
            ]
        );
    }

    /// ネストした `List` が深さ優先で展開され、各行が挿入順に並ぶことを検証する。
    #[test]
    fn nested_list_expands_depth_first() {
        fn inner_expand(_state: &CounterState) -> Vec<Node<CounterState>> {
            vec![
                Node::Text("inner-a".to_string()),
                Node::Text("inner-b".to_string()),
            ]
        }
        fn outer_expand(_state: &CounterState) -> Vec<Node<CounterState>> {
            vec![
                Node::Text("outer-a".to_string()),
                Node::List {
                    expand: inner_expand,
                    axis: crate::ui::Axis::default(),
                },
            ]
        }

        let screen: Screen<CounterState> = Screen::new("ネスト画面").list(outer_expand);
        let state = CounterState {
            memo: String::new(),
            items: Vec::new(),
        };

        assert_eq!(
            render_headless(&screen, &state),
            vec![
                "# ネスト画面".to_string(),
                "outer-a".to_string(),
                "inner-a".to_string(),
                "inner-b".to_string(),
            ]
        );
    }

    /// `VirtualList` は行を出力せず、`visit_expanded` の全展開経路により子ノード
    /// （`0..len(state)` の全項目）が後続して描画されることを検証する
    /// （TASK-5.5-S2 / REQ-5、既存描画走査の意味論的後方互換）。
    #[test]
    fn virtual_list_renders_all_items_via_full_expansion() {
        struct VirtualListState {
            len: usize,
        }
        fn virtual_len(state: &VirtualListState) -> usize {
            state.len
        }
        fn virtual_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
            Node::Text(format!("item-{index}"))
        }

        let screen: Screen<VirtualListState> = Screen::new("仮想化リスト画面")
            .text("見出し")
            .virtual_list(virtual_len, virtual_item, 40.0);
        let state = VirtualListState { len: 3 };

        assert_eq!(
            render_headless(&screen, &state),
            vec![
                "# 仮想化リスト画面".to_string(),
                "見出し".to_string(),
                "item-0".to_string(),
                "item-1".to_string(),
                "item-2".to_string(),
            ]
        );
    }

    /// `Node::Widget` は行を出力しない（真の no-op）ことを検証する。
    ///
    /// `sync_from_state` を呼べる経路がないままキャッシュ値をそのまま描画すると
    /// 状態非依存の誤表示になる（TASK-8.3-S2 レビュー指摘、Issue #97）ため、
    /// `render_headless` は `Node::Widget` を安全側（無出力）に倒す。
    #[test]
    fn widget_node_renders_no_lines() {
        use crate::ui::widget::{
            AccessibilityRole, InteractionState, Measurement, PaintCommand, Widget, WidgetId,
        };

        #[derive(Debug, Clone)]
        struct StubWidget {
            cached_count: i32,
        }

        impl Widget<CounterState> for StubWidget {
            fn id(&self) -> WidgetId {
                WidgetId::from_raw(0)
            }
            fn sync_from_state(&mut self, _state: &CounterState) {
                self.cached_count += 1;
            }
            fn measure(&self, _available: Measurement) -> Measurement {
                Measurement {
                    width: 0.0,
                    height: 0.0,
                }
            }
            fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
                vec![PaintCommand::Text(format!("count: {}", self.cached_count))]
            }
            fn hit_test(
                &self,
                _point: (f32, f32),
                _origin: (f32, f32),
                _size: Measurement,
            ) -> bool {
                false
            }
            fn accessibility_role(&self) -> AccessibilityRole {
                AccessibilityRole::Button
            }
            fn interaction_state(&self) -> InteractionState {
                InteractionState::default()
            }
            fn clone_box(&self) -> Box<dyn Widget<CounterState>> {
                Box::new(self.clone())
            }
        }

        let screen: Screen<CounterState> = Screen::new("ウィジェット画面")
            .text("見出し")
            .with_node(Node::Widget(Box::new(StubWidget { cached_count: 5 })));
        let state = CounterState {
            memo: String::new(),
            items: Vec::new(),
        };

        assert_eq!(
            render_headless(&screen, &state),
            vec!["# ウィジェット画面".to_string(), "見出し".to_string()]
        );
    }
}
