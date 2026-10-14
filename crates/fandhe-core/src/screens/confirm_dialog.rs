//! M7（#724）の型サンプル「確認ダイアログ」（Issue #729 / #739）。メモ一覧の各行にある
//! 「削除」ボタンを押すと、背後の一覧を残したまま前面に確認パネル
//! （「○○を削除しますか？」+ キャンセル / 削除）が現れ、キャンセル・削除・
//! Esc（`.on_back()`）・パネル外タップのいずれでも閉じる 1 画面。
//! `FANDHE_PROXY_APP=confirm_dialog` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! から呼び出される（`screens/mod.rs` の `pub mod confirm_dialog;` と対）。
//!
//! ## `DialogWidget` 方式（Issue #739）
//!
//! `Screen::dialog`（`Dialog<S>`、S46）は実描画・開閉イベント未配線のまま
//! （`docs/ai/SCHEMA.md` 参照）だが、[`crate::ui::DialogWidget`]（S47）に
//! `Widget::children` 経由の子ノード降下（[`DialogWidget::with_content`]）が加わったため
//! （`ui/widget.rs` / `ui/node.rs::expand_children` / `ui/event.rs::resolve_node_at`）、
//! 「一覧区画は常に描画したまま、その後ろに `Node::Widget(DialogWidget)` を重ねる」
//! 構成が組めるようになった。egui-proxy は開状態の `DialogWidget` を前面パネル + スクリムで
//! 描画し、パネル外タップを `dismiss_label` と同名のパネル内 `Button`
//! （[`DISMISS_LABEL`] = 「キャンセル」 = [`CANCEL_ID`]）へ `Event::ButtonClickedAt` として
//! dispatch する（egui-proxy 側の実装は別 Issue コメント参照）。これにより従来の
//! 「一覧区画 ⇔ 確認区画」を排他的に切り替える `nav` 方式（PR #737）とは異なり、背後の
//! 一覧が常に存在するため「外側」領域が実在し、外側タップで閉じる UI を実現できる。
//!
//! 開閉状態の SSOT は引き続きアプリ状態 `S`（[`is_confirming`] が
//! `state.pending` から導出、[`set_confirming`] が閉方向のみ書き込む）。
//! `confirm_dialog_screen(state) -> Screen<ConfirmDialogState>` のシグネチャ・
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `nav` 契約は変更しない
//! （registry 側の変更は不要）。
//!
//! 確認文言（「○○を削除しますか？」）は `DialogWidget` 本体の `body` ではなく、
//! `expand_confirm` が返す展開可能な子ノード（`Node::Text`、先頭要素）として持つ
//! （PR #792 codex-review 指摘 B）。`body` を使うと `render_headless` /
//! `convert_node` 等の非展開走査（`Widget::children` を呼ばない経路）に文言が
//! 出ず、既存出力が退行するため。`DialogWidget` の `body` は空文字。

use crate::ui::{DialogWidget, Node, Screen, WidgetId};

/// 一覧行の「削除」ボタンの `WidgetId` 起点（`ROW_ID_BASE + index`、SCHEMA.md パターン 7）。
const ROW_ID_BASE: u64 = 1000;
/// 確認パネル「キャンセル」ボタンの `WidgetId`。
const CANCEL_ID: WidgetId = WidgetId::from_raw(1);
/// 確認パネル「削除」ボタンの `WidgetId`。
const CONFIRM_ID: WidgetId = WidgetId::from_raw(2);
/// 確認パネル本体（`DialogWidget`）の `WidgetId`。
const DIALOG_ID: WidgetId = WidgetId::from_raw(3);

/// dismiss（パネル外タップ・`DialogWidget` の閉じるボタン相当）のブリッジラベル。
/// パネル内「キャンセル」ボタンのラベルと確実に一致させ、egui-proxy がパネル外タップを
/// この同名ボタンの path へ変換して dispatch できるようにする（モジュール doc 参照）。
const DISMISS_LABEL: &str = "キャンセル";

