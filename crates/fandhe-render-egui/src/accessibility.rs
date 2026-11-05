//! `fandhe_core::ui::widget::AccessibilityRole` → `accesskit::Role` 変換と、
//! ウィジェットのロール・アクセシブルネーム・インタラクション状態から
//! `accesskit::Node`（AX ノード）を構築するマッピング層（TASK-9.1-S2、REQ-9、Issue #215）。
//!
//! `fandhe-core` は `accesskit` へ依存しない設計（依存方向規約、
//! `.claude/rules/coding-rust.md`）であり、`AccessibilityRole::doc` が定める
//! 「プラットフォームレイヤ側で `accesskit::Role` へ変換する」契約を、本モジュールが
//! `fandhe-egui-proxy`（プラットフォーム最適化レイヤ）側で実装する。
//!
//! 本モジュールはターゲット非依存の純粋関数のみで構成する（egui の "accesskit"
//! feature・`accesskit_macos` 等のプラットフォームアダプタには一切依存しない）。
//! egui/AccessKit の描画経路への実配線（`ctx.accesskit_node_builder` 呼び出し）は
//! `lib.rs` の `render_screen` 側（macOS / Android / Windows 限定、
//! `#[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]`、
//! Issue #1001 で Windows を追加）が本モジュールの関数を呼び出す形で行う。

use accesskit::{Action, Node, Role};
use fandhe_core::{AccessibilityRole, InteractionState};

/// `fandhe-core` の [`AccessibilityRole`] を `accesskit::Role` へ変換する。
///
/// ワイルドカード腕を持たない網羅 match とする。`AccessibilityRole` へ新規
/// バリアントが追加された際に本関数がコンパイルエラーで検知できるようにするための
/// 意図的な設計判断（TASK-9.1-S2 計画）。18 種すべてに 1:1 対応する
/// `accesskit::Role`（0.21.1）が存在することを確認済み（`Separator` →
/// `Role::Splitter`、WAI-ARIA `separator` 相当）。
pub fn to_accesskit_role(role: AccessibilityRole) -> Role {
    match role {
        AccessibilityRole::Button => Role::Button,
        AccessibilityRole::TextInput => Role::TextInput,
        AccessibilityRole::List => Role::List,
        AccessibilityRole::Label => Role::Label,
        AccessibilityRole::CheckBox => Role::CheckBox,
        AccessibilityRole::RadioButton => Role::RadioButton,
        AccessibilityRole::RadioGroup => Role::RadioGroup,
        AccessibilityRole::Switch => Role::Switch,
        AccessibilityRole::Slider => Role::Slider,
        AccessibilityRole::ProgressIndicator => Role::ProgressIndicator,
        AccessibilityRole::Image => Role::Image,
        AccessibilityRole::ScrollView => Role::ScrollView,
        AccessibilityRole::Group => Role::Group,
        AccessibilityRole::Dialog => Role::Dialog,
        AccessibilityRole::ComboBox => Role::ComboBox,
        AccessibilityRole::TabList => Role::TabList,
        AccessibilityRole::Tooltip => Role::Tooltip,
        AccessibilityRole::Separator => Role::Splitter,
    }
}

/// `role` が操作可能（クリック・フォーカス等のアクションを持ちうる）かどうかを判定する。
///
/// `Dialog` / `RadioGroup` / `List` はコンテナロールだが、AT（スクリーンリーダー）が
/// フォーカスを当てて内部へ移動できる必要があるため `Action::Focus` を付与する
/// （`Dialog` はモーダルとしてフォーカストラップの起点になり、`RadioGroup` は
/// roving tabindex のコンテナ、`List` はリストボックス相当としてフォーカス移動の
/// 起点になる、いずれも WAI-ARIA の実務慣行）。
///
/// 表示専用ロール（`Label` / `Image` / `ScrollView` / `Group` / `Tooltip` /
/// `Separator` / `ProgressIndicator`）には `Action::Focus` を付与しない
/// （スクリーンリーダーの roving focus 対象を実際に操作可能な要素・
/// ナビゲーション起点となるコンテナへ限定する）。
fn is_operable(role: AccessibilityRole) -> bool {
    matches!(
        role,
        AccessibilityRole::Button
            | AccessibilityRole::TextInput
            | AccessibilityRole::CheckBox
            | AccessibilityRole::RadioButton
            | AccessibilityRole::Switch
            | AccessibilityRole::Slider
            | AccessibilityRole::ComboBox
            | AccessibilityRole::TabList
            | AccessibilityRole::Dialog
            | AccessibilityRole::RadioGroup
            | AccessibilityRole::List
    )
}

/// `role` がクリック相当の既定アクションを持つかどうかを判定する。
///
/// `Action::Click` は「単一クリック・タップ相当の操作」（accesskit 0.21.1 doc）であり、
/// 値の直接編集を伴う `TextInput` / `Slider` へは付与しない
/// （誤ってクリックのみで確定してしまう AT 側の誤操作を避ける安全側の判断）。
fn has_click_action(role: AccessibilityRole) -> bool {
    matches!(
        role,
        AccessibilityRole::Button
            | AccessibilityRole::CheckBox
            | AccessibilityRole::RadioButton
            | AccessibilityRole::Switch
            | AccessibilityRole::ComboBox
    )
}

