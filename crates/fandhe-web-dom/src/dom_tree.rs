//! `Screen<S>` / `Node<S>` からターゲット非依存の中間表現 `DomNode` への変換
//! （TASK-4.4-S2 / REQ-4）。
//!
//! `DomNode` は wasm32 の実 DOM（[`super::mount`]）・native の HTML シリアライズ
//! （[`super::html`]）の双方から共通に参照される中間表現であり、`web-sys` 等の
//! ターゲット固有型に依存しない。`fandhe-core` の `Node::expand_children` と同じ
//! 「`List` は都度 `expand(state)` を評価し再帰展開する」契約を踏襲する
//! （`render_headless` の都度評価契約と整合、TASK-1.1-S2/S4）。
//!
//! # マッピング方針
//!
//! | `Node<S>` バリアント | HTML 要素 |
//! |---|---|
//! | タイトル（`Screen::title()`） | `h1` |
//! | `Node::Text` | `p` |
//! | `Node::DynamicText`（Issue #581） | `p`（`Node::Text` と同じ。テキストは `get(state)` の都度評価） |
//! | `Node::StyledText`（Issue #812） | `Error` → `p role="alert" class="fandhe-text-error"`、`Body` → `p`（#814） |
//! | `Node::TextField` | `label` + `input`（`value` 属性は `get(state)`） |
//! | `Node::Button` | `button` |
//! | `Node::List` | `ul`（子要素は `li` でラップし、ネストした `List` も再帰的に `ul` へ） |
//! | `Node::Widget(Checkbox)` | `label` + `input type="checkbox"`（Issue #993） |
//! | `Node::Widget(ToggleSwitch)` | `label` + `input type="checkbox" role="switch"`（Issue #993） |
//! | `Node::Widget(Slider)` | `label` + `input type="range"`（Issue #993） |
//! | `Node::Widget(RadioButton)` | `label` + `input type="radio"`（Issue #993） |
//! | `Node::RgbaImage` | `img`（`src` は無圧縮 PNG の data URL、Issue #5） |
//! | `Node::Widget(ImageWidget)`（ソースが `blob:` で始まるとき） | `img`（Issue #1103）。それ以外は空プレースホルダ |

use fandhe_core::{
    CanvasWidget, Checkbox, ImageWidget, Measurement, Node, PaintCommand, RadioButton, Screen,
    Slider, TextRole, ToggleSwitch, Widget,
};

/// `Screen<S>` / `Node<S>` から変換した、ターゲット非依存の DOM 中間表現。
///
/// タグ名・属性・テキスト・子ノード・イベントフック識別子（`event_hook`）を保持する。
/// `event_hook` は [`super::mount`] が `Button::on_click` / `TextField::set` の
/// どちらへ配線すべきかを判別するために使う（HTML シリアライズ側では無視する）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomNode {
    /// HTML タグ名（例: `"h1"` / `"p"` / `"button"`）。
    pub tag: String,
    /// タグの属性一覧（`(名前, 値)` の挿入順リスト）。
    pub attrs: Vec<(String, String)>,
    /// タグ直下のテキストコンテンツ（子要素と排他ではないが、本レンダラでは
    /// テキストを持つノードは子要素を持たない最小構成とする）。
    pub text: Option<String>,
    /// 子ノード一覧（挿入順）。
    pub children: Vec<DomNode>,
    /// このノードに配線すべきイベントの種別。`None` は無配線（静的ノード）を表す。
    pub event_hook: Option<EventHook>,
}

/// [`DomNode`] に配線するイベントの種別（[`super::mount`] 専用のフック識別子）。
///
/// バリアントに紐づくインデックスは、元の `Screen<S>` を走査した際の
/// 「ボタン押下ハンドラ / テキスト入力の書き込みハンドラの通し番号」であり、
/// `mount` 側で状態 `S` に対する実際の関数ポインタを引き直すために使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventHook {
    /// `Node::Button::on_click` への配線要求（`click` イベント）。
    Click { handler_index: usize },
    /// `Node::TextField::set` への配線要求（`input` イベント）。
    Input { handler_index: usize },
    /// `Node::Widget(Checkbox|ToggleSwitch|Slider|RadioButton)` への配線要求
    /// （`change` イベント、Issue #993）。`handler_index` は
    /// [`Rendered::widget_handlers`] の添字、`kind` は組み立てるべき
    /// [`fandhe_core::WidgetValue`] の種別（[`super::mount`] が `HtmlInputElement` の
    /// 値をこの種別へ変換する）。
    Change {
        handler_index: usize,
        kind: WidgetValueKind,
    },
}

/// [`EventHook::Change`] が運ぶ、組み立てるべき [`fandhe_core::WidgetValue`] の種別。
///
/// `WidgetValue` 自体は値を保持する enum のため `EventHook`（`Copy` 前提、`mount.rs` が
/// `DomNode` を都度クローンせず参照するために `Copy` を要求する）へそのまま埋め込めない。
/// `RadioButton` の `Index` は自身の `value()`（`usize`）を運ぶ（[`super::mount`] が
/// `input.value()` を読まずこの値をそのまま使う。ラジオの `value` 属性は表示・native
/// シリアライズ用の付随情報であり、`change` 時の書き込み値は常にこの `kind` が運ぶ
/// 自分の `value()` で確定する設計、Issue #993）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetValueKind {
    /// `Checkbox` / `ToggleSwitch`（`input.checked()` を読む）。
    Bool,
    /// `Slider`（`input.value()` を `f32` へ解析する）。
    Number,
    /// `RadioButton`（自身の選択肢インデックスをそのまま運ぶ）。
    Index(usize),
}

impl DomNode {
    fn leaf(tag: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            text: Some(text.into()),
            children: Vec::new(),
            event_hook: None,
        }
    }

    fn container(tag: impl Into<String>, children: Vec<DomNode>) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            text: None,
            children,
            event_hook: None,
        }
    }
}

/// [`build_dom_tree`] / [`render`] の変換結果一式。`dom` は [`super::html::to_html`]
/// が消費するターゲット非依存の中間表現、`click_handlers` / `input_handlers` は
/// wasm32 専用の [`super::mount`] が [`EventHook::handler_index`] を引くための
/// 実体（`Button::on_click` / `TextField::set` の関数ポインタ）を、DOM ツリーの
/// 変換と同一走査順で保持する。
pub struct Rendered<S> {
    /// `Screen<S>` を変換した DOM 中間表現（ルートは `div`）。
    pub dom: DomNode,
    /// `handler_index` 順に並んだ `Node::Button::on_click`。
    pub click_handlers: Vec<fn(&mut S)>,
    /// `handler_index` 順に並んだ `Node::TextField::set`。
    pub input_handlers: Vec<fn(&mut S, String)>,
    /// `handler_index` 順に並んだ、`sync_from_state(state)` 済みの複製
    /// （`Checkbox` / `ToggleSwitch` / `Slider` / `RadioButton`、Issue #993）。
    /// [`super::mount`] は `change` イベントでこの複製の
    /// [`fandhe_core::Widget::apply_value`] を直接呼ぶ（複製は描画に使ったのと同じ id・
    /// 現在値を持つため、`id` の再照合は不要。`fandhe_core::ui::set_widget_value_at` の
    /// disabled ガードは複製の `interaction_state()` から同一に再現する、
    /// `mount.rs` の `attach_change` 参照）。
    pub widget_handlers: Vec<Box<dyn Widget<S>>>,
}

/// 変換中に採番するハンドラの通し番号と、対応する関数ポインタを蓄積する作業領域。
///
/// [`super::mount`] は `EventHook::handler_index` を使って「元の `Screen<S>` の
/// 何番目のボタン / テキスト入力に対応するイベントか」を引き直すため、採番順は
/// [`build_dom_tree`] / [`render`] の走査順（挿入順・深さ優先）と一致させる必要がある。
/// `DomNode` への変換と関数ポインタの採番を単一の走査（[`convert_node`]）で行うことで、
/// 2 つの走査ロジックを別々に持つことによる採番順のズレ（保守漏れ）を構造的に防ぐ。
struct HandlerCollector<S> {
    click_handlers: Vec<fn(&mut S)>,
    input_handlers: Vec<fn(&mut S, String)>,
    widget_handlers: Vec<Box<dyn Widget<S>>>,
    /// このツリーを変換しているマウントの識別子（[`super::mount::Applier::mount_id`]
    /// と同じ役割）。`RadioButton` の `name` 属性へ埋め込み、同一画面を同一ページの
    /// 複数 root へマウントしたときにラジオグループが衝突しないようにする
    /// （codex レビュー指摘 P1、PR #999）。native シリアライズ（[`super::html`]）は
    /// 単一マウント概念を持たないため `0` 固定でよい。
    mount_id: u64,
}