/// 確認ダイアログ画面が保持する状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialogState {
    /// 一覧のメモ（ダミー文言）。
    pub memos: Vec<String>,
    /// 削除確認中のメモ index。`None` なら確認パネルは出ない。
    pub pending: Option<usize>,
}

impl Default for ConfirmDialogState {
    /// 画面レジストリ（`generated_screens!`）は `Default::default()` で初期状態を
    /// 作るため、ダミーのメモ 3 件・確認パネル非表示で初期化する
    /// （`screens/settings.rs` と同じ判断、Issue #729 コメント参照）。
    fn default() -> Self {
        ConfirmDialogState {
            memos: vec![
                "メモ 1".to_string(),
                "メモ 2".to_string(),
                "メモ 3".to_string(),
            ],
            pending: None,
        }
    }
}

/// 一覧行の「削除」ボタンのハンドラ。`index` が範囲外なら何もしない
/// （panic を API 契約にしない、coding-rust.md のエラーハンドリング方針）。
/// 確認パネルが実際に表示中（[`is_confirming`]）の場合のみ無視する。egui-proxy の
/// backdrop（前面パネル + スクリム描画）は視覚的に背景操作を遮るのみで、
/// `StateStore::dispatch` 経由の一覧ボタン操作（path 解決した `ButtonClickedAt`）は
/// ハンドラに直接届くため防げない。「モーダル表示中は背景操作を無効化する」契約を
/// 描画バックエンドに依存せずコア側（ハンドラ）でも保証する（Issue #739、
/// PR #792 codex-review 2 巡目 P1 指摘）。`pending` が範囲外を指す（既に確定操作後の
/// 無効値）場合は `is_confirming` が `false` を返すため、このガードでは弾かず
/// `index < state.memos.len()` の通常経路で復帰できる（3 巡目 P1 指摘）。
pub fn request_delete(state: &mut ConfirmDialogState, index: usize) {
    if is_confirming(state) {
        return;
    }
    if index < state.memos.len() {
        state.pending = Some(index);
    }
}

/// 確認パネル「キャンセル」ボタン・`.on_back()`（Esc / Android バックキー）双方の
/// ハンドラ。確認パネルを閉じるのみで `memos` は変更しない。
pub fn cancel_delete(state: &mut ConfirmDialogState) {
    state.pending = None;
}

/// 確認パネル「削除」ボタンのハンドラ。`pending` が有効な index を指す場合のみ
/// 該当メモを削除する（範囲外・未確定は無視、panic しない）。
pub fn confirm_delete(state: &mut ConfirmDialogState) {
    if let Some(index) = state.pending.take() {
        if index < state.memos.len() {
            state.memos.remove(index);
        }
    }
}

/// `DialogWidget::new_dynamic` の `open_source`。`pending` が `memos` 内の有効な
/// index を指す間だけ確認パネルを開状態とする（範囲外 pending は非表示、
/// PR #737 レビュー P1 指摘と同じ理由）。
fn is_confirming(state: &ConfirmDialogState) -> bool {
    state
        .pending
        .is_some_and(|index| state.memos.get(index).is_some())
}

/// `DialogWidget::new_dynamic` の `set`（書き込み経路）。`false`（閉方向）のときのみ
/// `pending` をクリアする。開く操作は [`request_delete`] の責務であり、本関数の
/// `true` 引数は no-op とする（`DialogWidget` 側の書き込み経路はレガシー
/// `Event::ButtonClicked` ブリッジ・`StateStore::dispatch` 経由でのみ呼ばれ、開く方向の
/// 呼び出しは発生しない契約、`dialog_widget.rs` モジュール doc 参照）。
fn set_confirming(state: &mut ConfirmDialogState, open: bool) {
    if !open {
        state.pending = None;
    }
}

