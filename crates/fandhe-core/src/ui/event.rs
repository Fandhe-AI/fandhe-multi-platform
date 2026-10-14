//! ボタンのイベントハンドリング（TASK-1.1-S3 / REQ-1）。
//!
//! `Screen<S>` からラベル一致でボタンを探索し、その `on_click` を呼び出すヘルパーを提供する。
//! テスト・ヘッドレス検証（実描画バックエンド不在）からの利用を想定しており、
//! 汎用イベントディスパッチ基盤（状態更新ループ全体の配線）は S5（#21）のスコープであり、
//! 本モジュールでは Button 単体のハンドラ呼び出しまでに留める。
//!
//! S1（#17）マージにより `Node<S>` が `Text`/`TextField`/`Button`/`List` を持つため、
//! 探索は [`super::screen::Screen::iter`] を介して `Node::Button` のみを対象にする。
//! [`click_button`] は `Node::List` / `Node::VirtualList` の展開を行わない
//! **非展開走査**であるため、動的生成子内の `Button` は解決できない
//! （Issue #487 で顕在化した制約）。この制約を解消する経路は本モジュールが
//! 新設する [`click_button_at`] を参照。
//!
//! Issue #491（REQ-1 / REQ-13）: 動的生成子（`List::expand` / `VirtualList::item`）が
//! 生成する子ノードは `Screen::iter()` に現れないため、ラベル一致の非展開走査では
//! 発見できない。[`click_button_at`] はツリー上の位置（`path: &[usize]`）を降下して
//! 対象ノードへ到達する解決経路を提供する。`path` の意味論はレンダラ側
//! （`fandhe-egui-proxy::render_node` / `render_virtual_list`）が構築する
//! `path` / `child_path` と一致させている: `path[0]` はトップレベルノードの挿入順
//! index、以降のセグメントは直前ノードが `List` なら `expand(state)` の列挙 index、
//! `VirtualList` なら絶対項目 index、`Widget`（[`super::widget::Widget`] 実装、
//! 例 `DialogWidget`）なら `Widget::children(state)` の列挙 index（Issue #739）。
//! 降下は反復ループで行い（再帰によるスタック枯渇を避ける、A05 対策・
//! security.md 準拠）、`VirtualList` は該当セグメントの
//! `item(state, index)` を 1 回だけ呼ぶ（全展開しない、仮想化を維持する）。
//! 解決した葉ノードのラベルが `label` と一致する場合のみ発火する（描画時と
//! dispatch 時の状態乖離による誤発火を防ぐ整合性ガード、A08 対策）。
//!
//! TASK-17.2（REQ-17）: `docs/architecture/tv-focus-navigation-architecture.md` §5 が
//! 定める id 版の決定操作エントリポイント [`activate_focus_at`] を追加した。
//! `path` 降下ロジックは [`resolve_node_at`] へ抽出し、[`click_button_at`]・
//! [`activate_focus_at`] の双方が共有する（重複実装を避ける、同節）。`super::focus`
//! （`FocusRing`）もフォーカス移動解決のたびに同じ降下ロジックを必要とするため、
//! `resolve_node_at` はモジュール外部（`ui` クレート内）へ `pub(super)` 公開する。

use super::node::Node;
use super::screen::Screen;
use super::widget::WidgetId;
use crate::state::WidgetValue;

/// `path` をツリー上で降下し、対象ノードを解決する共通ヘルパー（[`click_button_at`]・
/// [`activate_focus_at`]・`super::focus::FocusRing` が共有する内部実装、TASK-17.2 /
/// `docs/architecture/tv-focus-navigation-architecture.md` §5）。
///
/// 降下規則は [`click_button_at`] のドキュメンテーションコメント（モジュール冒頭）と
/// 同一。`path` が空・範囲外・非コンテナ中間ノードを指す場合は `None` を返す
/// （panic を API 契約にしない、coding-rust.md のエラーハンドリング方針）。
/// ラベル・`id` 等の整合性ガードは呼び出し元（各公開関数）の責務とし、本ヘルパーは
/// 位置解決のみに専念する。
pub(super) fn resolve_node_at<S>(screen: &Screen<S>, path: &[usize], state: &S) -> Option<Node<S>> {
    let (&first, rest) = path.split_first()?;
    let top = screen.iter().nth(first)?;
    // `Node<S>` は `S: Clone` を要求しない手動 `Clone` 実装（node.rs 参照）を持つため、
    // ここで複製して以降の降下ループを所有値のみで進められる（トップレベル 1 回だけの
    // クローンで済み、深い path でも降下のたびに複製が増えるわけではない）。
    let mut current = top.clone();

    // 反復ループで降下する（再帰によるスタック枯渇を避ける、A05 対策・security.md 準拠）。
    for &segment in rest {
        current = match current {
            Node::List { expand, .. } => expand(state).into_iter().nth(segment)?,
            Node::VirtualList { len, item, .. } => {
                if segment >= len(state) {
                    return None;
                }
                item(state, segment)
            }
            // `Node::Widget`（`DialogWidget` 等）は `Widget::children(state)` が返す
            // 状態依存の子ノード列（`DialogWidget::content` 等）を降下する（Issue #739）。
            // これによりダイアログのパネル内ボタンへ `ButtonClickedAt` の path 解決が届く。
            Node::Widget(widget) => widget.children(state).into_iter().nth(segment)?,
            // `Button` / `IndexedButton` / `Text` / `TextField` は子を持たないため、
            // path がまだ続くのに到達した場合は中間ノード不整合として `None` を返す。
            _ => return None,
        };
    }

    Some(current)
}

