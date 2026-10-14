//! 状態管理とイベント→状態更新の配線（TASK-1.1-S5 / REQ-1）。
//!
//! `ui::click_button` は「単発のボタン押下ヘルパー」に留まっていたが、本モジュールは
//! それを一般化し、プラットフォーム統合レイヤ（未実装・platform-builder 担当）が
//! ネイティブイベントを [`Event`] へ変換して渡す窓口（[`StateStore::dispatch`]）を提供する
//! （共通コア ⇔ 最適化レイヤの境界、REQ-2）。
//!
//! 状態 `S` への書き込み経路は `dispatch` 内部で呼ばれる `ui::Node<S>` 定義済みの
//! 関数ポインタ（`fn(&mut S)` / `fn(&mut S, String)` 等）に限定し、`&mut S` を外部へ
//! 直接公開しない（クロージャキャプチャや任意コード実行を型レベルで遮断する
//! AI 型安全 DSL の方針、REQ-13 系列・PoC-6 踏襲）。
//!
//! ノード走査ロジックの単一ソースは `ui::event` が持つ（`Event::ButtonClicked` は
//! 公開 API `ui::click_button` へ委譲する）。`state` モジュールは `ui` モジュールが
//! 提供する公開 API のみに依存し、走査ロジックそのものを再実装しない。
//!
//! `state::read` / `state::write` は型定義（`ReadFn` / `WriteFn` 等）のみを提供し、
//! `ui` へは依存しない（`ui::node` → `state` の一方向）。対して本モジュール
//! （`state::store`）は逆に `ui` の公開 API（`Screen` / `Node` / `click_button`）を
//! 呼ぶ側であり、モジュール単位では相互参照になる。クレート境界（他クレートとの
//! 依存方向）は coding-rust.md の規約どおり一方向を維持しており、本モジュールが
//! 生む相互参照は同一クレート内部のモジュール間に閉じている。
//!
//! `Event::TextFieldChanged` は `Node::TextField::set`（`state::WriteFn`）へ委譲する
//! （`crates/fandhe-core/src/state/mod.rs` のモジュールドキュメント参照）。`Node::List`
//! はユーザー操作で発火するイベントを持たない（状態からの一方向な子要素展開のみ）ため、
//! 対応する `Event` バリアントは追加しない。
//!
//! Issue #491（REQ-1 / REQ-13）: `Event::ButtonClicked`（`ui::click_button` の非展開
//! 走査）は `List::expand` / `VirtualList::item` が動的生成する子の `Button` を
//! 解決できない。[`Event::ButtonClickedAt`] はツリー上の位置（`path`）を伴う新
//! バリアントとして追加し、`ui::click_button_at` へ委譲する（既存
//! `Event::ButtonClicked` / `TextFieldChanged` は変更しない、非破壊な追加）。
//! `TextField` の動的生成子内解決（`TextFieldChangedAt` 相当）は未対応
//! （out-of-scope-tracking.md 対象）。
//!
//! Issue #745（REQ-13 / NFR-9）: [`StateStore::apply`] は `dispatch` とは独立した
//! 別経路。`dispatch` は「`Event` の固定バリアント → `Node<S>` の関数ポインタ」に
//! 限定することで、AI が生成する `Node` ハンドラが環境キャプチャするクロージャを
//! 書けないことを型で担保する（本モジュール冒頭の説明どおり）。一方、別スレッドで
//! 完了する外部 I/O（保存・HTTP・カメラ等、Phase 8）の結果は完了時点でしか値が
//! 定まらず、クロージャによるキャプチャが本質的に必要になる。これを `Event` の
//! 新バリアントとして表現しようとすると、`Event` が `#[derive(PartialEq, Eq)]` の
//! 単一列挙型でありバリアントごとに異なる完了ペイロード型を持てないことと衝突し、
//! 「`dispatch` は同期・関数ポインタ限定」という既存契約を壊す。そのため `apply` を
//! `Event`/`dispatch` とは別の公開 API として追加し、既存契約を変更しないまま
//! 「外部 I/O 完了の反映」というユースケースに対応する（呼び出し元は
//! `fandhe-egui-proxy::drain_completions` を想定。`ui::Node` ハンドラ（AI 生成コード）
//! からは呼べない設計で、`StateStore` を握るのはプラットフォーム統合レイヤのみ）。