/// 一覧区画の展開（`Node::List::expand`）。各メモに「削除」ボタン付き 1 行を割り当てる。
/// 空なら案内文のみを返す。確認パネルの開閉に関わらず常に描画される
/// （モジュール doc 参照。背後の一覧を残すことでパネル外領域を実在させる）。
fn expand_memo_rows(state: &ConfirmDialogState) -> Vec<Node<ConfirmDialogState>> {
    if state.memos.is_empty() {
        return vec![Node::Text("メモはありません".to_string())];
    }
    state
        .memos
        .iter()
        .enumerate()
        .flat_map(|(i, name)| {
            [
                Node::Text(name.clone()),
                Node::IndexedButton {
                    label: "削除".to_string(),
                    index: i,
                    on_click: request_delete,
                    id: WidgetId::from_raw(ROW_ID_BASE + i as u64),
                },
            ]
        })
        .collect()
}

/// `DialogWidget::with_content` へ渡す子ノード展開。確認文言（`Node::Text`）+
/// 「キャンセル」「削除」の 2 ボタンを返す（PR #792 codex-review 指摘 B: 文言を
/// パネル本体の `body` へ移すと `render_headless` 等の非展開走査に出なくなり
/// 既存出力が退行するため、展開可能な子ノードへ戻した。`DialogWidget` 本体の
/// `body` は空文字とする、[`confirm_dialog_screen`] 参照）。`DialogWidget::children`
/// が開状態でのみ本関数を呼ぶ契約のため、ここでは追加の開閉判定を行わない。
/// `pending` が範囲外（既に確定操作後で本関数の再入があり得る）の場合は文言を
/// 空にする（安全側の判断、旧 `confirm_body` と同じ方針）。
fn expand_confirm(state: &ConfirmDialogState) -> Vec<Node<ConfirmDialogState>> {
    let message = match state.pending.and_then(|index| state.memos.get(index)) {
        Some(name) => format!("「{name}」を削除しますか？"),
        None => String::new(),
    };
    vec![
        Node::Text(message),
        Node::Button {
            label: DISMISS_LABEL.to_string(),
            on_click: cancel_delete,
            id: CANCEL_ID,
        },
        Node::Button {
            label: "削除".to_string(),
            on_click: confirm_delete,
            id: CONFIRM_ID,
        },
    ]
}