/// `path` で指定したツリー上の位置にある `Node::Button` / `Node::IndexedButton` の
/// `on_click` を、ラベル一致の整合性ガードを通したうえで呼び出す（Issue #491）。
///
/// `path` が空・範囲外・非コンテナ中間ノードを指す・葉ノードのラベルが `label` と
/// 不一致、のいずれかの場合は状態 `state` を変更せず `false` を返す
/// （panic を API 契約にしない、coding-rust.md のエラーハンドリング方針）。
///
/// 降下は反復ループで行い、`VirtualList` に遭遇した場合は該当セグメントの
/// `item(state, index)` のみを呼ぶ（`0..len(state)` の全展開はしない。
/// モジュール冒頭のコメント参照）。
///
/// # Examples
///
/// ```
/// use fandhe_core::{Screen, Node, WidgetId, click_button_at};
///
/// #[derive(Default)]
/// struct TodoState {
///     items: Vec<i32>,
/// }
///
/// fn increment_item(state: &mut TodoState, index: usize) {
///     if let Some(v) = state.items.get_mut(index) {
///         *v += 1;
///     }
/// }
///
/// fn expand(state: &TodoState) -> Vec<Node<TodoState>> {
///     (0..state.items.len())
///         .map(|i| Node::IndexedButton {
///             label: "+1".to_string(),
///             index: i,
///             on_click: increment_item,
///             id: WidgetId::from_raw(i as u64),
///         })
///         .collect()
/// }
///
/// let screen: Screen<TodoState> = Screen::new("todo").list(expand);
/// let mut state = TodoState { items: vec![0, 10] };
///
/// // path = [0, 1] は「トップレベル 0 番目（list）」の「展開順 1 番目（項目 index=1）」。
/// assert!(click_button_at(&screen, &[0, 1], "+1", &mut state));
/// assert_eq!(state.items, vec![0, 11]);
/// ```
pub fn click_button_at<S>(screen: &Screen<S>, path: &[usize], label: &str, state: &mut S) -> bool {
    // path 降下は共通ヘルパーへ委譲する（モジュール冒頭参照。`state: &mut S` は
    // `&S` を要求する `resolve_node_at` へ暗黙の再借用で渡せる）。
    let Some(node) = resolve_node_at(screen, path, state) else {
        return false;
    };

    // 解決した葉ノードのラベルが `label` と一致する場合のみ発火する（描画時と
    // dispatch 時の状態乖離による誤発火を防ぐ整合性ガード、A08 対策・モジュール冒頭参照）。
    match node {
        Node::Button {
            label: node_label,
            on_click,
            ..
        } if node_label == label => {
            on_click(state);
            true
        }
        Node::IndexedButton {
            label: node_label,
            index,
            on_click,
            ..
        } if node_label == label => {
            on_click(state, index);
            true
        }
        _ => false,
    }
}

/// `path` で指定したツリー上の位置にある `Node::Button` / `Node::IndexedButton` の
/// `on_click` を、`id`（`WidgetId`）一致の整合性ガードを通したうえで呼び出す
/// （TASK-17.2 / REQ-17、`docs/architecture/tv-focus-navigation-architecture.md` §5）。
///
/// [`click_button_at`]（ラベル版、既存 API・非破壊で温存）と並置する id 版であり、
/// `path` 降下ロジックは共通の内部ヘルパー [`resolve_node_at`] を共有する
/// （重複実装を避ける、Codex レビュー 3 巡目 P1 指摘対応）。
///
/// プラットフォームレイヤ（Android TV）が決定ボタン（`KEYCODE_DPAD_CENTER` 等）を
/// 受信した際、`super::focus::FocusRing::current`（`FocusEntry { path, id }`）を
/// そのまま本関数へ渡して呼び出す想定（設計文書 §5 のシーケンス図参照）。
///
/// 解決した葉ノードの `id` が引数 `id` と一致する場合のみ `on_click` を発火する。
/// 不一致・path 範囲外・非葉ノードはいずれも状態を変更せず `false` を返す
/// （`click_button_at` と同じエラーハンドリング方針、coding-rust.md）。この `id`
/// ガードは「フォーカス中とみなしていた項目が決定操作の瞬間には別の論理項目に
/// すり替わっていないか」を最終確認する役割を持つ（設計文書 §4.4「id による二層検証」）。
pub fn activate_focus_at<S>(
    screen: &Screen<S>,
    path: &[usize],
    id: WidgetId,
    state: &mut S,
) -> bool {
    let Some(node) = resolve_node_at(screen, path, state) else {
        return false;
    };

    match node {
        Node::Button {
            on_click,
            id: node_id,
            ..
        } if node_id == id => {
            on_click(state);
            true
        }
        Node::IndexedButton {
            index,
            on_click,
            id: node_id,
            ..
        } if node_id == id => {
            on_click(state, index);
            true
        }
        _ => false,
    }
}

