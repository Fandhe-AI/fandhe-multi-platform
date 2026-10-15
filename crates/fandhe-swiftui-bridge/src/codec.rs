//! 引数・戻り値の JSON シリアライズ・デシリアライズ（TASK-3.2-S3 / #60、REQ-3）。
//!
//! `docs/architecture/swiftui-bridge-architecture.md` §3.2（JSON ハイブリッド方式の採用）・
//! §3.1「codec モジュール」の実装。`unsafe` を一切含まない安全コードのみで完結する
//! （クレートルートの `#![deny(unsafe_code)]` に対し、本モジュールは `#[allow(unsafe_code)]`
//! を一切使わない）。
//!
//! # 呼び出し文脈
//!
//! S2（#59）の `ffi` モジュールが `extern "C"` エクスポート関数の内部で本モジュールの
//! [`encode_snapshot`] / [`decode_event`] を呼び出す想定（設計文書 3.3 の関数シグネチャ草案
//! `fandhe_render_snapshot` / `fandhe_dispatch_event` を参照）。本モジュール自体は
//! `fandhe-core` の `Screen<S>` / `Node<S>` / `Event` のみに依存し、FFI 境界（ポインタ・
//! 生存性検査）には関与しない。
//!
//! # スナップショット側（Rust → Swift）
//!
//! `fandhe_core::ui::Node<S>` は関数ポインタフィールド（`get` / `set` / `on_click` /
//! `expand`）を持ち `Serialize` を導出できないため、`fandhe-core` に依存しないプレーンな
//! データ型（[`SnapshotNode`] / [`RenderSnapshot`]）へ都度詰め替えてから JSON 化する
//! （設計文書 §3.3「`fandhe_render_snapshot`」参照）。この設計により `fandhe-core` に
//! serde 系の依存は一切追加されない。
//!
//! 走査は `Screen::iter()` と `Node::expand_children()` による再帰で行い、
//! `Screen::visit_expanded()`（フラットなコールバック列）ではなく、`List` の子要素境界を
//! JSON の入れ子構造として保持する（Swift 側が子要素を判別可能にするための精緻化。
//! S6（#63）の設計文書最終化にこの判断を反映するよう申し送り済み）。
//!
//! # イベント側（Swift → Rust）
//!
//! [`decode_event`] はペイロード長上限チェック → JSON パース（UTF-8 検証込み）→
//! 既知タグのみ受理、の順で入力を検証する（A03 インジェクション対策、security.md 準拠）。
//! 不正入力は panic ではなく [`CodecError`] を返す（coding-rust.md のエラーハンドリング方針）。

use fandhe_core::{Event, Node, Screen};
use serde::{Deserialize, Serialize};

/// スナップショット JSON のスキーマ世代番号。
///
/// S2（#59）の `fandhe_abi_version()` と併せて Rust ⇔ Swift のスキーマ世代不整合を
/// 検出可能にする布石（A08 整合性の不備対策、設計文書 §5）。
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Swift から受け取るイベント JSON ペイロードの長さ上限（バイト数）。
///
/// これを超えるペイロードは JSON パースを試みる前に [`CodecError::PayloadTooLarge`] として
/// 拒否する（A03 インジェクション対策・security.md 準拠。FFI 境界を信頼境界とみなす）。
pub const MAX_EVENT_PAYLOAD_LEN: usize = 64 * 1024;

/// 描画スナップショットの 1 ノードを表す、`fandhe-core` に依存しないプレーンな型。
///
/// `fandhe_core::Node<S>` を JSON 化可能な形へ詰め替えたもの（本モジュール冒頭のコメント
/// 参照）。フィールド構成は `Node<S>` の各バリアントに対応するが、関数ポインタは含まず、
/// `TextField` の `value` は `get(state)` を呼び出した時点の現在値である。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnapshotNode {
    /// 静的テキスト表示（`Node::Text` に対応）。
    Text {
        /// 表示するテキスト本体。
        text: String,
    },
    /// ラベル付きテキスト入力欄（`Node::TextField` に対応）。
    TextField {
        /// 入力欄に添えるラベル文字列。
        label: String,
        /// `get(state)` を呼び出した時点の現在値。
        value: String,
    },
    /// ボタン（`Node::Button` に対応）。
    Button {
        /// ボタンのラベル。
        label: String,
    },
    /// 動的な子要素一覧（`Node::List` に対応）。
    List {
        /// `expand(state)` を再帰的に詰め替えた子ノード一覧（ネスト構造を保持する）。
        children: Vec<SnapshotNode>,
    },
}