use crate::ui::{
    Node, Screen, WidgetId, activate_focus_at, click_button, click_button_at, request_back,
    set_widget_value_at,
};

/// `Event::WidgetValueChanged` が運ぶ値のペイロード（Issue #805 / 親 #804）。
///
/// `Checkbox` / `ToggleSwitch` / `Slider` / `RadioButton` は bool・f32・usize と
/// 運ぶ値の型がそれぞれ異なるが、`Event` は単一列挙型で `#[derive(PartialEq)]` を
/// 保つ必要があり（`ButtonActivatedAt` 等と同じ制約、`StateStore::apply` の
/// モジュール冒頭コメント §Issue #745 参照）、バリアントごとに異なるペイロード型を
/// 持たせられない。そのため本型で値を一つの列挙にまとめ、`Widget::apply_value` 側で
/// 型不一致（例: `Checkbox` に `Number` を渡す）を判定してガードする。
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetValue {
    /// `Checkbox` / `ToggleSwitch` が運ぶ真偽値。
    Bool(bool),
    /// `Slider` が運ぶ数値（`NaN`・範囲外は `apply_value` 側でガードする）。
    Number(f32),
    /// `RadioButton` が運ぶ選択肢インデックス。
    Index(usize),
}

/// プラットフォーム非依存の UI イベント表現。
///
/// プラットフォーム統合レイヤ（未実装）がネイティブイベント（タップ・クリック・
/// テキスト入力等）を本型へ変換し、`StateStore::dispatch` に渡す契約になる想定（REQ-2）。
/// 実装時点の `Node<S>` は `Button` / `TextField` が状態更新の起点となるため、
/// この 2 バリアントを定義する（`List` は子要素展開のみで発火イベントを持たない）。
// `WidgetValueChanged` の `WidgetValue::Number(f32)` が `Eq` を持たないため、
// `Event` 全体の派生から `Eq` を落とす（`f32` の非全順序に伴う既知の制約。
// 既存の等価比較呼び出しは `PartialEq` のみで足りるため挙動には影響しない）。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// ラベルが一致する `Node::Button` の押下（`on_click` を発火させる）。
    ButtonClicked {
        /// 発火対象を特定するボタンラベル（`Screen::button` の `label` と一致させる）。
        label: String,
    },
    /// ラベルが一致する `Node::TextField` への入力確定（`set` を発火させる）。
    TextFieldChanged {
        /// 発火対象を特定するテキスト入力欄のラベル（`Node::TextField::label` と一致させる）。
        label: String,
        /// `set` へ渡す新しい入力値。
        value: String,
    },
    /// ツリー上の位置（`path`）で特定した `Node::Button` / `Node::IndexedButton` の押下
    /// （Issue #491）。動的生成子（`List::expand` / `VirtualList::item`）内のボタンは
    /// ラベルだけでは一意に解決できないため、`path` で位置を特定する。
    ButtonClickedAt {
        /// 解決した葉ノードのラベルと一致することを要求する整合性ガード
        /// （`ui::click_button_at` の doc 参照。描画時と dispatch 時の状態乖離による
        /// 誤発火を防ぐ、A08 対策）。
        label: String,
        /// 画面ルートからのノード位置列（`ui::click_button_at` の doc 参照）。
        path: Vec<usize>,
    },
    /// TV フォーカスの決定操作（Issue #536）。`path` で解決した葉ノードの `WidgetId` が
    /// 一致した場合のみ `on_click` を発火する `ui::event::activate_focus_at` への窓口で、
    /// `ButtonClickedAt`（label + path ガード、#491）と対称の id 版。誤発火防止ガードを
    /// label ではなく `WidgetId` で行う点のみが異なる（描画時に解決した `FocusRing` の
    /// `id` をそのまま渡す想定、TASK-17.2）。TV のフォーカス移動・決定操作という
    /// プラットフォーム固有の入力を、状態 `S` への書き込みを `Event` 経由の `dispatch` に
    /// 限定する StateStore のカプセル化（モジュール冒頭のコメント参照）へ載せるための経路。
    ButtonActivatedAt {
        /// 発火対象を特定する `WidgetId`（`Screen::button` 等で付与した id と一致させる）。
        id: WidgetId,
        /// 画面ルートからのノード位置列（`ui::activate_focus_at` の doc 参照）。
        path: Vec<usize>,
    },
    /// OS の戻る操作（Issue #726）。プラットフォームレイヤが macOS/Linux/Windows の
    /// Esc・Android のバックキー等のネイティブ入力をこのバリアントへ変換して渡す契約
    /// （`fandhe-egui-proxy::dispatch_back_if_requested` 参照）。`ui::request_back` へ
    /// 委譲し、画面の `.on_back()` ハンドラ（典型的には
    /// [`crate::ui::ScreenStack::pop`] を呼ぶ）を発火する。ハンドラ未設定の画面では
    /// 状態を変更せず `false` を返す。
    BackRequested,
    /// `path` で特定した `Node::Widget`（`Checkbox` / `ToggleSwitch` / `Slider` /
    /// `RadioButton`）の値変更（Issue #805 / 親 #804）。`ButtonActivatedAt`（#536）と
    /// 対称の id ガード付き path 方式で、プラットフォームレイヤが描画した標準部品
    /// （egui のネイティブチェックボックス・トグル・スライダー・ラジオ等）が発生させた
    /// 値変更を `ui::event::set_widget_value_at` へ委譲して状態へ反映する。
    /// `ButtonActivatedAt` が `id` 一致を見てから `on_click` を呼ぶのと同様、本
    /// バリアントも解決した `Node::Widget` の `Widget::id()` が `id` と一致する場合
    /// のみ `Widget::apply_value` を呼ぶ（描画時と dispatch 時の状態乖離による誤反映
    /// 防止、A08 対策）。`RadioButton` はこれに加えて `apply_value` 内部で
    /// `WidgetValue::Index` と自身の `value()` の一致も見るため、id と index の
    /// 二段ガードになる。
    WidgetValueChanged {
        /// 発火対象を特定する `WidgetId`（`Screen::checkbox` 等で付与した id と
        /// 一致させる）。
        id: WidgetId,
        /// 画面ルートからのノード位置列（`ui::set_widget_value_at` の doc 参照）。
        path: Vec<usize>,
        /// 反映する新しい値。
        value: WidgetValue,
    },
}