/// `path` で指定したツリー上の位置にある `Node::Widget`（`Checkbox` / `ToggleSwitch` /
/// `Slider` / `RadioButton`）へ値変更を反映する（Issue #805 / 親 #804）。
///
/// `click_button_at` / `activate_focus_at` と同じく `path` 降下は共通ヘルパー
/// [`resolve_node_at`] へ委譲する。[`activate_focus_at`]（`ButtonActivatedAt` の id
/// ガード）と対称に、解決した `Node::Widget` の [`super::widget::Widget::id`] が
/// 引数 `id` と一致する場合のみ [`super::widget::Widget::apply_value`] を呼ぶ
/// （id ガードは本関数が一元的に持ち、`apply_value` 側には持たせない）。`path` が
/// 空・範囲外・非コンテナ中間ノードを指す・解決した葉ノードが `Node::Widget` でない・
/// `id` 不一致・ウィジェットが無効化中（`interaction_state().disabled`）・`apply_value` が
/// 型不一致や値の不正（`Slider` の `NaN` 等）を検出した、
/// のいずれかの場合は状態 `state` を変更せず `false` を返す（panic を API 契約にしない、
/// coding-rust.md のエラーハンドリング方針）。
///
/// `RadioButton` は `id` ガード通過後、`apply_value` 内部でさらに `WidgetValue::Index`
/// と自身の `value()` の一致も見るため、id → index の二段ガードになる
/// （グループ内の他の選択肢の `RadioButton` が誤って自分の値を書かないための追加防御）。
///
/// `path` の解決は描画時と `dispatch` 時の状態乖離（動的生成子の展開結果が変わる等）が
/// あり得るため、誤反映の最終防御は本関数の `id` ガードと各ウィジェットの `apply_value`
/// 実装が持つ型・値のガードの両方で担う（`click_button_at` のラベルガードと同じ
/// 整合性検証の思想、A08 対策）。
pub fn set_widget_value_at<S>(
    screen: &Screen<S>,
    path: &[usize],
    id: WidgetId,
    value: &WidgetValue,
    state: &mut S,
) -> bool {
    let Some(node) = resolve_node_at(screen, path, state) else {
        return false;
    };

    match node {
        // 無効化中のウィジェットは値変更を受け付けない（各ウィジェットの `apply_input` が
        // disabled 中は入力を無視する既存契約と揃える。描画後に無効化された部品へ遅延
        // イベントが届いた場合の誤反映も防ぐ、PR #808 codex-review P1 対応）。
        Node::Widget(widget) if widget.id() == id && !widget.interaction_state().disabled => {
            widget.apply_value(state, value)
        }
        _ => false,
    }
}

/// ボタンラベルに一致する `Node::Button` / `Node::IndexedButton` の `on_click` を呼び出す。
///
/// 見つからない場合は状態 `state` を変更せず `false` を返す（panic を API 契約にしない、
/// coding-rust.md のエラーハンドリング方針）。同一ラベルのボタンが複数存在する場合は
/// 先頭の 1 件のみ発火する（PoC-6 準拠の挙動）。
///
/// `Node::IndexedButton` はトップレベルに直接配置された場合のみ本関数で解決できる
/// （`Screen::iter()` の非展開走査対象であるため）。動的生成子
/// （`List::expand` / `VirtualList::item`）が生成した子ノードの解決は
/// [`click_button_at`] を使うこと（Issue #491、モジュール冒頭のコメント参照）。
///
/// # Examples
///
/// ```
/// use fandhe_core::{click_button, Screen, WidgetId};
///
/// #[derive(Default)]
/// struct Counter {
///     count: u32,
/// }
///
/// fn increment(state: &mut Counter) {
///     state.count += 1;
/// }
///
/// let screen: Screen<Counter> =
///     Screen::new("home").button(WidgetId::from_raw(1), "increment", increment);
/// let mut state = Counter::default();
///
/// assert!(click_button(&screen, "increment", &mut state));
/// assert_eq!(state.count, 1);
/// ```
pub fn click_button<S>(screen: &Screen<S>, label: &str, state: &mut S) -> bool {
    // `Node<S>` は複数バリアントを持つため、非 Button/IndexedButton ノードを
    // スキップして探索を継続できるよう `match` + `continue` にしている。`List` /
    // `VirtualList` はここでは展開せず（線形走査のため）、描画走査等で子要素まで
    // 辿る経路は `Screen::visit_expanded` を、動的生成子内のハンドラ解決は
    // `click_button_at` を利用する想定。
    for node in screen.iter() {
        match node {
            Node::Button {
                label: button_label,
                on_click,
                ..
            } if button_label == label => {
                on_click(state);
                return true;
            }
            Node::IndexedButton {
                label: button_label,
                index,
                on_click,
                ..
            } if button_label == label => {
                on_click(state, *index);
                return true;
            }
            _ => continue,
        }
    }
    false
}