/// `screen` を `state` の現在値で `DomNode` ツリー（ルートは `div`）へ変換する。
///
/// ハンドラ関数ポインタの採番結果は破棄する薄いラッパー。HTML シリアライズ
/// （[`super::html`]）専用に、DOM 構造のみを要する呼び出し元向けの糖衣構文。
/// 実 DOM へのイベント配線が必要な呼び出し元（[`super::mount`]）は [`render`] を使う。
/// `mount_id` はマウントごとに一意な識別子（[`super::mount::Applier::mount_id`]
/// 参照）。native シリアライズ・テスト等、単一マウント概念を持たない呼び出し元は
/// `0` 固定でよい。
pub fn build_dom_tree<S: 'static>(screen: &Screen<S>, state: &S, mount_id: u64) -> DomNode {
    render(screen, state, mount_id).dom
}

/// `screen` を `state` の現在値で変換し、DOM 中間表現とハンドラ関数ポインタ一式
/// （[`Rendered`]）を返す。
///
/// `List` の子要素は変換のたびに `expand(state)` が再評価される（都度評価の契約、
/// `render_headless` と同じ）。`Button::on_click` / `TextField::set` は呼び出さず、
/// [`Rendered::click_handlers`] / [`Rendered::input_handlers`] へ蓄積するのみに留める
/// （実際の呼び出しは wasm32 専用の [`super::mount`] がイベント発火時に行う）。
///
/// `mount_id` はマウントごとに一意な識別子（[`super::mount::Applier::mount_id`]
/// 参照）。`RadioButton` の `name` 属性へ埋め込み、同一画面を同一ページの複数 root
/// へマウントしたときのラジオグループ衝突を防ぐ。単一マウント概念を持たない
/// 呼び出し元（native シリアライズ・テスト）は `0` 固定でよい。
pub fn render<S: 'static>(screen: &Screen<S>, state: &S, mount_id: u64) -> Rendered<S> {
    let mut collector = HandlerCollector {
        click_handlers: Vec::new(),
        input_handlers: Vec::new(),
        widget_handlers: Vec::new(),
        mount_id,
    };
    let mut children = vec![DomNode::leaf("h1", screen.title())];
    children.extend(
        screen
            .iter()
            .map(|node| convert_node(node, state, &mut collector)),
    );
    Rendered {
        dom: DomNode::container("div", children),
        click_handlers: collector.click_handlers,
        input_handlers: collector.input_handlers,
        widget_handlers: collector.widget_handlers,
    }
}

/// 1 つの `Node<S>` を `DomNode` へ変換する（深さ優先の再帰本体）。
///
/// `List` は [`fandhe_core::Node::expand_children`] で 1 段展開し、各子ノードを
/// `li` でラップしたうえで自身を再帰的に呼び出す（ネストした `List` も
/// `ul` の入れ子として表現される）。
fn convert_node<S: 'static>(
    node: &Node<S>,
    state: &S,
    collector: &mut HandlerCollector<S>,
) -> DomNode {
    match node {
        Node::Text(text) => DomNode::leaf("p", text.clone()),
        Node::DynamicText { get } => DomNode::leaf("p", get(state)),
        Node::StyledText { role, get } => {
            let mut node = DomNode::leaf("p", get(state));
            match role {
                // 通常本文は `DynamicText` と同じ無属性の `p` のまま。
                TextRole::Body => {}
                // `role="alert"` は暗黙に `aria-live="assertive"` を持ち、支援技術へ
                // 即時通知される（検証エラー文の想定用途、#811/#814）。色はここでは
                // 付けず、`class` をセレクタの受け皿としてホスト側 CSS へ委ねる
                // （DSL に色を書かせない方針、#811）。属性値は定数のみで、状態由来の
                // 文字列は `text`（`html.rs::escape_html` / `mount.rs::set_text_content`
                // 経由）にしか流れない。
                //
                // `mount.rs` は状態更新のたびに DOM を全再構築するため、この要素も
                // 毎回作り直される。結果として入力のたびに `role="alert"` が
                // 再読み上げされ得るが、差分更新の導入は `mount.rs` 側の既存
                // out-of-scope 事項として残す。
                TextRole::Error => {
                    node.attrs.push(("role".to_string(), "alert".to_string()));
                    node.attrs
                        .push(("class".to_string(), "fandhe-text-error".to_string()));
                }
            }
            node
        }
        Node::TextField { label, get, set } => {
            let handler_index = collector.input_handlers.len();
            collector.input_handlers.push(*set);
            let value = get(state);
            DomNode {
                tag: "label".to_string(),
                attrs: Vec::new(),
                text: Some(label.clone()),
                children: vec![DomNode {
                    tag: "input".to_string(),
                    attrs: vec![("value".to_string(), value)],
                    text: None,
                    children: Vec::new(),
                    event_hook: Some(EventHook::Input { handler_index }),
                }],
                event_hook: None,
            }
        }
        Node::Button {
            label, on_click, ..
        } => {
            let handler_index = collector.click_handlers.len();
            collector.click_handlers.push(*on_click);
            DomNode {
                tag: "button".to_string(),
                attrs: Vec::new(),
                text: Some(label.clone()),
                children: Vec::new(),
                event_hook: Some(EventHook::Click { handler_index }),
            }
        }
        Node::IndexedButton { label, .. } => {
            // 動的生成子内のボタン（Issue #491）。
            // `HandlerCollector::click_handlers` は `fn(&mut S)`（引数なし）固定のため、
            // `fn(&mut S, usize)` の `on_click` をそのまま登録できない。
            // `(handler, index)` を持つ新 binding 種別の設計は本モジュールの対象外とし、
            // DOM 構造（`button` 要素）のみを表現する no-op（`event_hook: None`）に
            // 留める（out-of-scope-tracking.md 対象）。
            DomNode {
                tag: "button".to_string(),
                attrs: Vec::new(),
                text: Some(label.clone()),
                children: Vec::new(),
                event_hook: None,
            }
        }
        Node::List { .. } => {
            let items = node
                .expand_children(state)
                .iter()
                .map(|child| DomNode::container("li", vec![convert_node(child, state, collector)]))
                .collect();
            DomNode::container("ul", items)
        }
        Node::VirtualList { .. } => {
            // `Node::expand_children` は `VirtualList` を `0..len(state)` の全項目へ
            // 展開する（TASK-5.5-S2、fandhe-core::ui::node の設計判断）。DOM 側でも
            // 既存の `List` と同じ `ul`/`li` マッピングを用い、可視範囲限定の性能経路
            // （`Screen::visit_windowed`）を用いた仮想スクロール実配線は別 PR に委ねる
            // （out-of-scope-tracking.md 対象）。
            let items = node
                .expand_children(state)
                .iter()
                .map(|child| DomNode::container("li", vec![convert_node(child, state, collector)]))
                .collect();
            DomNode::container("ul", items)
        }
        Node::RgbaImage { pixels, alt } => {
            // `src` は自前で作る `data:image/png;base64,` のみ（外部由来の文字列を
            // 含まない）。`ImageWidget` が外部ソースの `data:` を拒否する方針とは別経路。
            DomNode {
                tag: "img".to_string(),
                attrs: vec![
                    ("src".to_string(), crate::png_data_url::png_data_url(pixels)),
                    ("alt".to_string(), alt.clone()),
                    ("class".to_string(), "fandhe-image".to_string()),
                    (
                        "style".to_string(),
                        "max-width:100%;height:auto".to_string(),
                    ),
                ],
                text: None,
                children: Vec::new(),
                event_hook: None,
            }
        }
        Node::Widget(widget) => {
            // `Checkbox`/`ToggleSwitch`/`Slider`/`RadioButton` はネイティブ `<input>`
            // として実 DOM マッピングする（Issue #993）。それ以外のウィジェットは
            // 下記の従来ロジック（`CanvasWidget` の盤面 or 空プレースホルダ）へ委ねる。
            if let Some(dom) = convert_input_widget(widget.as_ref(), state, collector) {
                return dom;
            }
            if let Some(dom) = convert_image_widget(widget.as_ref(), state) {
                return dom;
            }
            // `Widget<S>` トレイトオブジェクトの実 DOM マッピング（属性・イベント
            // フック含む）はレイアウトプロトコル（S4、#99）・個別ウィジェット実装
            // （S7 以降）確定後に行う。既定はプレースホルダ（空の `div`）のまま
            // （下記の盤面（`Circle`）専用の分岐を除く）。
            //
            // `widget.paint()` を素の `widget` へ呼んで内容を反映しない理由: この
            // マッピングは `&Node<S>` の共有参照のみを持つ変換経路から呼ばれ、
            // `Widget::sync_from_state`（`&mut self` 要求、`fandhe_core` widget.rs
            // 参照）を先に呼ぶ手段がない。この状態で `paint()` を呼ぶとウィジェット
            // 構築時点の古いキャッシュ値を状態非依存に DOM へ反映してしまう
            // （TASK-8.3-S2 レビューで指摘、Issue #97）。
            //
            // Issue #989（`tilt_ball` の Web 配線）でこの制約を `clone_box()` した
            // 複製へ `sync_from_state(state)` をかけることで解消した: 複製は
            // このスコープでのみ生存する一時オブジェクトのため `&mut` を安全に取れ、
            // 元の `widget`（共有参照）には触れない。`circle_to_dom_node` で
            // 変換できる `Circle` を 1 つ以上持つウィジェット（`CanvasWidget` が
            // 唯一の出力元、`ball_commands` 参照）だけ盤面コンテナを描き、それ以外の
            // ウィジェットは reconcile/sync パス（S3・#98 のスコープ）確立後に本アーム
            // 全体を実マッピングへ置き換えるまで、従来どおりの空プレースホルダのまま
            // 据え置く。
            //
            // 複製・描画は `CanvasWidget` に限る（`as_any` の型判定は `sync_from_state`
            // 前でも変わらない）。他のウィジェットで DOM 再構築のたびに `paint` まで
            // 走らせないため（PR #997 codex 指摘）。
            let is_canvas = widget
                .as_any()
                .is_some_and(|any| any.is::<CanvasWidget<S>>());
            let canvas = is_canvas.then(|| {
                let mut synced = widget.clone_box();
                synced.sync_from_state(state);
                let size = synced.measure(Measurement {
                    width: f32::INFINITY,
                    height: f32::INFINITY,
                });
                let commands = synced.paint((0.0, 0.0), size);
                (synced, size, commands)
            });
            let circles: Vec<DomNode> = canvas
                .iter()
                .flat_map(|(_, size, commands)| {
                    commands.iter().filter_map(|c| {
                        circle_to_dom_node(c).or_else(|| inset_rect_to_dom_node(c, *size))
                    })
                })
                .collect();

            // ただし `Widget::children` （既定は空、`DialogWidget` は開状態のとき
            // 確認文言・ボタンを返す）経由の状態依存の子は `List` と同じ変換で
            // プレースホルダ div の子として出力する。ウィジェット本体（属性・イベント
            // フック）の描画は円が無い限り上記のとおり未配線のまま（Issue #739
            // codex-review 指摘）。
            let children: Vec<DomNode> = widget
                .children(state)
                .iter()
                .map(|child| convert_node(child, state, collector))
                .collect();

            // ドラッグを奪うキャンバス（`pen_draw`）は、点が 0 個の初期状態でも描き始める面が要るため
            // 円が無くてもコンテナを出し、指・ペンのドラッグでページがスクロールしないようにする。
            let captures_drag = widget
                .as_any()
                .and_then(|any| any.downcast_ref::<CanvasWidget<S>>())
                .is_some_and(|c| c.captures_drag());
            // 円が無くても `StyledRect` の背景（`system_accent_color_show` の色見本）は
            // コンテナの背景色で描く。さもないと Web では見本が空のプレースホルダになる。
            let background = canvas.as_ref().and_then(|(_, _, commands)| {
                commands.iter().find_map(|c| match c {
                    PaintCommand::StyledRect { fill } => Some(fill.to_hex()),
                    _ => None,
                })
            });
            if let (true, Some((synced, size, _))) = (
                captures_drag || !circles.is_empty() || background.is_some(),
                canvas,
            ) {
                let mut attrs = vec![
                    ("class".to_string(), "fandhe-canvas".to_string()),
                    ("role".to_string(), "img".to_string()),
                ];
                if let Some(label) = synced.accessibility_label() {
                    attrs.push(("aria-label".to_string(), label));
                }
                let mut style = format!(
                    "position:relative;width:{}px;height:{}px",
                    size.width, size.height
                );
                if captures_drag {
                    style.push_str(";touch-action:none");
                }
                if let Some(color) = background {
                    style.push_str(&format!(";background-color:{color}"));
                }
                attrs.push(("style".to_string(), style));
                DomNode {
                    tag: "div".to_string(),
                    attrs,
                    text: None,
                    children: circles.into_iter().chain(children).collect(),
                    event_hook: None,
                }
            } else {
                // 円が無い場合は従来どおりの空プレースホルダ表現を維持する
                // （既存ウィジェットの DOM 出力を変えないため、Issue #739）。
                // ただしラベルを持つウィジェット（`convert_input_widget` が扱わない
                // 残り 14 種、Issue #993 受入基準 (a)）は `role="group"` +
                // `aria-label` で a11y 名だけ出す（ダミー部品への置き換えはしない）。
                let mut attrs = Vec::new();
                if let Some(label) = widget.accessibility_label() {
                    attrs.push(("role".to_string(), "group".to_string()));
                    attrs.push(("aria-label".to_string(), label));
                }
                let text = if children.is_empty() {
                    Some(String::new())
                } else {
                    None
                };
                DomNode {
                    tag: "div".to_string(),
                    attrs,
                    text,
                    children,
                    event_hook: None,
                }
            }
        }
    }
}