/// ロール・アクセシブルネーム・インタラクション状態から `accesskit::Node` を構築する。
///
/// `label` は [`fandhe_core::ui::widget::Widget::accessibility_label`] が返す値を
/// そのまま渡す想定（画面上に既に表示されているラベル文字列のみを運ぶ契約、
/// OWASP A03 対策・`.claude/rules/security.md` 準拠）。`interaction.disabled` を
/// `Node::set_disabled` へ反映し、操作可能ロールには `Action::Focus`（および
/// クリック相当ロールには `Action::Click`）を付与する。
///
/// `set_bounds` によるレイアウト位置の反映は taffy 統合後のスコープ（out-of-scope、
/// TASK-9.1-S2 計画 §8）のため、本関数では扱わない。
pub fn widget_ax_node(
    role: AccessibilityRole,
    label: Option<&str>,
    interaction: InteractionState,
) -> Node {
    let mut node = Node::new(to_accesskit_role(role));

    if let Some(label) = label {
        node.set_label(label);
    }

    if interaction.disabled {
        node.set_disabled();
    }

    if is_operable(role) {
        node.add_action(Action::Focus);
    }
    if has_click_action(role) {
        node.add_action(Action::Click);
    }

    node
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- to_accesskit_role: 18 バリアント全件の対応表（TASK-9.1-S2 検証方法 §6-5） ---

    #[test]
    fn to_accesskit_role_covers_all_eighteen_variants() {
        let cases = [
            (AccessibilityRole::Button, Role::Button),
            (AccessibilityRole::TextInput, Role::TextInput),
            (AccessibilityRole::List, Role::List),
            (AccessibilityRole::Label, Role::Label),
            (AccessibilityRole::CheckBox, Role::CheckBox),
            (AccessibilityRole::RadioButton, Role::RadioButton),
            (AccessibilityRole::RadioGroup, Role::RadioGroup),
            (AccessibilityRole::Switch, Role::Switch),
            (AccessibilityRole::Slider, Role::Slider),
            (
                AccessibilityRole::ProgressIndicator,
                Role::ProgressIndicator,
            ),
            (AccessibilityRole::Image, Role::Image),
            (AccessibilityRole::ScrollView, Role::ScrollView),
            (AccessibilityRole::Group, Role::Group),
            (AccessibilityRole::Dialog, Role::Dialog),
            (AccessibilityRole::ComboBox, Role::ComboBox),
            (AccessibilityRole::TabList, Role::TabList),
            (AccessibilityRole::Tooltip, Role::Tooltip),
            (AccessibilityRole::Separator, Role::Splitter),
        ];

        assert_eq!(
            cases.len(),
            18,
            "AccessibilityRole の全 18 バリアントを網羅する"
        );
        for (role, expected) in cases {
            assert_eq!(to_accesskit_role(role), expected);
        }
    }

    #[test]
    fn widget_ax_node_sets_label_when_present() {
        let node = widget_ax_node(
            AccessibilityRole::Button,
            Some("送信"),
            InteractionState::default(),
        );

        assert_eq!(node.label(), Some("送信"));
        assert_eq!(node.role(), Role::Button);
    }

    #[test]
    fn widget_ax_node_leaves_label_unset_when_none() {
        let node = widget_ax_node(AccessibilityRole::Image, None, InteractionState::default());

        assert_eq!(node.label(), None);
    }

    #[test]
    fn widget_ax_node_reflects_disabled_interaction_state() {
        let mut interaction = InteractionState::default();
        interaction.set_disabled(true);

        let node = widget_ax_node(AccessibilityRole::Button, Some("送信"), interaction);

        assert!(node.is_disabled());
    }

    #[test]
    fn widget_ax_node_is_not_disabled_by_default() {
        let node = widget_ax_node(
            AccessibilityRole::Button,
            Some("送信"),
            InteractionState::default(),
        );

        assert!(!node.is_disabled());
    }

    #[test]
    fn widget_ax_node_grants_focus_and_click_for_button() {
        let node = widget_ax_node(
            AccessibilityRole::Button,
            Some("送信"),
            InteractionState::default(),
        );

        assert!(node.supports_action(Action::Focus));
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn widget_ax_node_grants_focus_without_click_for_slider() {
        let node = widget_ax_node(
            AccessibilityRole::Slider,
            Some("音量"),
            InteractionState::default(),
        );

        assert!(node.supports_action(Action::Focus));
        assert!(!node.supports_action(Action::Click));
    }

    #[test]
    fn widget_ax_node_grants_no_actions_for_display_only_roles() {
        let node = widget_ax_node(
            AccessibilityRole::Label,
            Some("hello"),
            InteractionState::default(),
        );

        assert!(!node.supports_action(Action::Focus));
        assert!(!node.supports_action(Action::Click));
    }

    #[test]
    fn widget_ax_node_grants_focus_without_click_for_dialog_radio_group_and_list() {
        for role in [
            AccessibilityRole::Dialog,
            AccessibilityRole::RadioGroup,
            AccessibilityRole::List,
        ] {
            let node = widget_ax_node(role, Some("container"), InteractionState::default());

            assert!(
                node.supports_action(Action::Focus),
                "{role:?} には Action::Focus を付与する（AT のナビゲーション起点）"
            );
            assert!(
                !node.supports_action(Action::Click),
                "{role:?} はクリック相当の既定アクションを持たない"
            );
        }
    }
}