/// 状態 `S` を所有し、イベント→状態更新→再描画判定を配線する状態コンテナ。
///
/// `state()` は共有参照のみを公開し、書き込みは `dispatch` 経由の関数ポインタ呼び出しに限定する
/// （任意のクロージャキャプチャを遮断する AI 型安全 DSL の中核設計、モジュール冒頭のコメント参照）。
/// `version` は状態が更新されるたびに増加し、プラットフォーム統合レイヤが
/// 再描画要否を判定するためのフックとして使う想定（実描画確認は TASK-1.2 以降のスコープ）。
///
/// # Examples
///
/// ```
/// use fandhe_core::{Event, Screen, StateStore, WidgetId};
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
/// let mut store = StateStore::new(Counter::default());
///
/// let handled = store.dispatch(&screen, &Event::ButtonClicked { label: "increment".into() });
///
/// assert!(handled);
/// assert_eq!(store.state().count, 1);
/// assert_eq!(store.version(), 1);
/// ```
#[derive(Debug)]
pub struct StateStore<S> {
    state: S,
    version: u64,
}

impl<S> StateStore<S> {
    /// 初期状態 `state` を所有する状態コンテナを作成する。
    pub fn new(state: S) -> Self {
        Self { state, version: 0 }
    }

    /// 現在の状態への共有参照を返す。書き込みは `dispatch` 経由に限定するため、
    /// `&mut S` を返す公開 API はここには置かない（モジュール冒頭のコメント参照）。
    pub fn state(&self) -> &S {
        &self.state
    }