/// `ImageWidget` を `img` へ変換する（Issue #1103、写真選択）。`src` へ出すのは
/// `blob:` で始まるソースだけ（橋渡し層が復号確認後に作った URL。`javascript:`・`data:`・
/// リモート URL・未配線の識別子は出さない、A03）。それ以外は `None` を返し、呼び出し元の
/// 空プレースホルダへ任せる。複製へ `sync_from_state` をかけて読む（`convert_input_widget` と同じ）。
fn convert_image_widget<S: 'static>(widget: &dyn Widget<S>, state: &S) -> Option<DomNode> {
    if !widget
        .as_any()
        .is_some_and(|any| any.is::<ImageWidget<S>>())
    {
        return None;
    }
    let mut synced = widget.clone_box();
    synced.sync_from_state(state);
    let img = synced.as_any()?.downcast_ref::<ImageWidget<S>>()?;
    let src = img.source_text();
    if !src.starts_with("blob:") {
        return None;
    }
    Some(DomNode {
        tag: "img".to_string(),
        attrs: vec![
            ("src".to_string(), src.to_string()),
            ("alt".to_string(), img.alt_text().to_string()),
            ("class".to_string(), "fandhe-image".to_string()),
            (
                "style".to_string(),
                "max-width:100%;height:auto".to_string(),
            ),
        ],
        text: None,
        children: Vec::new(),
        event_hook: None,
    })
}