/// `Screen<S>` の走査結果全体を表すスナップショット。JSON 化して Swift 側へ渡す
/// トップレベルの型（設計文書 §3.3「`fandhe_render_snapshot`」戻り値の中身）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderSnapshot {
    /// スナップショット JSON のスキーマ世代番号（[`SNAPSHOT_SCHEMA_VERSION`] を参照）。
    pub schema_version: u32,
    /// 画面タイトル（`Screen::title()`）。
    pub title: String,
    /// スナップショット生成時点の `StateStore::version()`。Swift 側の再描画判定に使う。
    pub state_version: u64,
    /// 画面直下のノード一覧（挿入順、`List` は子要素を入れ子で保持する）。
    pub nodes: Vec<SnapshotNode>,
}

/// Swift から渡るイベント JSON の externally-tagged 表現。
///
/// `#[serde(deny_unknown_fields)]` により未知フィールドを含むペイロードを拒否し、
/// 未知の `type` タグは（`serde` の tagged enum 既定動作により）パース時点で
/// `serde_json::Error` となる（[`decode_event`] がこれを [`CodecError::Json`] へ変換する）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum EventPayload {
    /// `{"type":"button_clicked","label":...}` に対応する。
    ButtonClicked {
        /// 発火対象のボタンラベル。
        label: String,
    },
    /// `{"type":"text_field_changed","label":...,"value":...}` に対応する。
    TextFieldChanged {
        /// 発火対象のテキスト入力欄ラベル。
        label: String,
        /// 新しい入力値。
        value: String,
    },
}

impl From<EventPayload> for Event {
    fn from(payload: EventPayload) -> Self {
        match payload {
            EventPayload::ButtonClicked { label } => Event::ButtonClicked { label },
            EventPayload::TextFieldChanged { label, value } => {
                Event::TextFieldChanged { label, value }
            }
        }
    }
}

/// codec モジュールが返すエラー型（coding-rust.md 準拠、`unwrap()` / `expect()` を使わない）。
///
/// FFI エラーコード（`i32`）への変換は本モジュールの対象外（設計文書 §3.3
/// 「エラー伝播」参照）であり、本モジュールは `Result` を返すところまでを責務とする。
/// メッセージに内部ポインタ値・アドレス等の実装詳細は含めない（A05 対策）。
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// イベントペイロードが [`MAX_EVENT_PAYLOAD_LEN`] を超えている。
    #[error("イベントペイロードが上限を超えています（{len} バイト、上限 {max} バイト）")]
    PayloadTooLarge {
        /// 実際に渡されたペイロードのバイト数。
        len: usize,
        /// 許容する上限バイト数（[`MAX_EVENT_PAYLOAD_LEN`]）。
        max: usize,
    },
    /// JSON のシリアライズ・デシリアライズ（UTF-8 検証・未知タグ・未知フィールド・
    /// フィールド欠落を含む）に失敗した。
    #[error("JSON の処理に失敗しました: {0}")]
    Json(#[from] serde_json::Error),
}