    /// 状態が更新されるたびに増加するバージョン番号。
    ///
    /// プラットフォーム統合レイヤ（未実装）が再描画要否を判定するためのフックとして使う想定。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// `screen` のノード列を宣言順に走査し、`event` に一致する最初のハンドラを発火する。
    ///
    /// 一致するノードが無ければ状態を変更せず `false` を返す（panic を API 契約にしない、
    /// coding-rust.md のエラーハンドリング方針）。発火時のみ `version` を進める。
    /// 同一ラベルが複数存在する場合は宣言順で先頭のノードのみ発火する
    /// （`ui::click_button` から踏襲した PoC-6 準拠の挙動）。
    pub fn dispatch(&mut self, screen: &Screen<S>, event: &Event) -> bool {
        let handled = match event {
            // 走査ロジックの実体は `ui::event::click_button` が単一ソースとして持つため、
            // ここでは公開 API へ委譲するだけの薄い窓口とする（モジュール冒頭のコメント参照）。
            Event::ButtonClicked { label } => click_button(screen, label, &mut self.state),
            Event::TextFieldChanged { label, value } => {
                Self::dispatch_text_field_changed(screen, label, value, &mut self.state)
            }
            Event::ButtonClickedAt { label, path } => {
                click_button_at(screen, path, label, &mut self.state)
            }
            Event::ButtonActivatedAt { id, path } => {
                activate_focus_at(screen, path, *id, &mut self.state)
            }
            Event::BackRequested => request_back(screen, &mut self.state),
            Event::WidgetValueChanged { id, path, value } => {
                set_widget_value_at(screen, path, *id, value, &mut self.state)
            }
        };
        if handled {
            self.version += 1;
        }
        handled
    }

    /// 外部 I/O（保存・HTTP・カメラ等、Phase 8）の完了結果を状態へ反映する。
    ///
    /// `dispatch` とは異なり `Event` を経由せず、呼び出し元が用意したクロージャ `f` を
    /// そのまま `&mut S` に対して実行する（モジュール冒頭のコメント参照）。`dispatch` の
    /// ような「一致しなければ何もしない」判定は持たず、`f` は無条件に実行され `version`
    /// は必ず進む。呼び出し元は「反映すべき完了である」ことを保証する契約とし、
    /// `ui::Node` ハンドラ（AI 生成コード）からは呼べない設計とする（`StateStore` を
    /// 握るのはプラットフォーム統合レイヤのみ、A04/A05 対策）。
    pub fn apply(&mut self, f: impl FnOnce(&mut S)) {
        f(&mut self.state);
        self.version += 1;
    }