/// `Checkbox`/`ToggleSwitch`/`Slider`/`RadioButton`（egui 系で実描画済みの 4 種、
/// `ui-widget-catalog` 対応表参照）をネイティブ `<input>` へ変換する（Issue #993）。
///
/// 対象外の型（`as_any` が上記 4 型のいずれとも一致しない）は `None` を返し、
/// 呼び出し元（[`convert_node`]）が従来の盤面 / 空プレースホルダ変換へフォールバックする。
///
/// `widget.clone_box()` した複製へ `sync_from_state(state)` をかけてから値を読む
/// （`Node::Widget` の他分岐と同じ設計判断、モジュール doc「未配線の契約」節参照）。
/// 複製は [`Rendered::widget_handlers`] へ蓄積し、[`super::mount`] の `change`
/// イベントが `handler_index` でこの複製の [`fandhe_core::Widget::apply_value`] を
/// 直接呼べるようにする（`id` 一致は描画に使ったのと同一の複製を経由するため
/// 構造的に保証され、`fandhe_core::ui::set_widget_value_at` のような再照合は不要）。
fn convert_input_widget<S: 'static>(
    widget: &dyn Widget<S>,
    state: &S,
    collector: &mut HandlerCollector<S>,
) -> Option<DomNode> {
    let any = widget.as_any()?;
    let is_checkbox = any.is::<Checkbox<S>>();
    let is_toggle = any.is::<ToggleSwitch<S>>();
    let is_slider = any.is::<Slider<S>>();
    let is_radio = any.is::<RadioButton<S>>();
    if !(is_checkbox || is_toggle || is_slider || is_radio) {
        return None;
    }

    let mut synced = widget.clone_box();
    synced.sync_from_state(state);
    let disabled = synced.interaction_state().disabled;

    let (label, mut input_attrs, kind) = if let Some(checkbox) = synced
        .as_any()
        .and_then(|any| any.downcast_ref::<Checkbox<S>>())
    {
        let mut attrs = vec![("type".to_string(), "checkbox".to_string())];
        if checkbox.is_checked() {
            attrs.push(("checked".to_string(), String::new()));
        }
        (
            checkbox.label_text().to_string(),
            attrs,
            WidgetValueKind::Bool,
        )
    } else if let Some(toggle) = synced
        .as_any()
        .and_then(|any| any.downcast_ref::<ToggleSwitch<S>>())
    {
        let mut attrs = vec![
            ("type".to_string(), "checkbox".to_string()),
            ("role".to_string(), "switch".to_string()),
        ];
        if toggle.is_on() {
            attrs.push(("checked".to_string(), String::new()));
        }
        (
            toggle.label_text().to_string(),
            attrs,
            WidgetValueKind::Bool,
        )
    } else if let Some(slider) = synced
        .as_any()
        .and_then(|any| any.downcast_ref::<Slider<S>>())
    {
        // `min`/`max`/`step` を `value` より先に設定する（設計判断、モジュール doc
        // マッピング方針表参照）。`step()` が `None` のときは `step="any"`
        // （ブラウザ既定の刻み無し）を明示する。
        let attrs = vec![
            ("type".to_string(), "range".to_string()),
            ("min".to_string(), slider.min().to_string()),
            ("max".to_string(), slider.max().to_string()),
            (
                "step".to_string(),
                slider
                    .step()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "any".to_string()),
            ),
            ("value".to_string(), slider.value().to_string()),
        ];
        (
            slider.label_text().to_string(),
            attrs,
            WidgetValueKind::Number,
        )
    } else {
        // 上の `is_checkbox`/`is_toggle`/`is_slider`/`is_radio` のいずれかが `true` で
        // 型判定した直後のため `?` が `None` を返すことはない（`as_any` は
        // `sync_from_state` 前後で型が変わらない、モジュール doc「未配線の契約」節参照）。
        let radio = synced
            .as_any()
            .and_then(|any| any.downcast_ref::<RadioButton<S>>())?;
        // `name` はグループ内で共通にする（codex レビュー指摘 #999: 未設定だと各選択肢が
        // 独立したラジオボタン扱いになり、矢印キーでの選択肢移動・Tab での単一巡回が
        // 壊れる）。同一グループの全 `RadioButton<S>` は同じ `get`（グループの現在選択値
        // を読む関数ポインタ）を共有する設計（radio_button.rs モジュール doc 参照）のため、
        // そのアドレスをグループ識別子として使う。異なる状態フィールドを読む別グループは
        // 別アドレスになり、自然に別 `name` へ分かれる。
        //
        // 加えて `collector.mount_id` を先頭に含める（codex レビュー指摘 P1、PR #999
        // 2 巡目: 同じ `settings_screen()` を同一ページの異なる root へ 2 回マウントすると
        // 各マウントは独立した状態 `S` を持つにもかかわらず `read_fn` の関数アドレスは
        // 同一のためラジオグループが衝突していた）。`mount_id` は
        // [`super::mount::Applier::mount_id`] と同じ「マウント呼び出しごとに一意」な値
        // （native シリアライズ・テストは `0` 固定）で、同一マウント内の全 `RadioButton`
        // は同じ `mount_id` を共有するため、グループ分けは従来どおり `read_fn` のみで決まる。
        let mut attrs = vec![
            ("type".to_string(), "radio".to_string()),
            (
                "name".to_string(),
                format!(
                    "radio-group-{}-{:x}",
                    collector.mount_id,
                    radio.read_fn() as usize
                ),
            ),
            ("value".to_string(), radio.value().to_string()),
        ];
        if radio.is_selected() {
            attrs.push(("checked".to_string(), String::new()));
        }
        (
            radio.label_text().to_string(),
            attrs,
            WidgetValueKind::Index(radio.value()),
        )
    };

    if disabled {
        input_attrs.push(("disabled".to_string(), String::new()));
    }

    let handler_index = collector.widget_handlers.len();
    collector.widget_handlers.push(synced);

    Some(DomNode {
        tag: "label".to_string(),
        attrs: Vec::new(),
        text: Some(label),
        children: vec![DomNode {
            tag: "input".to_string(),
            attrs: input_attrs,
            text: None,
            children: Vec::new(),
            event_hook: Some(EventHook::Change {
                handler_index,
                kind,
            }),
        }],
        event_hook: None,
    })
}

/// `PaintCommand::Circle`（#828）を `border-radius: 50%` の `div` へ変換する。
///
/// 上記マッピング方針表（`Node<S>` バリアント → HTML 要素）とは別枠の変換関数
/// （`PaintCommand` は `Node<S>` ではない）。`Node::Widget` 分岐から本関数を実際に
/// 呼ぶ配線は #989（型サンプル `tilt_ball` の Web 配線）で完了済み（上記
/// `convert_node` の `Node::Widget` アーム参照）。`Circle` 以外のバリアントは
/// `None` を返す。
pub fn circle_to_dom_node(command: &PaintCommand) -> Option<DomNode> {
    let PaintCommand::Circle {
        center,
        radius,
        fill,
    } = command
    else {
        return None;
    };
    let diameter = radius * 2.0;
    let left = center.0 - radius;
    let top = center.1 - radius;
    Some(DomNode {
        tag: "div".to_string(),
        attrs: vec![
            ("class".to_string(), "fandhe-circle".to_string()),
            (
                "style".to_string(),
                format!(
                    "position:absolute;left:{left}px;top:{top}px;width:{diameter}px;height:{diameter}px;border-radius:50%;background-color:{};",
                    fill.to_hex()
                ),
            ),
        ],
        text: None,
        children: Vec::new(),
        event_hook: None,
    })
}