/// `screen` の [`Screen::back_handler`]（`.on_back()` で設定したハンドラ）を発火する
/// （Issue #726、`click_button` と同型の窓口）。
///
/// プラットフォームレイヤが OS の戻る操作（macOS/Linux/Windows の Esc、Android の
/// バックキー）を検知し、`crate::state::Event::BackRequested` 経由で呼ばれる想定
/// （`state/store.rs` のモジュール doc 参照）。`on_back` 未設定の画面（既定）では
/// 何もせず `false` を返す（panic を API 契約にしない、coding-rust.md 準拠）。
pub fn request_back<S>(screen: &Screen<S>, state: &mut S) -> bool {
    match screen.back_handler() {
        Some(handler) => {
            handler(state);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Screen;
    use crate::WidgetId;

    /// テスト用の最小状態: ボタン押下回数を数えるカウンタ。
    ///
    /// `agreed` / `level` / `choice` は `set_widget_value_at`（Issue #805）の
    /// テスト専用フィールドで、それぞれ `Checkbox` / `ToggleSwitch`、`Slider`、
    /// `RadioButton` の書き込み先として使う。
    #[derive(Default)]
    struct Counter {
        count: u32,
        agreed: bool,
        level: f32,
        choice: usize,
    }

    fn increment(state: &mut Counter) {
        state.count += 1;
    }

    fn decrement(state: &mut Counter) {
        state.count = state.count.saturating_sub(1);
    }

    /// ラベル一致ボタンの `on_click` が呼ばれ、状態が期待どおり更新されることを確認する。
    #[test]
    fn click_button_invokes_handler() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(1), "increment", increment);
        let mut state = Counter::default();

        let handled = click_button(&screen, "increment", &mut state);

        assert!(handled);
        assert_eq!(state.count, 1);
    }

    /// 存在しないラベル指定時に `false` が返り、状態が変化しないことを確認する。
    #[test]
    fn click_button_returns_false_when_not_found() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(1), "increment", increment);
        let mut state = Counter::default();

        let handled = click_button(&screen, "no-such-button", &mut state);

        assert!(!handled);
        assert_eq!(state.count, 0);
    }

    /// 複数ボタンが存在する場合、指定ラベルのハンドラのみ発火することを確認する。
    #[test]
    fn click_button_fires_only_matching_label() {
        let screen: Screen<Counter> = Screen::new("test")
            .button(WidgetId::from_raw(1), "increment", increment)
            .button(WidgetId::from_raw(2), "decrement", decrement);
        let mut state = Counter {
            count: 5,
            ..Default::default()
        };

        let handled = click_button(&screen, "decrement", &mut state);

        assert!(handled);
        assert_eq!(state.count, 4);
    }

    /// 同一ラベルのボタンが複数存在する場合、先頭の 1 件のみ発火することを確認する
    /// （本モジュールのドキュメンテーションコメントに明記された PoC-6 準拠の挙動）。
    ///
    /// `increment` と `decrement` という異なる効果のハンドラを同一ラベルに割り当て、
    /// 先頭（increment）のみが呼ばれるなら `count == 1`、2 件目（decrement）のみなら
    /// `count == 0`、両方呼ばれるなら `count == 0` となり、3 パターンを判別できる。
    #[test]
    fn click_button_fires_only_first_when_labels_duplicate() {
        let screen: Screen<Counter> = Screen::new("test")
            .button(WidgetId::from_raw(1), "action", increment)
            .button(WidgetId::from_raw(2), "action", decrement);
        let mut state = Counter::default();

        let handled = click_button(&screen, "action", &mut state);

        assert!(handled);
        assert_eq!(state.count, 1);
    }

    // ------------------------------------------------------------------
    // `click_button_at`（Issue #491）のテスト。
    // ------------------------------------------------------------------

    /// 動的生成子（`List` / `VirtualList`）を持つテスト用状態。項目ごとに値を持ち、
    /// `IndexedButton::on_click` が対象 index の値だけを変更できることを検証する。
    #[derive(Default)]
    struct ItemsState {
        values: Vec<i32>,
    }

    fn increment_item(state: &mut ItemsState, index: usize) {
        if let Some(v) = state.values.get_mut(index) {
            *v += 1;
        }
    }

    /// `List::expand` 版: 項目ごとに `IndexedButton` を生成する。
    fn expand_indexed_buttons(state: &ItemsState) -> Vec<Node<ItemsState>> {
        (0..state.values.len())
            .map(|i| Node::IndexedButton {
                label: "+1".to_string(),
                index: i,
                on_click: increment_item,
                id: WidgetId::from_raw(i as u64),
            })
            .collect()
    }

    /// `List` の子（動的生成された `IndexedButton`）を path 解決で発火でき、
    /// 対象 index の値のみが変わることを検証する（受け入れ条件 (1)）。
    #[test]
    fn click_button_at_resolves_indexed_button_inside_list() {
        let screen: Screen<ItemsState> = Screen::new("test").list(expand_indexed_buttons);
        let mut state = ItemsState {
            values: vec![10, 20, 30],
        };

        // path = [0, 1]: トップレベル 0 番目（list）→ 展開順 1 番目（index=1 の項目）。
        let handled = click_button_at(&screen, &[0, 1], "+1", &mut state);

        assert!(handled);
        assert_eq!(state.values, vec![10, 21, 30]);
    }

    /// `VirtualList::item` の呼び出し回数を数える計測用状態。
    #[derive(Default)]
    struct CountingVirtualListState {
        values: Vec<i32>,
        item_call_count: std::sync::atomic::AtomicUsize,
    }

    fn counting_len(state: &CountingVirtualListState) -> usize {
        state.values.len()
    }

    fn counting_item(
        state: &CountingVirtualListState,
        index: usize,
    ) -> Node<CountingVirtualListState> {
        state
            .item_call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Node::IndexedButton {
            label: "+1".to_string(),
            index,
            on_click: increment_counting_item,
            id: WidgetId::from_raw(index as u64),
        }
    }

    fn increment_counting_item(state: &mut CountingVirtualListState, index: usize) {
        if let Some(v) = state.values.get_mut(index) {
            *v += 1;
        }
    }

    /// `VirtualList` 項目の `IndexedButton` が正しい index でハンドラを受領し、かつ
    /// `item` が対象 index の解決のためにちょうど 1 回しか呼ばれない（仮想化を
    /// 維持する、`0..len(state)` の全展開をしない）ことを検証する（受け入れ条件 (2)(3)）。
    #[test]
    fn click_button_at_resolves_virtual_list_item_calling_item_exactly_once() {
        let screen: Screen<CountingVirtualListState> =
            Screen::new("test").virtual_list(counting_len, counting_item, 40.0);
        let mut state = CountingVirtualListState {
            values: vec![0, 0, 0, 0, 0],
            item_call_count: std::sync::atomic::AtomicUsize::new(0),
        };

        // path = [0, 3]: トップレベル 0 番目（virtual_list）→ 絶対 index 3 の項目。
        let handled = click_button_at(&screen, &[0, 3], "+1", &mut state);

        assert!(handled);
        assert_eq!(state.values, vec![0, 0, 0, 1, 0]);
        assert_eq!(
            state
                .item_call_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "item(state, index) は対象 index の解決のためちょうど 1 回だけ呼ばれるはず\
             （全展開しない、仮想化を維持する）"
        );
    }

    /// path がツリーの範囲外を指す場合は `false` を返し、状態が変化しないことを検証する
    /// （受け入れ条件 (4)）。
    #[test]
    fn click_button_at_returns_false_when_path_out_of_range() {
        let screen: Screen<ItemsState> = Screen::new("test").list(expand_indexed_buttons);
        let mut state = ItemsState {
            values: vec![10, 20],
        };

        // トップレベル index 5 は存在しない。
        let handled = click_button_at(&screen, &[5], "+1", &mut state);

        assert!(!handled);
        assert_eq!(state.values, vec![10, 20]);
    }

    /// path が非コンテナ中間ノード（`Text` 等、子を持たない）を経由しようとする場合は
    /// `false` を返し、状態が変化しないことを検証する（受け入れ条件 (4)）。
    #[test]
    fn click_button_at_returns_false_when_intermediate_node_is_not_a_container() {
        let screen: Screen<ItemsState> = Screen::new("test").text("見出し");
        let mut state = ItemsState {
            values: vec![10, 20],
        };

        // path 長 2 だが、トップレベル 0 番目は `Text`（子を持たない）。
        let handled = click_button_at(&screen, &[0, 0], "+1", &mut state);

        assert!(!handled);
        assert_eq!(state.values, vec![10, 20]);
    }

    /// path で解決した葉ノードのラベルが指定 `label` と不一致の場合は `false` を返し、
    /// 状態が変化しないことを検証する（受け入れ条件 (5)、整合性ガード）。
    #[test]
    fn click_button_at_returns_false_when_leaf_label_does_not_match() {
        let screen: Screen<ItemsState> = Screen::new("test").list(expand_indexed_buttons);
        let mut state = ItemsState {
            values: vec![10, 20],
        };

        let handled = click_button_at(&screen, &[0, 0], "存在しないラベル", &mut state);

        assert!(!handled);
        assert_eq!(state.values, vec![10, 20]);
    }

    /// `VirtualList` 項目内に `List` をネストした構成でも path 解決できることを
    /// 検証する（受け入れ条件 (6)）。
    #[test]
    fn click_button_at_resolves_nested_list_inside_virtual_list_item() {
        fn nested_len(state: &ItemsState) -> usize {
            state.values.len()
        }
        fn nested_item(_state: &ItemsState, index: usize) -> Node<ItemsState> {
            fn inner_expand(state: &ItemsState) -> Vec<Node<ItemsState>> {
                (0..state.values.len())
                    .map(|i| Node::IndexedButton {
                        label: "+1".to_string(),
                        index: i,
                        on_click: increment_item,
                        id: WidgetId::from_raw(i as u64),
                    })
                    .collect()
            }
            // 各項目は「その項目専用の `List`」を 1 つだけ子に持つ最小構成。
            let _ = index;
            Node::List {
                expand: inner_expand,
                axis: crate::Axis::default(),
            }
        }

        let screen: Screen<ItemsState> =
            Screen::new("test").virtual_list(nested_len, nested_item, 40.0);
        let mut state = ItemsState {
            values: vec![10, 20, 30],
        };

        // path = [0, 1, 2]: トップレベル 0 番目（virtual_list）→ 絶対 index 1 の項目
        // （nested_item が返す `List`）→ その展開順 2 番目（index=2 の `IndexedButton`）。
        let handled = click_button_at(&screen, &[0, 1, 2], "+1", &mut state);

        assert!(handled);
        assert_eq!(state.values, vec![10, 20, 31]);
    }

    /// `List` の子として `Node::Button`（`IndexedButton` ではない既存バリアント）を
    /// 生成する構成でも path 解決できることを検証する（`click_button_at` が
    /// `Button` / `IndexedButton` の両方を解決対象とすることの確認）。
    #[test]
    fn click_button_at_resolves_plain_button_inside_list() {
        fn expand_plain_button(_state: &Counter) -> Vec<Node<Counter>> {
            vec![Node::Button {
                label: "increment".to_string(),
                on_click: increment,
                id: WidgetId::from_raw(1),
            }]
        }

        let screen: Screen<Counter> = Screen::new("test").list(expand_plain_button);
        let mut state = Counter::default();

        let handled = click_button_at(&screen, &[0, 0], "increment", &mut state);

        assert!(handled);
        assert_eq!(state.count, 1);
    }

    // ------------------------------------------------------------------
    // `activate_focus_at`（TASK-17.2 / REQ-17）のテスト。
    // ------------------------------------------------------------------

    /// `id` が一致する場合、`Node::Button` の `on_click` が発火することを検証する
    /// （設計文書 §5 の正常系）。
    #[test]
    fn activate_focus_at_fires_button_on_id_match() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(7), "increment", increment);
        let mut state = Counter::default();

        let handled = activate_focus_at(&screen, &[0], WidgetId::from_raw(7), &mut state);

        assert!(handled);
        assert_eq!(state.count, 1);
    }

    /// `id` が一致する場合、動的生成子（`List`）内の `Node::IndexedButton` の
    /// `on_click` が発火することを検証する。
    #[test]
    fn activate_focus_at_fires_indexed_button_on_id_match() {
        let screen: Screen<ItemsState> = Screen::new("test").list(expand_indexed_buttons);
        let mut state = ItemsState {
            values: vec![10, 20, 30],
        };

        // path = [0, 1]: トップレベル 0 番目（list）→ 展開順 1 番目（index=1、id=1）。
        let handled = activate_focus_at(&screen, &[0, 1], WidgetId::from_raw(1), &mut state);

        assert!(handled);
        assert_eq!(state.values, vec![10, 21, 30]);
    }

    /// 解決した葉ノードの `id` が引数 `id` と不一致の場合は `on_click` を呼ばず
    /// `false` を返すことを検証する（設計文書 §5「id による二層検証」の決定時ガード）。
    #[test]
    fn activate_focus_at_rejects_on_id_mismatch() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(1), "increment", increment);
        let mut state = Counter::default();

        // 実際のノードの id は 1 だが、別の id を渡す（フォーカス中とみなしていた
        // 項目が決定操作の瞬間に別の論理項目にすり替わっていたケースを模す）。
        let handled = activate_focus_at(&screen, &[0], WidgetId::from_raw(999), &mut state);

        assert!(!handled);
        assert_eq!(state.count, 0);
    }

    /// path が範囲外を指す場合は `false` を返し、状態が変化しないことを検証する。
    #[test]
    fn activate_focus_at_returns_false_when_path_out_of_range() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(1), "increment", increment);
        let mut state = Counter::default();

        let handled = activate_focus_at(&screen, &[5], WidgetId::from_raw(1), &mut state);

        assert!(!handled);
        assert_eq!(state.count, 0);
    }

    /// `.on_back()` で設定したハンドラが `request_back` から発火することを検証する
    /// （Issue #726）。
    #[test]
    fn request_back_invokes_handler_when_set() {
        let screen: Screen<Counter> = Screen::new("test").on_back(decrement);
        let mut state = Counter {
            count: 1,
            ..Default::default()
        };

        let handled = request_back(&screen, &mut state);

        assert!(handled);
        assert_eq!(state.count, 0);
    }

    /// `.on_back()` 未設定の画面では `false` を返し、状態が変化しないことを検証する。
    #[test]
    fn request_back_returns_false_when_unset() {
        let screen: Screen<Counter> = Screen::new("test");
        let mut state = Counter::default();

        let handled = request_back(&screen, &mut state);

        assert!(!handled);
        assert_eq!(state.count, 0);
    }

    // ------------------------------------------------------------------
    // `Node::Widget` の子降下（Issue #739）。
    // ------------------------------------------------------------------

    fn is_dialog_open(state: &Counter) -> bool {
        state.count > 0
    }

    fn set_dialog_open(state: &mut Counter, open: bool) {
        if !open {
            state.count = 0;
        }
    }

    fn expand_dialog_content(_state: &Counter) -> Vec<Node<Counter>> {
        vec![Node::Button {
            label: "increment".to_string(),
            on_click: increment,
            id: WidgetId::from_raw(2),
        }]
    }

    /// `resolve_node_at` が `Node::Widget`（`DialogWidget`）経由でパネル内の
    /// `Button` へ降下し、`click_button_at` が発火できることを検証する（Issue #739）。
    #[test]
    fn click_button_at_resolves_button_inside_dialog_widget_children() {
        use crate::DialogWidget;

        let screen: Screen<Counter> = Screen::new("test").with_node(Node::Widget(Box::new(
            DialogWidget::new_dynamic(
                WidgetId::from_raw(1),
                "確認",
                "本文",
                "キャンセル",
                true,
                is_dialog_open,
                set_dialog_open,
            )
            .with_content(expand_dialog_content),
        )));
        let mut state = Counter {
            count: 1,
            ..Default::default()
        };

        // path = [0, 0]: トップレベル 0 番目（Widget）→ children(state) の 0 番目。
        let handled = click_button_at(&screen, &[0, 0], "increment", &mut state);

        assert!(handled);
        assert_eq!(state.count, 2);
    }

    /// PR #792 codex-review 指摘 A: `set_disabled(true)` の開状態 `DialogWidget` は
    /// `children` が空を返すため、`resolve_node_at` が子へ降下できず
    /// `click_button_at` は発火しない（disabled 中はすべての入力を無視する契約を
    /// 子ハンドラにも及ぼす）。
    #[test]
    fn click_button_at_returns_false_for_disabled_dialog_widget_children() {
        use crate::DialogWidget;

        let mut dialog = DialogWidget::new_dynamic(
            WidgetId::from_raw(1),
            "確認",
            "本文",
            "キャンセル",
            true,
            is_dialog_open,
            set_dialog_open,
        )
        .with_content(expand_dialog_content);
        dialog.set_disabled(true);
        let screen: Screen<Counter> = Screen::new("test").with_node(Node::Widget(Box::new(dialog)));
        let mut state = Counter {
            count: 1,
            ..Default::default()
        };

        let handled = click_button_at(&screen, &[0, 0], "increment", &mut state);

        assert!(!handled);
        assert_eq!(state.count, 1);
    }

    // --- set_widget_value_at（Issue #805） ---

    fn get_agreed(state: &Counter) -> bool {
        state.agreed
    }

    fn set_agreed(state: &mut Counter, value: bool) {
        state.agreed = value;
    }

    fn get_level(state: &Counter) -> f32 {
        state.level
    }

    fn set_level(state: &mut Counter, value: f32) {
        state.level = value;
    }

    fn get_choice(state: &Counter) -> usize {
        state.choice
    }

    fn set_choice(state: &mut Counter, value: usize) {
        state.choice = value;
    }

    /// `Checkbox` に `Bool` を渡すと `set_agreed` が発火することを確認する。
    #[test]
    fn set_widget_value_at_checkbox_sets_bool() {
        let screen: Screen<Counter> =
            Screen::new("test").checkbox(WidgetId::from_raw(1), "agree", get_agreed, set_agreed);
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Bool(true),
            &mut state,
        );

        assert!(handled);
        assert!(state.agreed);
    }

    /// `Checkbox` に `Number`（型不一致）を渡すと `false` を返し、状態を変更しないことを
    /// 確認する。
    #[test]
    fn set_widget_value_at_checkbox_rejects_type_mismatch() {
        let screen: Screen<Counter> =
            Screen::new("test").checkbox(WidgetId::from_raw(1), "agree", get_agreed, set_agreed);
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Number(1.0),
            &mut state,
        );

        assert!(!handled);
        assert!(!state.agreed);
    }

    /// `ToggleSwitch` に `Bool` を渡すと `set_agreed` が発火することを確認する。
    #[test]
    fn set_widget_value_at_toggle_switch_sets_bool() {
        let screen: Screen<Counter> = Screen::new("test").toggle_switch(
            WidgetId::from_raw(1),
            "agree",
            get_agreed,
            set_agreed,
        );
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Bool(true),
            &mut state,
        );

        assert!(handled);
        assert!(state.agreed);
    }

    /// `Slider` に範囲内の `Number` を渡すと `set_level` が発火することを確認する。
    #[test]
    fn set_widget_value_at_slider_sets_number() {
        let screen: Screen<Counter> = Screen::new("test").slider(
            WidgetId::from_raw(1),
            "level",
            0.0,
            10.0,
            get_level,
            set_level,
        );
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Number(4.0),
            &mut state,
        );

        assert!(handled);
        assert_eq!(state.level, 4.0);
    }

    /// `Slider` に `NaN` を渡すと `false` を返し、状態を変更しないことを確認する
    /// （信頼境界の入力検証、A03 対策）。
    #[test]
    fn set_widget_value_at_slider_rejects_nan() {
        let screen: Screen<Counter> = Screen::new("test").slider(
            WidgetId::from_raw(1),
            "level",
            0.0,
            10.0,
            get_level,
            set_level,
        );
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Number(f32::NAN),
            &mut state,
        );

        assert!(!handled);
        assert_eq!(state.level, 0.0);
    }

    /// `Slider` に範囲外の `Number` を渡すと `[min, max]` へクランプして反映することを
    /// 確認する。
    #[test]
    fn set_widget_value_at_slider_clamps_out_of_range() {
        let screen: Screen<Counter> = Screen::new("test").slider(
            WidgetId::from_raw(1),
            "level",
            0.0,
            10.0,
            get_level,
            set_level,
        );
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Number(99.0),
            &mut state,
        );

        assert!(handled);
        assert_eq!(state.level, 10.0);
    }

    /// `RadioButton` に自身の `value` と一致する `Index` を渡すと `set_choice` が
    /// 発火することを確認する。
    #[test]
    fn set_widget_value_at_radio_button_sets_index_on_match() {
        let screen: Screen<Counter> = Screen::new("test").radio_button(
            WidgetId::from_raw(1),
            "option-a",
            0,
            get_choice,
            set_choice,
        );
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Index(0),
            &mut state,
        );

        assert!(handled);
        assert_eq!(state.choice, 0);
    }

    /// `RadioButton` に自身の `value` と不一致な `Index` を渡すと `false` を返し、
    /// 状態を変更しないことを確認する（描画時と dispatch 時の状態乖離による誤発火防止、
    /// A08 対策）。
    #[test]
    fn set_widget_value_at_radio_button_rejects_index_mismatch() {
        let screen: Screen<Counter> = Screen::new("test").radio_button(
            WidgetId::from_raw(1),
            "option-a",
            0,
            get_choice,
            set_choice,
        );
        let mut state = Counter {
            choice: 1,
            ..Default::default()
        };

        // このラジオボタンの value() は 0 のため、Index(1) は不一致（このラジオボタン
        // ではなく別の選択肢が選ばれたことを意味し、このハンドラの担当外）。
        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Index(1),
            &mut state,
        );

        assert!(!handled);
        assert_eq!(state.choice, 1);
    }

    /// path が `Node::Text` を指す場合は `false` を返すことを確認する（`Node::Widget`
    /// 以外への値変更は対象外）。
    #[test]
    fn set_widget_value_at_returns_false_for_non_widget_node() {
        let screen: Screen<Counter> =
            Screen::new("test").with_node(Node::Text("hello".to_string()));
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Bool(true),
            &mut state,
        );

        assert!(!handled);
    }

    /// path が範囲外の場合は `false` を返すことを確認する。
    #[test]
    fn set_widget_value_at_returns_false_for_out_of_range_path() {
        let screen: Screen<Counter> =
            Screen::new("test").checkbox(WidgetId::from_raw(1), "agree", get_agreed, set_agreed);
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[5],
            WidgetId::from_raw(1),
            &WidgetValue::Bool(true),
            &mut state,
        );

        assert!(!handled);
    }

    /// id 不一致の場合は `false` を返し、状態を変更しないことを確認する
    /// （`activate_focus_at` と対称の id ガード、レビュー指摘対応）。
    #[test]
    fn set_widget_value_at_returns_false_when_id_mismatch() {
        let screen: Screen<Counter> =
            Screen::new("test").checkbox(WidgetId::from_raw(1), "agree", get_agreed, set_agreed);
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(999),
            &WidgetValue::Bool(true),
            &mut state,
        );

        assert!(!handled);
        assert!(!state.agreed);
    }

    /// 無効化中のウィジェットには値変更を適用しない（PR #808 codex-review P1）。
    #[test]
    fn set_widget_value_at_returns_false_when_widget_disabled() {
        let mut toggle = super::super::toggle_switch::ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "agree",
            get_agreed,
            set_agreed,
        );
        toggle.set_disabled(true);
        let screen: Screen<Counter> = Screen::new("test").with_node(Node::Widget(Box::new(toggle)));
        let mut state = Counter::default();

        let handled = set_widget_value_at(
            &screen,
            &[0],
            WidgetId::from_raw(1),
            &WidgetValue::Bool(true),
            &mut state,
        );

        assert!(!handled);
        assert!(!state.agreed);
    }
}