    /// ラベルが一致する `Node::TextField` の `set` を発火する走査ロジック。
    ///
    /// `ui::click_button` に相当する処理を `TextField` 向けに提供する（`ui` モジュールは
    /// 現時点で `Button` 向けの公開ヘルパーしか持たないため、`state` モジュール側で
    /// 同等の走査を行う。走査対象バリアントが異なるだけで、探索アルゴリズム自体は
    /// `click_button` と同じ「宣言順で先頭一致のみ発火」である）。
    /// 走査は `Screen::iter`（非展開の線形走査）ベースのため、`List::expand` で
    /// 動的に生成された子ノード内の `TextField` は対象外。`Button` は
    /// `Event::ButtonClickedAt`（`ui::click_button_at`、Issue #491）で解決可能に
    /// なったが、`TextField` の動的生成子内解決は未対応（out-of-scope-tracking.md 対象。
    /// 子要素まで辿る描画走査は `Screen::visit_expanded` を参照）。
    fn dispatch_text_field_changed(
        screen: &Screen<S>,
        label: &str,
        value: &str,
        state: &mut S,
    ) -> bool {
        for node in screen.iter() {
            let Node::TextField {
                label: field_label,
                set,
                ..
            } = node
            else {
                continue;
            };
            if field_label == label {
                set(state, value.to_string());
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::WidgetId;

    /// テスト用の最小状態: ボタン押下回数とメモ欄を持つ。
    #[derive(Default)]
    struct Counter {
        count: u32,
        note: String,
    }

    fn increment(state: &mut Counter) {
        state.count += 1;
    }

    fn decrement(state: &mut Counter) {
        state.count = state.count.saturating_sub(1);
    }

    fn get_note(state: &Counter) -> String {
        state.note.clone()
    }

    fn set_note(state: &mut Counter, value: String) {
        state.note = value;
    }

    /// `ButtonClicked` が一致するとハンドラが発火し、状態と `version` が更新されることを確認する。
    #[test]
    fn dispatch_button_clicked_invokes_handler_and_bumps_version() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(1), "increment", increment);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonClicked {
                label: "increment".into(),
            },
        );

        assert!(handled);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// ラベル不一致の場合は `false` を返し、状態・`version` ともに変化しないことを確認する。
    #[test]
    fn dispatch_returns_false_and_keeps_state_when_label_not_found() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(1), "increment", increment);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonClicked {
                label: "no-such-button".into(),
            },
        );

        assert!(!handled);
        assert_eq!(store.state().count, 0);
        assert_eq!(store.version(), 0);
    }

    /// 同一ラベルが複数存在する場合、宣言順で先頭のノードのみ発火することを確認する。
    #[test]
    fn dispatch_fires_only_first_when_labels_duplicate() {
        let screen: Screen<Counter> = Screen::new("test")
            .button(WidgetId::from_raw(1), "action", increment)
            .button(WidgetId::from_raw(2), "action", decrement);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonClicked {
                label: "action".into(),
            },
        );

        assert!(handled);
        assert_eq!(store.state().count, 1);
    }

    /// イベントを連続 dispatch すると状態が累積更新され、`version` も都度進むことを確認する
    /// （イベント→状態更新ループ全体の配線を検証する）。
    #[test]
    fn dispatch_accumulates_state_across_multiple_events() {
        let screen: Screen<Counter> = Screen::new("test")
            .button(WidgetId::from_raw(1), "increment", increment)
            .button(WidgetId::from_raw(2), "decrement", decrement);
        let mut store = StateStore::new(Counter::default());

        assert!(store.dispatch(
            &screen,
            &Event::ButtonClicked {
                label: "increment".into()
            }
        ));
        assert!(store.dispatch(
            &screen,
            &Event::ButtonClicked {
                label: "increment".into()
            }
        ));
        assert!(store.dispatch(
            &screen,
            &Event::ButtonClicked {
                label: "decrement".into()
            }
        ));

        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 3);
    }

    /// `TextFieldChanged` が一致するとハンドラが発火し、状態と `version` が更新されることを確認する。
    #[test]
    fn dispatch_text_field_changed_invokes_handler_and_bumps_version() {
        let screen: Screen<Counter> = Screen::new("test").with_node(Node::TextField {
            label: "memo".to_string(),
            get: get_note,
            set: set_note,
        });
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::TextFieldChanged {
                label: "memo".into(),
                value: "更新後".into(),
            },
        );

        assert!(handled);
        assert_eq!(store.state().note, "更新後");
        assert_eq!(store.version(), 1);
    }

    /// ラベル不一致の場合は `false` を返し、状態・`version` ともに変化しないことを確認する。
    #[test]
    fn dispatch_text_field_changed_returns_false_when_label_not_found() {
        let screen: Screen<Counter> = Screen::new("test").with_node(Node::TextField {
            label: "memo".to_string(),
            get: get_note,
            set: set_note,
        });
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::TextFieldChanged {
                label: "no-such-field".into(),
                value: "更新後".into(),
            },
        );

        assert!(!handled);
        assert_eq!(store.state().note, "");
        assert_eq!(store.version(), 0);
    }

    /// `ButtonClickedAt`（Issue #491）が動的生成子内の `IndexedButton` を正しく解決し、
    /// 状態と `version` が更新されることを確認する。
    #[test]
    fn dispatch_button_clicked_at_resolves_indexed_button_inside_list_and_bumps_version() {
        fn expand_indexed_buttons(state: &Counter) -> Vec<Node<Counter>> {
            let _ = state;
            vec![Node::IndexedButton {
                label: "+1".to_string(),
                index: 0,
                on_click: |state: &mut Counter, _index: usize| state.count += 1,
                id: WidgetId::from_raw(1),
            }]
        }

        let screen: Screen<Counter> = Screen::new("test").list(expand_indexed_buttons);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonClickedAt {
                label: "+1".into(),
                path: vec![0, 0],
            },
        );

        assert!(handled);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// 不正な path（範囲外）を指定した場合は `false` を返し、状態・`version` ともに
    /// 変化しないことを確認する（Issue #491）。
    #[test]
    fn dispatch_button_clicked_at_returns_false_and_keeps_state_when_path_invalid() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(1), "increment", increment);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonClickedAt {
                label: "increment".into(),
                path: vec![5],
            },
        );

        assert!(!handled);
        assert_eq!(store.state().count, 0);
        assert_eq!(store.version(), 0);
    }

    /// `ButtonActivatedAt`（Issue #536）が id 一致で `on_click` を発火し、
    /// 状態と `version` が更新されることを確認する。
    #[test]
    fn dispatch_button_activated_at_fires_on_id_match_and_bumps_version() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(7), "increment", increment);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonActivatedAt {
                id: WidgetId::from_raw(7),
                path: vec![0],
            },
        );

        assert!(handled);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// id 不一致の場合は `false` を返し、状態・`version` ともに変化しないことを確認する
    /// （誤発火防止ガードが label ではなく id で機能していることの確認、#536）。
    #[test]
    fn dispatch_button_activated_at_rejects_on_id_mismatch() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(7), "increment", increment);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonActivatedAt {
                id: WidgetId::from_raw(999),
                path: vec![0],
            },
        );

        assert!(!handled);
        assert_eq!(store.state().count, 0);
        assert_eq!(store.version(), 0);
    }

    /// path が範囲外の場合は `false` を返し、状態・`version` ともに変化しないことを確認する。
    #[test]
    fn dispatch_button_activated_at_returns_false_when_path_out_of_range() {
        let screen: Screen<Counter> =
            Screen::new("test").button(WidgetId::from_raw(7), "increment", increment);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonActivatedAt {
                id: WidgetId::from_raw(7),
                path: vec![5],
            },
        );

        assert!(!handled);
        assert_eq!(store.state().count, 0);
        assert_eq!(store.version(), 0);
    }

    /// 動的生成子（`VirtualList` 内の `IndexedButton`）でも id 一致で発火することを確認する
    /// （`ui::event::activate_focus_at_fires_indexed_button_on_id_match` の StateStore 版、#536）。
    #[test]
    fn dispatch_button_activated_at_resolves_indexed_button_inside_list() {
        fn expand_indexed_buttons(state: &Counter) -> Vec<Node<Counter>> {
            let _ = state;
            vec![Node::IndexedButton {
                label: "+1".to_string(),
                index: 0,
                on_click: |state: &mut Counter, _index: usize| state.count += 1,
                id: WidgetId::from_raw(1),
            }]
        }

        let screen: Screen<Counter> = Screen::new("test").list(expand_indexed_buttons);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonActivatedAt {
                id: WidgetId::from_raw(1),
                path: vec![0, 0],
            },
        );

        assert!(handled);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// `apply`（Issue #745）にクロージャを渡すと状態が更新され、`version` が進むことを確認する。
    #[test]
    fn apply_mutates_state_and_bumps_version() {
        let mut store = StateStore::new(Counter::default());

        store.apply(|state| state.count += 1);

        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// `apply` を複数回呼ぶと、呼び出し回数分だけ `version` が進むことを確認する
    /// （`dispatch` と異なり毎回無条件に反映される、モジュール冒頭のコメント参照）。
    #[test]
    fn apply_accumulates_across_multiple_calls() {
        let mut store = StateStore::new(Counter::default());

        store.apply(|state| state.count += 1);
        store.apply(|state| state.note = "done".to_string());
        store.apply(|state| state.count += 1);

        assert_eq!(store.state().count, 2);
        assert_eq!(store.state().note, "done");
        assert_eq!(store.version(), 3);
    }

    /// `WidgetValueChanged`（Issue #805）が一致するノードに値を反映し、`version` を
    /// 進めることを確認する（走査ロジック自体の検証は `ui::event` 側のテストが担う）。
    #[test]
    fn dispatch_widget_value_changed_invokes_apply_value_and_bumps_version() {
        use crate::ui::Screen as UiScreen;

        fn get_agreed(state: &Counter) -> bool {
            state.count != 0
        }
        fn set_agreed(state: &mut Counter, value: bool) {
            state.count = value as u32;
        }

        let screen: UiScreen<Counter> =
            UiScreen::new("test").checkbox(WidgetId::from_raw(1), "agree", get_agreed, set_agreed);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::WidgetValueChanged {
                id: WidgetId::from_raw(1),
                path: vec![0],
                value: WidgetValue::Bool(true),
            },
        );

        assert!(handled);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// path が範囲外の場合は `false` を返し、状態・`version` ともに変化しないことを確認する。
    #[test]
    fn dispatch_widget_value_changed_returns_false_when_path_invalid() {
        use crate::ui::Screen as UiScreen;

        fn get_agreed(state: &Counter) -> bool {
            state.count != 0
        }
        fn set_agreed(state: &mut Counter, value: bool) {
            state.count = value as u32;
        }

        let screen: UiScreen<Counter> =
            UiScreen::new("test").checkbox(WidgetId::from_raw(1), "agree", get_agreed, set_agreed);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::WidgetValueChanged {
                id: WidgetId::from_raw(1),
                path: vec![5],
                value: WidgetValue::Bool(true),
            },
        );

        assert!(!handled);
        assert_eq!(store.state().count, 0);
        assert_eq!(store.version(), 0);
    }

    /// id 不一致の場合は `false` を返し、状態・`version` ともに変化しないことを確認する
    /// （`ButtonActivatedAt` と対称の id ガード、レビュー指摘対応）。
    #[test]
    fn dispatch_widget_value_changed_returns_false_when_id_mismatch() {
        use crate::ui::Screen as UiScreen;

        fn get_agreed(state: &Counter) -> bool {
            state.count != 0
        }
        fn set_agreed(state: &mut Counter, value: bool) {
            state.count = value as u32;
        }

        let screen: UiScreen<Counter> =
            UiScreen::new("test").checkbox(WidgetId::from_raw(1), "agree", get_agreed, set_agreed);
        let mut store = StateStore::new(Counter::default());

        let handled = store.dispatch(
            &screen,
            &Event::WidgetValueChanged {
                id: WidgetId::from_raw(999),
                path: vec![0],
                value: WidgetValue::Bool(true),
            },
        );

        assert!(!handled);
        assert_eq!(store.state().count, 0);
        assert_eq!(store.version(), 0);
    }
}