/// 確認ダイアログ画面。一覧区画を常に描画したうえで、その後ろに確認パネル
/// （`Node::Widget(DialogWidget)`）を重ねる（モジュール doc 参照）。
/// `crates/fandhe-egui-proxy/src/screens/registry.rs` の `nav` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約（`screens/list_detail.rs::current_screen`
/// と同型、`docs/ai/SCHEMA.md` パターン 8）。
///
/// `.on_back(cancel_delete)` は確認表示中のみ登録する（現状維持）。非表示時は
/// `.on_back()` 未設定のため `request_back` は `false` を返し、Esc / Android
/// バックキーは上位の画面遷移・アプリ終了へそのまま渡る（PR #737 レビュー指摘と
/// 同じ契約、`screens/list_detail.rs` の `back_requested_pops_detail_but_is_no_op_on_root`
/// と同じ設計判断）。
pub fn confirm_dialog_screen(state: &ConfirmDialogState) -> Screen<ConfirmDialogState> {
    let confirming = is_confirming(state);
    let dialog = DialogWidget::new_dynamic(
        DIALOG_ID,
        "削除の確認",
        "",
        DISMISS_LABEL,
        confirming,
        is_confirming,
        set_confirming,
    )
    .with_content(expand_confirm);

    let screen = Screen::new("メモ")
        .text("メモ一覧")
        .list(expand_memo_rows)
        .with_node(Node::Widget(Box::new(dialog)));

    if confirming {
        screen.on_back(cancel_delete)
    } else {
        screen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateStore;
    use crate::ui::{
        DialogHitRegion, InteractionInput, Measurement, click_button_at, render_headless,
    };

    fn initial_state() -> ConfirmDialogState {
        ConfirmDialogState::default()
    }

    /// 初期状態（確認パネル非表示）では、パネルのボタン文言は出ず、一覧の
    /// 「削除」ボタンのみが出ることを確認する。
    #[test]
    fn initial_render_shows_list_without_confirm_panel() {
        let state = initial_state();
        let screen = confirm_dialog_screen(&state);
        let lines = render_headless(&screen, &state);

        assert!(lines.iter().any(|line| line == "(削除)"));
        assert!(!lines.iter().any(|line| line == "(キャンセル)"));
    }

    /// `request_delete(0)` 後は確認パネルの確認文言・「キャンセル」「削除」ボタンが
    /// 出現し、背後の一覧行（削除ボタン込み）も引き続き出力されることを確認する
    /// （一覧を透過表示したまま前面にパネルを重ねる構成、モジュール doc 参照）。
    /// 確認文言のヘッドレス出力は PR #792 codex-review 指摘 B（旧テストの
    /// 文言 assert を復活）。
    #[test]
    fn render_after_request_delete_shows_confirm_panel_and_list() {
        let mut state = initial_state();
        request_delete(&mut state, 0);
        let screen = confirm_dialog_screen(&state);
        let lines = render_headless(&screen, &state);

        assert!(
            lines
                .iter()
                .any(|line| line == "「メモ 1」を削除しますか？")
        );
        assert!(lines.iter().any(|line| line == "(キャンセル)"));
        assert!(lines.iter().any(|line| line == "(削除)"));
        assert!(lines.iter().any(|line| line.contains("メモ 1")));
    }

    /// 外側タップ回帰（Issue #739）: 確認表示中の `Node::Widget` を取り出し、
    /// `clone_box` + `sync_from_state` で reconcile 済みクローンを作った上で
    /// `hit_region` がパネル外座標を `Scrim` と判定し、`apply_pointer` の
    /// press → release が返す `Event` を `StateStore::dispatch` すると
    /// `pending` がクリアされ、再描画で確認ボタンが消えることを確認する。
    ///
    /// `Event::ButtonClicked { label: "キャンセル" }` は非展開走査
    /// （`click_button`）ではパネル内ボタンへ到達できないため、egui-proxy が行うのと
    /// 同じ変換（path 解決 → `ButtonClickedAt`）をテスト側でも行う
    /// （`ui::event::resolve_node_at` 相当、`path = [dialog_index, 0]` は
    /// `expand_confirm` の先頭要素＝「キャンセル」ボタン）。
    #[test]
    fn tapping_outside_the_panel_dismisses_via_dialog_widget_dismiss_button() {
        let mut state = initial_state();
        request_delete(&mut state, 0);
        let screen = confirm_dialog_screen(&state);

        // `Node::Widget` は末尾（一覧の後ろ）に置いた 1 件のみ。
        let dialog_index = screen
            .iter()
            .position(|node| matches!(node, Node::Widget(_)))
            .expect("DialogWidget ノードが存在する");
        let widget_node = screen
            .iter()
            .nth(dialog_index)
            .expect("dialog_index は有効な範囲");
        let Node::Widget(widget) = widget_node else {
            panic!("Widget バリアントであるべき");
        };

        let mut synced = widget.clone_box();
        synced.sync_from_state(&state);
        let dialog = synced
            .as_any()
            .expect("DialogWidget は as_any をオーバーライドする")
            .downcast_ref::<DialogWidget<ConfirmDialogState>>()
            .expect("DialogWidget へ downcast できる");

        let region = dialog.hit_region(
            (700.0, 500.0),
            (0.0, 0.0),
            Measurement {
                width: 800.0,
                height: 600.0,
            },
        );
        assert_eq!(region, DialogHitRegion::Scrim);

        let mut synced_mut = widget.clone_box();
        synced_mut.sync_from_state(&state);
        let dialog_mut = synced_mut
            .as_any()
            .and_then(|any| any.downcast_ref::<DialogWidget<ConfirmDialogState>>())
            .expect("DialogWidget へ downcast できる");
        let mut dialog_mut = dialog_mut.clone();
        dialog_mut.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);
        let event = dialog_mut
            .apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim)
            .expect("スクリム press→release は dismiss イベントを返す");

        // `DialogWidget::apply_pointer` はレガシー `Event::ButtonClicked { label }` を
        // 返すが、動的生成子（`with_content`）内のボタンは非展開走査（`click_button`）
        // では解決できないため、egui-proxy が行うのと同じ変換（path 解決込みの
        // `ButtonClickedAt`）へ組み替えて `StateStore::dispatch` する
        // （`ui::event::resolve_node_at` 相当、モジュール冒頭のコメント参照）。
        let crate::state::Event::ButtonClicked { label } = event else {
            panic!("ButtonClicked であるべき");
        };
        assert_eq!(label, DISMISS_LABEL);

        let mut store = StateStore::new(state);
        let handled = store.dispatch(
            &screen,
            &crate::state::Event::ButtonClickedAt {
                label,
                // path = [dialog_index, 1]: `expand_confirm` の 2 番目の要素
                // （0: 確認文言の Text、1: 「キャンセル」ボタン。PR #792
                // codex-review 指摘 B で先頭に Text を戻したため index がずれた）。
                path: vec![dialog_index, 1],
            },
        );
        assert!(handled);
        assert_eq!(store.state().pending, None);

        let rendered = render_headless(&screen, store.state());
        assert!(!rendered.iter().any(|line| line == "(キャンセル)"));
    }

    /// パネル内「削除」を `ButtonClickedAt` で dispatch すると `memos` が 1 件減り、
    /// `pending` がクリアされることを確認する（Issue #739）。
    #[test]
    fn clicking_confirm_button_inside_panel_removes_memo() {
        let mut state = initial_state();
        request_delete(&mut state, 0);
        let screen = confirm_dialog_screen(&state);

        let dialog_index = screen
            .iter()
            .position(|node| matches!(node, Node::Widget(_)))
            .expect("DialogWidget ノードが存在する");

        // path = [dialog_index, 2]: `expand_confirm` の 3 番目の要素＝「削除」ボタン
        // （0: 確認文言の Text、1: 「キャンセル」ボタン、2: 「削除」ボタン。
        // PR #792 codex-review 指摘 B で先頭に Text を戻したため index がずれた）。
        let handled = click_button_at(&screen, &[dialog_index, 2], "削除", &mut state);

        assert!(handled);
        assert_eq!(
            state.memos,
            vec!["メモ 2".to_string(), "メモ 3".to_string()]
        );
        assert_eq!(state.pending, None);
    }

    /// PR #792 codex-review 3 巡目 P1 指摘の回帰: `pending` が範囲外
    /// （`memos` 1 件・`pending == Some(1)`）を指す状態は `is_confirming` が
    /// `false` を返すため確認パネルは非表示。この状態で一覧の「削除」
    /// （`request_delete(0)`）を押すと、旧ガード（`pending.is_some()`）では
    /// 無視されて画面操作で復帰できなかったが、`is_confirming` 基準のガードでは
    /// 通常どおり `pending` が有効値へ更新される。
    #[test]
    fn request_delete_recovers_when_pending_is_out_of_range() {
        let mut state = ConfirmDialogState {
            memos: vec!["メモ 1".to_string()],
            pending: Some(1),
        };
        assert!(!is_confirming(&state));

        request_delete(&mut state, 0);

        assert_eq!(state.pending, Some(0));
    }

    /// PR #792 codex-review 3 巡目 P2 指摘の回帰: `request_delete(0)` 後の画面を
    /// `visit_windowed` で走査しても、`Node::Widget`（`DialogWidget`）配下の
    /// 確認文言・「キャンセル」「削除」ボタンが `visit_expanded` 経由の走査と同様に
    /// 得られることを確認する（`visit_node_windowed` が List / VirtualList に加え
    /// Widget の子へも降下するようになったため）。
    #[test]
    fn visit_windowed_descends_into_dialog_widget_children() {
        let mut state = initial_state();
        request_delete(&mut state, 0);
        let screen = confirm_dialog_screen(&state);

        let mut labels: Vec<String> = Vec::new();
        screen.visit_windowed(&state, &(0..100), |node| match node {
            Node::Text(text) => labels.push(text.clone()),
            Node::Button { label, .. } => labels.push(label.clone()),
            _ => {}
        });

        assert!(labels.iter().any(|l| l == "「メモ 1」を削除しますか？"));
        assert!(labels.iter().any(|l| l == "キャンセル"));
        assert!(labels.iter().any(|l| l == "削除"));
    }
}