/// `PaintCommand::InsetRect`（#444、Issue #447）を、キャンバス `size` の各辺から内側へ寄せた
/// 絶対配置の `div` へ変換する。反転する軸は egui 側（`paint_command::inset_rect`）と同じく
/// 大きさ 0 に潰す。`InsetRect` 以外は `None`。`Node::Widget` の canvas 分岐が円と並べて呼ぶ。
pub fn inset_rect_to_dom_node(command: &PaintCommand, size: Measurement) -> Option<DomNode> {
    let PaintCommand::InsetRect {
        left,
        top,
        right,
        bottom,
        fill,
    } = command
    else {
        return None;
    };
    let w = (size.width - left - right).max(0.0);
    let h = (size.height - top - bottom).max(0.0);
    Some(DomNode {
        tag: "div".to_string(),
        attrs: vec![
            ("class".to_string(), "fandhe-inset-rect".to_string()),
            (
                "style".to_string(),
                format!(
                    "position:absolute;left:{left}px;top:{top}px;width:{w}px;height:{h}px;background-color:{};",
                    fill.to_hex()
                ),
            ),
        ],
        text: None,
        children: Vec::new(),
        event_hook: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::sample_screen;

    #[test]
    fn inset_rect_to_dom_node_places_and_collapses() {
        use fandhe_core::token::ColorValue;
        let size = Measurement {
            width: 240.0,
            height: 200.0,
        };
        let cmd = |l, t, r, b| PaintCommand::InsetRect {
            left: l,
            top: t,
            right: r,
            bottom: b,
            fill: ColorValue::rgb(1, 2, 3),
        };
        let style = |n: DomNode| n.attrs.into_iter().find(|(k, _)| k == "style").unwrap().1;
        let s = style(inset_rect_to_dom_node(&cmd(10.0, 20.0, 30.0, 40.0), size).unwrap());
        assert!(
            s.contains("left:10px;top:20px;width:200px;height:140px;"),
            "{s}"
        );
        let s = style(inset_rect_to_dom_node(&cmd(200.0, 0.0, 100.0, 300.0), size).unwrap());
        assert!(s.contains("width:0px;height:0px;"), "{s}");
        assert!(inset_rect_to_dom_node(&PaintCommand::Rect, size).is_none());
    }

    /// `foldable_posture_show`: 読めたときだけ canvas と InsetRect が出る。縦の折れ目で 2 枚。
    #[test]
    fn foldable_posture_canvas_only_when_read() {
        use fandhe_core::screens::foldable_posture_show::{
            apply_read, foldable_posture_show_web_screen, Fold, FoldInfo, FoldOrientation,
            FoldablePostureShowState, Posture, PostureReading, ReadFailure,
        };
        let html = |s: &FoldablePostureShowState| {
            crate::html::to_html(&build_dom_tree(&foldable_posture_show_web_screen(), s, 0))
        };
        let mut state = FoldablePostureShowState::default();
        assert!(!html(&state).contains("fandhe-inset-rect"));
        apply_read(&mut state, Err(ReadFailure::Unsupported));
        assert!(!html(&state).contains("fandhe-inset-rect"));
        apply_read(&mut state, Err(ReadFailure::Failed("x".into())));
        assert!(!html(&state).contains("fandhe-inset-rect"));
        apply_read(
            &mut state,
            Ok(PostureReading {
                posture: Posture::Folded,
                fold: FoldInfo::Known(Fold {
                    orientation: FoldOrientation::Vertical,
                    start: 400.0,
                    thickness: 20.0,
                    extent: 820.0,
                }),
            }),
        );
        let h = html(&state);
        assert!(h.contains("fandhe-canvas"), "{h}");
        assert_eq!(h.matches("fandhe-inset-rect").count(), 2, "{h}");
        assert!(h.contains("折れている") && h.contains("折れ目: 縦"), "{h}");
    }

    /// `safe_area_insets_show`: 読めたときだけ canvas と InsetRect が出る（嘘の成功を出さない）。
    #[test]
    fn safe_area_insets_canvas_only_when_read() {
        use fandhe_core::screens::safe_area_insets_show::{
            apply_read, safe_area_insets_show_web_screen, Insets, ReadFailure,
            SafeAreaInsetsShowState,
        };
        let html = |s: &SafeAreaInsetsShowState| {
            crate::html::to_html(&build_dom_tree(&safe_area_insets_show_web_screen(), s, 0))
        };
        let mut state = SafeAreaInsetsShowState::default();
        assert!(!html(&state).contains("fandhe-inset-rect"));
        apply_read(&mut state, Err(ReadFailure::Unsupported));
        assert!(!html(&state).contains("fandhe-inset-rect"));
        apply_read(&mut state, Err(ReadFailure::Failed("x".into())));
        assert!(!html(&state).contains("fandhe-inset-rect"));
        apply_read(
            &mut state,
            Ok(Some(Insets {
                top: 47.0,
                right: 0.0,
                bottom: 34.0,
                left: 5.0,
            })),
        );
        let h = html(&state);
        assert!(h.contains("fandhe-canvas") && h.contains("fandhe-inset-rect"));
        assert!(h.contains("left:5px;top:47px;"), "{h}");
    }
    use fandhe_core::WidgetId;

    struct CounterState {
        note: String,
        items: Vec<String>,
    }

    fn get_note(state: &CounterState) -> String {
        state.note.clone()
    }
    fn set_note(state: &mut CounterState, value: String) {
        state.note = value;
    }
    fn on_click(_state: &mut CounterState) {}
    fn expand_items(state: &CounterState) -> Vec<Node<CounterState>> {
        state.items.iter().cloned().map(Node::Text).collect()
    }

    fn empty_state() -> CounterState {
        CounterState {
            note: String::new(),
            items: Vec::new(),
        }
    }

    #[test]
    fn title_becomes_h1_as_first_child() {
        let screen: Screen<CounterState> = Screen::new("見出し画面");
        let state = empty_state();

        let dom = build_dom_tree(&screen, &state, 0);

        assert_eq!(dom.tag, "div");
        assert_eq!(dom.children[0].tag, "h1");
        assert_eq!(dom.children[0].text.as_deref(), Some("見出し画面"));
    }

    #[test]
    fn text_node_maps_to_p_element() {
        let screen: Screen<CounterState> = Screen::new("test").text("本文");
        let state = empty_state();

        let dom = build_dom_tree(&screen, &state, 0);

        assert_eq!(dom.children[1].tag, "p");
        assert_eq!(dom.children[1].text.as_deref(), Some("本文"));
    }

    /// `Node::DynamicText` は `Node::Text` と同じ `p` 要素へ変換され、`get(state)` の
    /// 現在値が反映されることを検証する（Issue #581）。
    #[test]
    fn dynamic_text_maps_to_p_element_with_current_value() {
        fn get_note_label(state: &CounterState) -> String {
            format!("メモ: {}", state.note)
        }

        let screen: Screen<CounterState> = Screen::new("test").dynamic_text(get_note_label);
        let mut state = empty_state();
        state.note = "下書き".to_string();

        let dom = build_dom_tree(&screen, &state, 0);

        assert_eq!(dom.children[1].tag, "p");
        assert_eq!(dom.children[1].text.as_deref(), Some("メモ: 下書き"));
    }

    #[test]
    fn styled_text_error_maps_to_p_with_alert_role_and_class() {
        fn get_error_label(state: &CounterState) -> String {
            state.note.clone()
        }

        let screen: Screen<CounterState> = Screen::new("test").error_text(get_error_label);
        let mut state = empty_state();
        state.note = "名前を入力してください".to_string();

        let dom = build_dom_tree(&screen, &state, 0);

        assert_eq!(dom.children[1].tag, "p");
        assert_eq!(
            dom.children[1].attrs,
            vec![
                ("role".to_string(), "alert".to_string()),
                ("class".to_string(), "fandhe-text-error".to_string()),
            ]
        );
        assert_eq!(
            dom.children[1].text.as_deref(),
            Some("名前を入力してください")
        );
    }

    #[test]
    fn styled_text_body_maps_to_plain_p() {
        fn get_body_label(state: &CounterState) -> String {
            state.note.clone()
        }

        let node: Node<CounterState> = Node::StyledText {
            role: TextRole::Body,
            get: get_body_label,
        };
        let mut collector = HandlerCollector {
            click_handlers: Vec::new(),
            input_handlers: Vec::new(),
            widget_handlers: Vec::new(),
            mount_id: 0,
        };
        let mut state = empty_state();
        state.note = "本文".to_string();

        let dom = convert_node(&node, &state, &mut collector);

        assert_eq!(dom.tag, "p");
        assert!(dom.attrs.is_empty());
        assert_eq!(dom.text.as_deref(), Some("本文"));
    }

    #[test]
    fn text_field_maps_to_label_and_input_with_current_value() {
        let screen: Screen<CounterState> =
            Screen::new("test").text_field("メモ", get_note, set_note);
        let mut state = empty_state();
        state.note = "下書き".to_string();

        let dom = build_dom_tree(&screen, &state, 0);
        let label_node = &dom.children[1];

        assert_eq!(label_node.tag, "label");
        assert_eq!(label_node.text.as_deref(), Some("メモ"));
        let input_node = &label_node.children[0];
        assert_eq!(input_node.tag, "input");
        assert_eq!(
            input_node.attrs,
            vec![("value".to_string(), "下書き".to_string())]
        );
        assert_eq!(
            input_node.event_hook,
            Some(EventHook::Input { handler_index: 0 })
        );
    }

    #[test]
    fn button_maps_to_button_with_click_hook() {
        let screen: Screen<CounterState> =
            Screen::new("test").button(WidgetId::from_raw(1), "押す", on_click);
        let state = empty_state();

        let dom = build_dom_tree(&screen, &state, 0);
        let button_node = &dom.children[1];

        assert_eq!(button_node.tag, "button");
        assert_eq!(button_node.text.as_deref(), Some("押す"));
        assert_eq!(
            button_node.event_hook,
            Some(EventHook::Click { handler_index: 0 })
        );
    }

    #[test]
    fn list_maps_to_ul_with_li_wrapped_items_reflecting_state() {
        let screen: Screen<CounterState> = Screen::new("test").list(expand_items);
        let mut state = empty_state();
        state.items = vec!["a".to_string()];

        let dom = build_dom_tree(&screen, &state, 0);
        let list_node = &dom.children[1];
        assert_eq!(list_node.tag, "ul");
        assert_eq!(list_node.children.len(), 1);
        assert_eq!(list_node.children[0].tag, "li");
        assert_eq!(list_node.children[0].children[0].text.as_deref(), Some("a"));

        state.items.push("b".to_string());
        let dom_after = build_dom_tree(&screen, &state, 0);
        assert_eq!(dom_after.children[1].children.len(), 2);
        assert_eq!(
            dom_after.children[1].children[1].children[0]
                .text
                .as_deref(),
            Some("b")
        );
    }

    #[test]
    fn nested_list_becomes_nested_ul() {
        fn inner_expand(_state: &CounterState) -> Vec<Node<CounterState>> {
            vec![Node::Text("inner".to_string())]
        }
        fn outer_expand(_state: &CounterState) -> Vec<Node<CounterState>> {
            vec![Node::List {
                expand: inner_expand,
                axis: fandhe_core::Axis::default(),
            }]
        }

        let screen: Screen<CounterState> = Screen::new("test").list(outer_expand);
        let state = empty_state();

        let dom = build_dom_tree(&screen, &state, 0);
        let outer_ul = &dom.children[1];
        assert_eq!(outer_ul.tag, "ul");
        let outer_li = &outer_ul.children[0];
        assert_eq!(outer_li.tag, "li");
        let inner_ul = &outer_li.children[0];
        assert_eq!(inner_ul.tag, "ul");
        assert_eq!(inner_ul.children[0].tag, "li");
        assert_eq!(
            inner_ul.children[0].children[0].text.as_deref(),
            Some("inner")
        );
    }

    #[test]
    fn multiple_buttons_get_distinct_click_handler_indices() {
        let screen: Screen<CounterState> = Screen::new("test")
            .button(WidgetId::from_raw(1), "a", on_click)
            .button(WidgetId::from_raw(2), "b", on_click);
        let state = empty_state();

        let dom = build_dom_tree(&screen, &state, 0);

        assert_eq!(
            dom.children[1].event_hook,
            Some(EventHook::Click { handler_index: 0 })
        );
        assert_eq!(
            dom.children[2].event_hook,
            Some(EventHook::Click { handler_index: 1 })
        );
    }

    #[test]
    fn click_and_input_handlers_are_invocable_at_their_handler_index() {
        // `render()` を直接呼び出し、`Rendered::click_handlers` / `input_handlers` の
        // 中身・順序が `event_hook.handler_index` と対応することを検証する
        // （`mount.rs` が依存する重要な契約: `handler_index` で引いた関数ポインタが
        // 実際に期待する状態更新を引き起こすこと）。
        fn set_flag_a(state: &mut CounterState) {
            state.note = "a-clicked".to_string();
        }
        fn set_flag_b(state: &mut CounterState) {
            state.note = "b-clicked".to_string();
        }
        fn set_note_x(state: &mut CounterState, value: String) {
            state.note = format!("x:{value}");
        }
        fn set_note_y(state: &mut CounterState, value: String) {
            state.note = format!("y:{value}");
        }
        fn get_dummy(_state: &CounterState) -> String {
            String::new()
        }

        let screen: Screen<CounterState> = Screen::new("test")
            .button(WidgetId::from_raw(1), "a", set_flag_a)
            .text_field("メモ1", get_dummy, set_note_x)
            .button(WidgetId::from_raw(2), "b", set_flag_b)
            .text_field("メモ2", get_dummy, set_note_y);
        let mut state = empty_state();

        let rendered = render(&screen, &state, 0);
        assert_eq!(rendered.click_handlers.len(), 2);
        assert_eq!(rendered.input_handlers.len(), 2);

        // click_handlers[1] は 2 番目のボタン（"b"）の on_click と一致し、
        // 呼び出すと状態が期待通り変わることを検証する。
        rendered.click_handlers[1](&mut state);
        assert_eq!(state.note, "b-clicked");

        rendered.click_handlers[0](&mut state);
        assert_eq!(state.note, "a-clicked");

        rendered.input_handlers[1](&mut state, "値2".to_string());
        assert_eq!(state.note, "y:値2");

        rendered.input_handlers[0](&mut state, "値1".to_string());
        assert_eq!(state.note, "x:値1");

        // DOM 側の handler_index も対応する採番順になっていることを確認する。
        assert_eq!(
            rendered.dom.children[1].event_hook,
            Some(EventHook::Click { handler_index: 0 })
        );
        assert_eq!(
            rendered.dom.children[3].event_hook,
            Some(EventHook::Click { handler_index: 1 })
        );
        // input 側も同様に、複数 TextField の handler_index が採番順（`mount.rs` の
        // `InputElements` が依存する対応関係）どおりであることを確認する。
        assert_eq!(
            rendered.dom.children[2].children[0].event_hook,
            Some(EventHook::Input { handler_index: 0 })
        );
        assert_eq!(
            rendered.dom.children[4].children[0].event_hook,
            Some(EventHook::Input { handler_index: 1 })
        );
    }

    #[test]
    fn sample_screen_converts_without_panicking() {
        let screen = sample_screen();
        let state = fandhe_core::SampleState::default();

        let dom = build_dom_tree(&screen, &state, 0);

        // 先頭の h1（タイトル）+ トップレベルノード 1 件につき DomNode 1 件
        // （`convert_node` は 1:1 変換）。sample_screen() の要素数を件数へ
        // 固定せず screen.len() から都度導出する（Issue #694）。
        assert_eq!(dom.children.len(), screen.len() + 1);
        assert_eq!(dom.children[0].text.as_deref(), Some(screen.title()));
    }

    /// `Node::Widget` は状態非依存の空 `div` プレースホルダへ変換されることを
    /// 検証する（`widget.paint()` を呼ばない、TASK-8.3-S2 レビュー指摘・Issue #97）。
    #[test]
    fn widget_node_converts_to_empty_placeholder_div() {
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

        let dom = build_dom_tree(&screen, &state, 0);

        // タイトル（h1）+ widget = 2 要素。
        assert_eq!(dom.children.len(), 2);
        let widget_dom = &dom.children[1];
        assert_eq!(widget_dom.tag, "div");
        assert_eq!(widget_dom.text.as_deref(), Some(""));
        assert!(widget_dom.children.is_empty());
    }

    /// `Node::Widget(DialogWidget)` は `Widget::children` 経由の子ノード（確認文言 +
    /// 「キャンセル」「削除」）を出力し、非表示時（`pending = None`）は出さないことを
    /// 検証する（PR #792 codex-review 2 巡目 P1 指摘、Issue #739）。
    #[test]
    fn confirm_dialog_widget_children_appear_only_while_pending() {
        use fandhe_core::screens::confirm_dialog::{confirm_dialog_screen, ConfirmDialogState};

        fn texts(node: &DomNode, out: &mut Vec<String>) {
            if let Some(text) = &node.text {
                out.push(text.clone());
            }
            for child in &node.children {
                texts(child, out);
            }
        }

        // 一覧行にも「削除」ボタン（`IndexedButton`）が常に出るため、確認区画の
        // 出現有無は `Node::Widget` の変換結果（末尾の子）に限定して確認する。

        let mut state = ConfirmDialogState::default();
        let screen = confirm_dialog_screen(&state);
        let dom = build_dom_tree(&screen, &state, 0);
        let widget_dom = dom.children.last().expect("widget ノードが末尾にある");
        let mut out = Vec::new();
        texts(widget_dom, &mut out);
        assert!(!out.contains(&"「メモ 1」を削除しますか？".to_string()));
        assert!(!out.contains(&"キャンセル".to_string()));
        assert!(!out.contains(&"削除".to_string()));

        state.pending = Some(0);
        let screen = confirm_dialog_screen(&state);
        let dom = build_dom_tree(&screen, &state, 0);
        let widget_dom = dom.children.last().expect("widget ノードが末尾にある");
        let mut out = Vec::new();
        texts(widget_dom, &mut out);
        assert!(out.contains(&"「メモ 1」を削除しますか？".to_string()));
        assert!(out.contains(&"キャンセル".to_string()));
        assert!(out.contains(&"削除".to_string()));
    }

    #[test]
    fn circle_to_dom_node_computes_bounding_box_and_color() {
        use fandhe_core::token::ColorValue;

        let command = PaintCommand::Circle {
            center: (10.0, 20.0),
            radius: 5.0,
            fill: ColorValue::rgba(255, 0, 0, 255),
        };
        let dom = circle_to_dom_node(&command).expect("Circle は Some を返す契約");
        assert_eq!(dom.tag, "div");
        let style = dom
            .attrs
            .iter()
            .find(|(name, _)| name == "style")
            .map(|(_, value)| value.as_str())
            .expect("style 属性を持つ");
        assert!(style.contains("left:5px"));
        assert!(style.contains("top:15px"));
        assert!(style.contains("width:10px"));
        assert!(style.contains("height:10px"));
        assert!(style.contains("border-radius:50%"));
        assert!(style.contains(&format!(
            "background-color:{};",
            ColorValue::rgba(255, 0, 0, 255).to_hex()
        )));
    }

    #[test]
    fn circle_to_dom_node_returns_none_for_non_circle_commands() {
        assert!(circle_to_dom_node(&PaintCommand::Rect).is_none());
        assert!(circle_to_dom_node(&PaintCommand::Text("x".to_string())).is_none());
    }

    /// drag capture 付きキャンバス（`pen_draw`）は点が 0 個でもコンテナを出し、`touch-action:none` を付ける
    /// （Issue #358）。drag capture の無い `tilt_ball` の style は変えない。
    #[test]
    fn drag_capture_canvas_renders_empty_container_with_touch_action_none() {
        use fandhe_core::screens::pen_draw::{current_screen, PenDrawState};
        use fandhe_core::screens::tilt_ball::{tilt_ball_screen, TiltBallState};

        fn canvas_style(node: &DomNode) -> Option<String> {
            if node
                .attrs
                .iter()
                .any(|(k, v)| k == "class" && v == "fandhe-canvas")
            {
                return node
                    .attrs
                    .iter()
                    .find(|(k, _)| k == "style")
                    .map(|(_, v)| v.clone());
            }
            node.children.iter().find_map(canvas_style)
        }

        let pen = PenDrawState::default();
        let style = canvas_style(&build_dom_tree(&current_screen(&pen), &pen, 0)).unwrap();
        assert!(style.contains("width:320px"));
        assert!(style.contains("touch-action:none"));

        let ball = TiltBallState::default();
        let style = canvas_style(&build_dom_tree(&tilt_ball_screen(), &ball, 0)).unwrap();
        assert!(!style.contains("touch-action"));
    }

    /// `StyledRect` だけの Canvas（`system_accent_color_show`）が背景色付きで出ること。
    /// 読めていない状態では見本を描かない（嘘の成功を出さない）。
    #[test]
    fn styled_rect_only_canvas_renders_background() {
        use fandhe_core::screens::system_accent_color_show::{
            apply_read, system_accent_color_show_web_screen, Rgb, SystemAccentColorShowState,
        };

        let mut state = SystemAccentColorShowState::default();
        let html = crate::html::to_html(&build_dom_tree(
            &system_accent_color_show_web_screen(),
            &state,
            0,
        ));
        assert!(!html.contains("background-color"));

        apply_read(
            &mut state,
            Ok(Rgb {
                r: 10,
                g: 132,
                b: 255,
            }),
        );
        let html = crate::html::to_html(&build_dom_tree(
            &system_accent_color_show_web_screen(),
            &state,
            0,
        ));
        assert!(html.contains("fandhe-canvas"));
        assert!(html.contains("background-color:#0a84ffff"));
    }

    /// `Node::Widget(CanvasWidget)` が円を実際に描くことを確認する（Issue #989、
    /// `tilt_ball` の盤面が Web でも見える契約）。`fandhe-circle` の `left` は
    /// `BALL_RADIUS`（玉半径）を引いた値になる（`circle_to_dom_node` は中心座標から
    /// 半径ぶん引いた位置を左上に置く）。
    #[test]
    fn tilt_ball_screen_renders_ball_as_fandhe_circle_div() {
        use fandhe_core::screens::tilt_ball::{tilt_ball_screen, TiltBallState};

        let state = TiltBallState::default();
        let screen = tilt_ball_screen();
        let dom = build_dom_tree(&screen, &state, 0);

        fn find_canvas(node: &DomNode) -> Option<&DomNode> {
            if node
                .attrs
                .iter()
                .any(|(k, v)| k == "class" && v == "fandhe-canvas")
            {
                return Some(node);
            }
            node.children.iter().find_map(find_canvas)
        }

        let canvas = find_canvas(&dom).expect("盤面コンテナ div が出力される");
        let circle = canvas
            .children
            .iter()
            .find(|c| {
                c.attrs
                    .iter()
                    .any(|(k, v)| k == "class" && v == "fandhe-circle")
            })
            .expect("玉の div が出力される");
        let style = circle
            .attrs
            .iter()
            .find(|(name, _)| name == "style")
            .map(|(_, value)| value.as_str())
            .expect("style 属性を持つ");
        // 既定状態: 玉は盤面中央（160, 160）、半径 14。
        assert!(style.contains("left:146"));
        assert!(style.contains("top:146"));
    }

    /// 右へ何ステップか進めると `left` が増える（`step` が反映されることの回帰確認）。
    #[test]
    fn tilt_ball_screen_ball_position_moves_with_step() {
        use fandhe_core::screens::tilt_ball::{step, tilt_ball_screen, TiltBallState};

        let mut state = TiltBallState::default();
        for _ in 0..10 {
            step(&mut state, (1.0, 0.0), 1.0 / 60.0);
        }
        let screen = tilt_ball_screen();
        let dom = build_dom_tree(&screen, &state, 0);

        fn find_circle(node: &DomNode) -> Option<&DomNode> {
            if node
                .attrs
                .iter()
                .any(|(k, v)| k == "class" && v == "fandhe-circle")
            {
                return Some(node);
            }
            node.children.iter().find_map(find_circle)
        }
        let circle = find_circle(&dom).expect("玉の div が出力される");
        let style = circle
            .attrs
            .iter()
            .find(|(name, _)| name == "style")
            .map(|(_, value)| value.as_str())
            .expect("style 属性を持つ");
        assert!(!style.contains("left:146"));
    }

    // --- Issue #993: Checkbox/ToggleSwitch/Slider/RadioButton の実 DOM マッピング ---

    fn find_input<'a>(node: &'a DomNode, attr: &str, value: &str) -> Option<&'a DomNode> {
        if node.tag == "input" && node.attrs.iter().any(|(k, v)| k == attr && v == value) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|c| find_input(c, attr, value))
    }

    fn has_attr(node: &DomNode, name: &str, value: &str) -> bool {
        node.attrs.iter().any(|(k, v)| k == name && v == value)
    }

    #[test]
    fn settings_screen_toggle_switch_maps_to_switch_role_checkbox_input() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let state = SettingsState::default();

        let dom = build_dom_tree(&screen, &state, 0);
        let input = find_input(&dom, "role", "switch").expect("switch 入力が出力される");

        assert!(has_attr(input, "type", "checkbox"));
        // 既定状態は通知オン。
        assert!(has_attr(input, "checked", ""));
    }

    #[test]
    fn settings_screen_slider_maps_to_range_input_with_min_max_step_value() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let state = SettingsState::default();

        let dom = build_dom_tree(&screen, &state, 0);
        let input = find_input(&dom, "type", "range").expect("range 入力が出力される");

        assert!(has_attr(input, "min", "12"));
        assert!(has_attr(input, "max", "24"));
        assert!(has_attr(input, "step", "1"));
        // 既定の文字サイズ 16。
        assert!(has_attr(input, "value", "16"));
    }

    #[test]
    fn settings_screen_radio_group_checks_only_matching_index() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let state = SettingsState::default();

        let dom = build_dom_tree(&screen, &state, 0);

        fn radio_inputs<'a>(node: &'a DomNode, out: &mut Vec<&'a DomNode>) {
            if node.tag == "input" && node.attrs.iter().any(|(k, v)| k == "type" && v == "radio") {
                out.push(node);
            }
            for child in &node.children {
                radio_inputs(child, out);
            }
        }
        let mut radios = Vec::new();
        radio_inputs(&dom, &mut radios);

        // 既定のテーマは System（index 0）。
        assert_eq!(radios.len(), 3);
        assert!(has_attr(radios[0], "value", "0"));
        assert!(has_attr(radios[0], "checked", ""));
        assert!(!has_attr(radios[1], "checked", ""));
        assert!(!has_attr(radios[2], "checked", ""));
    }

    // Issue #999 codex レビュー指摘: name 未設定だと各選択肢が独立したラジオボタン扱いに
    // なり、矢印キー選択・Tab 巡回が壊れる。同一グループ内で name が共通であることを検証する。
    #[test]
    fn settings_screen_radio_group_shares_common_name() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let state = SettingsState::default();

        let dom = build_dom_tree(&screen, &state, 0);

        fn radio_inputs<'a>(node: &'a DomNode, out: &mut Vec<&'a DomNode>) {
            if node.tag == "input" && node.attrs.iter().any(|(k, v)| k == "type" && v == "radio") {
                out.push(node);
            }
            for child in &node.children {
                radio_inputs(child, out);
            }
        }
        let mut radios = Vec::new();
        radio_inputs(&dom, &mut radios);

        assert_eq!(radios.len(), 3);
        let name = |node: &DomNode| {
            node.attrs
                .iter()
                .find(|(k, _)| k == "name")
                .map(|(_, v)| v.clone())
                .expect("radio input に name 属性がある")
        };
        let names: Vec<String> = radios.iter().map(|r| name(r)).collect();
        assert!(!names[0].is_empty());
        assert_eq!(names[0], names[1]);
        assert_eq!(names[0], names[2]);
    }

    // codex レビュー指摘 P1（PR #999 2 巡目）: `name` が `read_fn` のアドレスのみで
    // 決まると、同じ `settings_screen()` を同一ページの異なる root に 2 回マウントした
    // ときに、独立した状態を持つ両画面のラジオグループが同一 `name` になり衝突する
    // （一方の選択操作でもう一方の選択表示が外れる）。`mount_id` を含めることで
    // マウントをまたいだグループ名が分かれることを検証する。
    #[test]
    fn settings_screen_radio_group_name_differs_across_mounts() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        fn radio_names(mount_id: u64) -> Vec<String> {
            let screen = settings_screen();
            let state = SettingsState::default();
            let dom = build_dom_tree(&screen, &state, mount_id);

            fn radio_inputs<'a>(node: &'a DomNode, out: &mut Vec<&'a DomNode>) {
                if node.tag == "input"
                    && node.attrs.iter().any(|(k, v)| k == "type" && v == "radio")
                {
                    out.push(node);
                }
                for child in &node.children {
                    radio_inputs(child, out);
                }
            }
            let mut radios = Vec::new();
            radio_inputs(&dom, &mut radios);
            radios
                .iter()
                .map(|node| {
                    node.attrs
                        .iter()
                        .find(|(k, _)| k == "name")
                        .map(|(_, v)| v.clone())
                        .expect("radio input に name 属性がある")
                })
                .collect()
        }

        let names_a = radio_names(0);
        let names_b = radio_names(1);

        // 同一マウント内ではグループ名が共通のまま（既存契約の維持）。
        assert_eq!(names_a[0], names_a[1]);
        // 別マウントとは常に異なる（衝突回避）。
        assert_ne!(names_a[0], names_b[0]);
    }

    #[test]
    fn settings_screen_has_five_widget_handlers_with_sequential_change_hooks() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let state = SettingsState::default();

        let rendered = render(&screen, &state, 0);

        // toggle_switch(id1) + slider(id2) + radio_button(id3..5) の 5 件
        // （`button` は `Node::Widget` を経由しないため対象外）。
        assert_eq!(rendered.widget_handlers.len(), 5);

        let mut seen = Vec::new();
        fn collect_change_indices(node: &DomNode, out: &mut Vec<usize>) {
            if let Some(EventHook::Change { handler_index, .. }) = node.event_hook {
                out.push(handler_index);
            }
            for child in &node.children {
                collect_change_indices(child, out);
            }
        }
        collect_change_indices(&rendered.dom, &mut seen);
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn checkbox_alone_maps_to_checkbox_input_without_role() {
        struct FormState {
            agreed: bool,
        }
        fn get_agreed(state: &FormState) -> bool {
            state.agreed
        }
        fn set_agreed(state: &mut FormState, value: bool) {
            state.agreed = value;
        }

        let screen: Screen<FormState> =
            Screen::new("test").checkbox(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        let state = FormState { agreed: false };

        let dom = build_dom_tree(&screen, &state, 0);
        let label_node = &dom.children[1];
        assert_eq!(label_node.tag, "label");
        assert_eq!(label_node.text.as_deref(), Some("同意する"));
        let input = &label_node.children[0];
        assert_eq!(input.tag, "input");
        assert!(has_attr(input, "type", "checkbox"));
        assert!(!input.attrs.iter().any(|(k, _)| k == "role"));
        assert!(!input.attrs.iter().any(|(k, _)| k == "checked"));
    }

    #[test]
    fn widget_handler_apply_value_updates_state_and_rerender_reflects_it() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let mut state = SettingsState::default();

        let rendered = render(&screen, &state, 0);
        // radio_button(id5, Dark, index=2) は 5 件中 5 番目（handler_index 4）。
        let dark = &rendered.widget_handlers[4];
        assert!(dark.apply_value(&mut state, &fandhe_core::WidgetValue::Index(2)));
        assert_eq!(state.theme, fandhe_core::screens::settings::Theme::Dark);

        let dom_after = build_dom_tree(&screen, &state, 0);
        fn radio_inputs<'a>(node: &'a DomNode, out: &mut Vec<&'a DomNode>) {
            if node.tag == "input" && node.attrs.iter().any(|(k, v)| k == "type" && v == "radio") {
                out.push(node);
            }
            for child in &node.children {
                radio_inputs(child, out);
            }
        }
        let mut radios = Vec::new();
        radio_inputs(&dom_after, &mut radios);
        assert!(has_attr(radios[2], "checked", ""));
        assert!(!has_attr(radios[0], "checked", ""));

        fn find_text(node: &DomNode, needle: &str) -> bool {
            if node.text.as_deref().is_some_and(|t| t.contains(needle)) {
                return true;
            }
            node.children.iter().any(|c| find_text(c, needle))
        }
        assert!(find_text(&dom_after, "テーマ: ダーク"));
    }

    #[test]
    fn widget_handler_apply_value_rejects_type_mismatch_and_leaves_state_unchanged() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let mut state = SettingsState::default();
        let original = state.clone();

        let rendered = render(&screen, &state, 0);
        // handler_index 0 は toggle_switch（`Bool` を扱う）。`Index` を渡すと拒否される。
        let toggle = &rendered.widget_handlers[0];
        let applied = toggle.apply_value(&mut state, &fandhe_core::WidgetValue::Index(1));

        assert!(!applied);
        assert_eq!(state, original);
    }

    #[test]
    fn settings_screen_to_html_contains_switch_role_and_range_type() {
        use fandhe_core::screens::settings::{settings_screen, SettingsState};

        let screen = settings_screen();
        let state = SettingsState::default();

        let html = super::super::html::to_html(&build_dom_tree(&screen, &state, 0));

        assert!(html.contains(r#"role="switch""#));
        assert!(html.contains(r#"type="range""#));
    }

    /// 未対応ウィジェット（`convert_input_widget` が扱わない残り 14 種）は空
    /// プレースホルダのままだが、`accessibility_label` を持つ場合は `role="group"` +
    /// `aria-label` で a11y 名を出す（Issue #993 受入基準 (a)）。
    #[test]
    fn unsupported_widget_with_label_gets_group_role_and_aria_label() {
        use fandhe_core::{
            AccessibilityRole, InteractionState, Measurement, Node, PaintCommand, Screen, Widget,
            WidgetId,
        };

        struct DummyState;

        #[derive(Debug, Clone)]
        struct LabeledStubWidget;

        impl Widget<DummyState> for LabeledStubWidget {
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
                Vec::new()
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
            fn accessibility_label(&self) -> Option<String> {
                Some("未対応部品".to_string())
            }
            fn interaction_state(&self) -> InteractionState {
                InteractionState::default()
            }
            fn clone_box(&self) -> Box<dyn Widget<DummyState>> {
                Box::new(self.clone())
            }
        }

        let screen: Screen<DummyState> =
            Screen::new("画面").with_node(Node::Widget(Box::new(LabeledStubWidget)));
        let state = DummyState;

        let dom = build_dom_tree(&screen, &state, 0);
        let widget_dom = &dom.children[1];

        assert_eq!(widget_dom.tag, "div");
        assert!(has_attr(widget_dom, "role", "group"));
        assert!(has_attr(widget_dom, "aria-label", "未対応部品"));
    }

    /// 実画素の画像ノード（Issue #5）: `img` + PNG の data URL + alt で出る。
    #[test]
    fn rgba_image_emits_img_with_png_data_url() {
        let pixels = fandhe_core::RgbaPixels::new(1, 1, vec![1u8, 2, 3, 4]).unwrap();
        let screen: Screen<CounterState> = Screen::new("t").rgba_image(pixels, "サムネイル");
        let dom = build_dom_tree(&screen, &empty_state(), 0);
        let img = dom
            .children
            .iter()
            .find(|c| c.tag == "img")
            .expect("img が出る");
        let src = &img.attrs.iter().find(|(k, _)| k == "src").unwrap().1;
        assert!(src.starts_with("data:image/png;base64,"), "{src}");
        assert!(img
            .attrs
            .contains(&("alt".to_string(), "サムネイル".to_string())));
    }

    /// 写真選択（Issue #1103）: `blob:` のソースだけ `img` を出し、他は出さない。
    #[test]
    fn image_widget_emits_img_only_for_blob_source() {
        use fandhe_core::screens::photo_pick::{current_screen, PhotoPickState, ShownPhoto};

        let find_img = |source: &str| {
            let state = PhotoPickState {
                shown: Some(ShownPhoto {
                    source: source.to_string(),
                    format: "PNG".into(),
                    size: 1,
                    width: 1,
                    height: 1,
                }),
                ..Default::default()
            };
            let dom = build_dom_tree(&current_screen(&state), &state, 0);
            dom.children.into_iter().find(|c| c.tag == "img")
        };

        let img = find_img("blob:https://example.test/abc").expect("blob は img");
        assert!(img.attrs.contains(&(
            "src".to_string(),
            "blob:https://example.test/abc".to_string()
        )));
        assert!(img
            .attrs
            .contains(&("alt".to_string(), "選んだ画像".to_string())));
        for bad in [
            "photo:1",
            "javascript:alert(1)",
            "https://example.test/a.png",
            "data:image/png;base64,AA",
            "",
        ] {
            assert!(find_img(bad).is_none(), "{bad} は img にしない");
        }
    }
}