/// `node` を（`state` を参照しつつ）[`SnapshotNode`] へ詰め替える。
///
/// `List` は [`fandhe_core::Node::expand_children`] で 1 段展開したうえで、各子ノードへ
/// 本関数を再帰適用する（ネストした `List` にも対応する、モジュール冒頭のコメント参照）。
fn to_snapshot_node<S>(node: &Node<S>, state: &S) -> SnapshotNode {
    match node {
        Node::Text(text) => SnapshotNode::Text { text: text.clone() },
        Node::DynamicText { get } => SnapshotNode::Text { text: get(state) },
        // role の伝搬（`SnapshotNode` への専用フィールド追加）は #813 / #814 の判断待ち
        // （未起票、計画メモ参照）。本 Issue（#812）時点では `DynamicText` と同じ
        // `SnapshotNode::Text` への変換に留める。
        Node::StyledText { get, .. } => SnapshotNode::Text { text: get(state) },
        Node::TextField { label, get, .. } => SnapshotNode::TextField {
            label: label.clone(),
            value: get(state),
        },
        Node::Button { label, .. } => SnapshotNode::Button {
            label: label.clone(),
        },
        Node::IndexedButton { label, .. } => {
            // 動的生成子内のボタン（Issue #491）。Swift 側へは既存 `SnapshotNode::Button`
            // と区別せずマッピングする（見た目・スナップショット構造は同一）。
            // イベントデコード側（`EventPayload` への `ButtonClickedAt` 追加）は
            // 本モジュールの対象外（out-of-scope-tracking.md 対象）。
            SnapshotNode::Button {
                label: label.clone(),
            }
        }
        Node::List { .. } => {
            let children = node
                .expand_children(state)
                .iter()
                .map(|child| to_snapshot_node(child, state))
                .collect();
            SnapshotNode::List { children }
        }
        Node::VirtualList { .. } => {
            // `Node::expand_children` は `VirtualList` を `0..len(state)` の全項目へ
            // 展開する（TASK-5.5-S2、fandhe-core::ui::node の設計判断）。Swift 側へは
            // 既存の `List` と区別せず `SnapshotNode::List` へ変換し、可視範囲限定の
            // 性能経路（`Screen::visit_windowed`）の実配線は別 PR に委ねる
            // （out-of-scope-tracking.md 対象。ScrollArea 実配線と同じ理由）。
            let children = node
                .expand_children(state)
                .iter()
                .map(|child| to_snapshot_node(child, state))
                .collect();
            SnapshotNode::List { children }
        }
        // 実画素の画像（Issue #5）を Swift へ渡すには `SnapshotNode` の拡張とスキーマ世代の
        // 更新が要るため対象外（別 Issue）。当面は代替テキストだけを `Text` として渡す。
        Node::RgbaImage { alt, .. } => SnapshotNode::Text { text: alt.clone() },
        Node::Widget(widget) => {
            // `Widget<S>` トレイトオブジェクト本体（`paint()` によるピクセル単位の
            // 描画コマンド）の Swift 向けスナップショット表現の精緻化（専用
            // `SnapshotNode` バリアント追加）はレイアウトプロトコル（S4、#99）・
            // 個別ウィジェット実装（S7 以降）確定後に行う。`widget.paint()` を
            // 呼んで内容を反映しない理由: この変換は `&Node<S>` の共有参照のみを
            // 持つ経路から呼ばれ、`Widget::sync_from_state`（`&mut self` 要求、
            // `fandhe_core` widget.rs 参照）を先に呼ぶ手段がない。この状態で
            // `paint()` を呼ぶとウィジェット構築時点の古いキャッシュ値を状態非依存に
            // Swift 側へ送ってしまう（TASK-8.3-S2 レビュー指摘、Issue #97）。
            //
            // 一方で `Widget::children`（Issue #739 で追加）は状態依存の子ノード列
            // （例: `DialogWidget` が開状態のときの確認文言・ボタン）を返す設計で、
            // `&self` のみで呼べる。これを呼ばずに空プレースホルダを返すと
            // `confirm_dialog_screen` のような「確認区画を Widget の子に持つ」画面で
            // 文言・ボタンがスナップショットから消えてしまう（codex-review 2 巡目
            // 指摘）。そのため `List` と同じ変換で子ノードを再帰的に出力する。
            // ウィジェット本体（`paint()` 配線）は未対応のまま。
            let children = widget
                .children(state)
                .iter()
                .map(|child| to_snapshot_node(child, state))
                .collect();
            SnapshotNode::List { children }
        }
    }
}

/// `screen` を `state` に対して走査し、JSON（UTF-8 バイト列）のスナップショットへ
/// シリアライズする（設計文書 §3.3「`fandhe_render_snapshot`」の実体）。
///
/// S2（#59）の `ffi` モジュールから呼ばれ、`extern "C"` 境界を越えて Swift 側へ返す
/// バッファの中身を構築する想定。`S` に対してジェネリックであり、単相化（特定の
/// アプリ状態型への固定）は S2 の ffi 層が行う。
pub fn encode_snapshot<S>(
    screen: &Screen<S>,
    state: &S,
    state_version: u64,
) -> Result<Vec<u8>, CodecError> {
    let nodes = screen
        .iter()
        .map(|node| to_snapshot_node(node, state))
        .collect();
    let snapshot = RenderSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        title: screen.title().to_string(),
        state_version,
        nodes,
    };
    let bytes = serde_json::to_vec(&snapshot)?;
    Ok(bytes)
}

/// Swift から渡されたイベント JSON バイト列を検証・デシリアライズし、
/// `fandhe_core::Event` へ変換する（設計文書 §3.3「`fandhe_dispatch_event`」の実体）。
///
/// 検証順序: (1) 長さ上限（[`MAX_EVENT_PAYLOAD_LEN`]）チェック、(2) JSON パース
/// （UTF-8 検証込み）、(3) 既知の `type` タグ・フィールドのみ受理（`deny_unknown_fields`）。
/// いずれかに失敗しても panic せず [`CodecError`] を返す（A03 インジェクション対策）。
pub fn decode_event(bytes: &[u8]) -> Result<Event, CodecError> {
    if bytes.len() > MAX_EVENT_PAYLOAD_LEN {
        return Err(CodecError::PayloadTooLarge {
            len: bytes.len(),
            max: MAX_EVENT_PAYLOAD_LEN,
        });
    }
    let payload: EventPayload = serde_json::from_slice(bytes)?;
    Ok(payload.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::{sample_screen, SampleState, StateStore};

    /// `sample_screen()` を走査したスナップショット JSON が、タイトル・ノード種別・順序・
    /// `TextField` の現在値・`List` の子要素（状態反映・ネスト）を正しく含むことを検証する。
    ///
    /// 件数・添字・文言を本テストへ固定するのではなく、`sample_screen()` の戻り値
    /// （`Screen<SampleState>`）自体から `title()` / `len()` / 各 `Node` の `get` を
    /// 都度導出して照合する。こうすることで `sample_screen()` にノードが増減しても
    /// 本テストの期待値更新漏れが起きない（構造変更時に追随すべき唯一の場所は
    /// `sample.rs` と `tests/sample_screen.rs` に一本化する、Issue #694）。
    #[test]
    fn encode_snapshot_reflects_title_nodes_and_current_state() {
        let screen = sample_screen();
        let state = SampleState {
            memo: "こんにちは".to_string(),
            items: vec!["item-1".to_string(), "item-2".to_string()],
            ..Default::default()
        };

        let bytes = encode_snapshot(&screen, &state, 7).expect("エンコードに成功するはず");
        let snapshot: RenderSnapshot =
            serde_json::from_slice(&bytes).expect("有効な JSON であるはず");

        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.title, screen.title());
        assert_eq!(snapshot.state_version, 7);
        assert_eq!(snapshot.nodes.len(), screen.len());

        let mut checked = 0;
        for (node, snap) in screen.iter().zip(&snapshot.nodes) {
            match (node, snap) {
                (Node::Text(text), SnapshotNode::Text { text: snap_text }) => {
                    assert_eq!(snap_text, text);
                }
                (Node::DynamicText { get }, SnapshotNode::Text { text: snap_text }) => {
                    assert_eq!(snap_text, &get(&state));
                }
                (
                    Node::TextField { label, get, .. },
                    SnapshotNode::TextField {
                        label: snap_label,
                        value: snap_value,
                    },
                ) => {
                    assert_eq!(snap_label, label);
                    assert_eq!(snap_value, &get(&state));
                }
                (Node::Button { label, .. }, SnapshotNode::Button { label: snap_label }) => {
                    assert_eq!(snap_label, label);
                }
                (Node::List { expand, .. }, SnapshotNode::List { children }) => {
                    let expected = expand(&state);
                    assert_eq!(children.len(), expected.len());
                    if let (Some(first_expected), Some(first_child)) =
                        (expected.first(), children.first())
                    {
                        match (first_expected, first_child) {
                            (Node::Text(text), SnapshotNode::Text { text: snap_text }) => {
                                assert_eq!(snap_text, text);
                            }
                            other => panic!("List 先頭子の種別不一致: {other:?}"),
                        }
                    }
                }
                other => panic!("種別不一致: {other:?}"),
            }
            checked += 1;
        }
        assert_eq!(checked, screen.len());
    }

    /// `Node::DynamicText` が `SnapshotNode::Text`（`List` ではなく）へ変換され、
    /// `get(state)` の現在値を反映することを検証する（Issue #581、単体レベルの回帰防止）。
    #[test]
    fn encode_snapshot_converts_dynamic_text_to_snapshot_text() {
        struct CountState {
            count: u32,
        }

        fn count_label(state: &CountState) -> String {
            format!("カウント: {}", state.count)
        }

        let screen: Screen<CountState> = Screen::new("画面").dynamic_text(count_label);
        let state = CountState { count: 7 };

        let bytes = encode_snapshot(&screen, &state, 0).expect("エンコードに成功するはず");
        let snapshot: RenderSnapshot = serde_json::from_slice(&bytes).unwrap();

        match &snapshot.nodes[0] {
            SnapshotNode::Text { text } => assert_eq!(text, "カウント: 7"),
            other => panic!("Text であるべき: {other:?}"),
        }
    }

    /// ネストした `List`（`List` の子として `List` を展開する）が再帰的に詰め替えられ、
    /// JSON 上も入れ子の `children` として表現されることを検証する。
    #[test]
    fn encode_snapshot_preserves_nested_list_structure() {
        struct NestedState;

        fn inner_expand(_state: &NestedState) -> Vec<Node<NestedState>> {
            vec![Node::Text("inner".to_string())]
        }
        fn outer_expand(_state: &NestedState) -> Vec<Node<NestedState>> {
            vec![Node::List {
                expand: inner_expand,
                axis: fandhe_core::Axis::default(),
            }]
        }

        let screen: Screen<NestedState> = Screen::new("ネスト画面").list(outer_expand);
        let state = NestedState;

        let bytes = encode_snapshot(&screen, &state, 0).expect("エンコードに成功するはず");
        let snapshot: RenderSnapshot = serde_json::from_slice(&bytes).unwrap();

        match &snapshot.nodes[0] {
            SnapshotNode::List { children } => match &children[0] {
                SnapshotNode::List { children } => match &children[0] {
                    SnapshotNode::Text { text } => assert_eq!(text, "inner"),
                    other => panic!("Text であるべき: {other:?}"),
                },
                other => panic!("List であるべき: {other:?}"),
            },
            other => panic!("List であるべき: {other:?}"),
        }
    }

    /// `decode_event` がボタン押下イベント JSON を `Event::ButtonClicked` へ変換し、
    /// `StateStore::dispatch` まで結合して `version()` が増加することを検証する。
    #[test]
    fn decode_event_button_clicked_dispatches_and_bumps_version() {
        let screen = sample_screen();
        let mut store = StateStore::new(SampleState::default());

        let json = r#"{"type":"button_clicked","label":"カウントを増やす"}"#;
        let event = decode_event(json.as_bytes()).expect("デコードに成功するはず");

        assert_eq!(
            event,
            Event::ButtonClicked {
                label: "カウントを増やす".to_string()
            }
        );

        let handled = store.dispatch(&screen, &event);
        assert!(handled);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// `decode_event` がテキスト入力変更イベント JSON を `Event::TextFieldChanged` へ
    /// 変換し、`dispatch` まで結合して状態・`version` が更新されることを検証する。
    #[test]
    fn decode_event_text_field_changed_dispatches_and_bumps_version() {
        let screen = sample_screen();
        let mut store = StateStore::new(SampleState::default());

        let json = r#"{"type":"text_field_changed","label":"メモ","value":"更新後"}"#;
        let event = decode_event(json.as_bytes()).expect("デコードに成功するはず");

        let handled = store.dispatch(&screen, &event);
        assert!(handled);
        assert_eq!(store.state().memo, "更新後");
        assert_eq!(store.version(), 1);
    }

    /// 長さ上限を超えるペイロードは JSON パースを試みず `PayloadTooLarge` を返す。
    #[test]
    fn decode_event_rejects_payload_exceeding_length_limit() {
        let oversized = vec![b' '; MAX_EVENT_PAYLOAD_LEN + 1];

        let err = decode_event(&oversized).expect_err("上限超過はエラーになるはず");
        match err {
            CodecError::PayloadTooLarge { len, max } => {
                assert_eq!(len, MAX_EVENT_PAYLOAD_LEN + 1);
                assert_eq!(max, MAX_EVENT_PAYLOAD_LEN);
            }
            other => panic!("PayloadTooLarge であるべき: {other:?}"),
        }
    }

    /// 不正な JSON（構文エラー）は panic せず `CodecError::Json` を返す。
    #[test]
    fn decode_event_rejects_malformed_json() {
        let err = decode_event(b"{not valid json").expect_err("パース失敗はエラーになるはず");
        assert!(matches!(err, CodecError::Json(_)));
    }

    /// 未知の `type` タグは既知のバリアントとして受理せずエラーを返す。
    #[test]
    fn decode_event_rejects_unknown_type_tag() {
        let json = br#"{"type":"unknown_event","label":"x"}"#;
        let err = decode_event(json).expect_err("未知タグはエラーになるはず");
        assert!(matches!(err, CodecError::Json(_)));
    }

    /// 未知フィールドを含むペイロードは `deny_unknown_fields` により拒否される。
    #[test]
    fn decode_event_rejects_unknown_fields() {
        let json = br#"{"type":"button_clicked","label":"x","extra":"y"}"#;
        let err = decode_event(json).expect_err("未知フィールドはエラーになるはず");
        assert!(matches!(err, CodecError::Json(_)));
    }

    /// 必須フィールドが欠落したペイロードはパースに失敗しエラーを返す。
    #[test]
    fn decode_event_rejects_missing_required_field() {
        let json = br#"{"type":"button_clicked"}"#;
        let err = decode_event(json).expect_err("フィールド欠落はエラーになるはず");
        assert!(matches!(err, CodecError::Json(_)));
    }

    /// `Node::Widget` は既定の `Widget::children`（空）を `List` と同じ変換で
    /// 詰め替えるため、子を持たないウィジェットは空の `SnapshotNode::List` へ
    /// 変換されることを検証する（`widget.paint()` を呼ばない、TASK-8.3-S2
    /// レビュー指摘・Issue #97。子ノード降下は Issue #739 で追加）。
    #[test]
    fn widget_node_converts_to_empty_list_when_no_children() {
        use fandhe_core::{
            AccessibilityRole, InteractionState, Measurement, Node, PaintCommand, Screen, Widget,
            WidgetId,
        };

        struct DummyState;

        #[derive(Debug, Clone)]
        struct StubWidget;

        impl Widget<DummyState> for StubWidget {
            fn id(&self) -> WidgetId {
                WidgetId::from_raw(0)
            }
            fn sync_from_state(&mut self, _state: &DummyState) {}
            fn measure(&self, _available: Measurement) -> Measurement {
                Measurement {
                    width: 0.0,
                    height: 0.0,
                }
            }
            fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
                vec![PaintCommand::Text("stub".to_string())]
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
            fn clone_box(&self) -> Box<dyn Widget<DummyState>> {
                Box::new(self.clone())
            }
        }

        let screen: Screen<DummyState> =
            Screen::new("画面").with_node(Node::Widget(Box::new(StubWidget)));
        let state = DummyState;

        let node = screen.iter().next().expect("Widget ノードが 1 件あるはず");
        let snapshot = to_snapshot_node(node, &state);

        assert_eq!(snapshot, SnapshotNode::List { children: vec![] });
    }

    /// `confirm_dialog_screen` の `Node::Widget(DialogWidget)` は開状態
    /// （`pending = Some(0)`）のとき `Widget::children` 経由で確認文言・
    /// 「キャンセル」「削除」ボタンをスナップショットへ出力することを検証する
    /// （PR #792 codex-review 2 巡目 P1 指摘の回帰防止）。
    #[test]
    fn encode_snapshot_includes_dialog_widget_children_when_confirming() {
        use fandhe_core::screens::confirm_dialog::{confirm_dialog_screen, ConfirmDialogState};

        let state = ConfirmDialogState {
            memos: vec!["メモ 1".to_string()],
            pending: Some(0),
        };
        let screen = confirm_dialog_screen(&state);

        let bytes = encode_snapshot(&screen, &state, 0).expect("エンコードに成功するはず");
        let json = String::from_utf8(bytes).expect("UTF-8 であるはず");

        assert!(json.contains("「メモ 1」を削除しますか？"));
        assert!(json.contains("キャンセル"));
        assert!(json.contains("削除"));
    }

    /// `pending = None`（確認パネル非表示）では `Widget::children` が空を返すため、
    /// 確認文言・ボタンがスナップショットに出ないことを検証する。
    #[test]
    fn encode_snapshot_omits_dialog_widget_children_when_not_confirming() {
        use fandhe_core::screens::confirm_dialog::{confirm_dialog_screen, ConfirmDialogState};

        let state = ConfirmDialogState {
            memos: vec!["メモ 1".to_string()],
            pending: None,
        };
        let screen = confirm_dialog_screen(&state);

        let bytes = encode_snapshot(&screen, &state, 0).expect("エンコードに成功するはず");
        let json = String::from_utf8(bytes).expect("UTF-8 であるはず");

        assert!(!json.contains("を削除しますか？"));
        assert!(!json.contains("キャンセル"));
    }
}
