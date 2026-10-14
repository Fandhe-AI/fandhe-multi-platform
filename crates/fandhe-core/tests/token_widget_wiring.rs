//! 結合テスト: デザイントークン基盤（`fandhe_core::token`）と基本ウィジェット
//! （`fandhe_core::ui`）の配線（TASK-8.4-S14、Issue #170）を、クレート外部の
//! 公開 API のみを使って検証する。
//!
//! ローカルで組み立てた `TokenSet`（パレット + セマンティックマッピング +
//! タイポグラフィスケール）を `TokenResolver` へ渡し、代表ウィジェット
//! （`Button` / `CheckboxWidget` / `LabelWidget`）の `paint_styled` が
//! - 解決済み具体値（`ColorValue` / `TypographyValue`）を運ぶこと
//! - `set_theme_overrides` によるテーマ差分適用で解決色が変わること
//! - 未定義キー時に無スタイル描画列へフォールバックすること
//!
//! の 3 観点を確認する（実装計画 §4 記載の結合テスト観点）。

use fandhe_core::{
    Button, CheckboxWidget, ColorValue, LabelWidget, Measurement, PaintCommand,
    SemanticColorMapping, SemanticColorRole, TokenPath, TokenRef, TokenResolver, TokenSet, Widget,
    WidgetId, palette, palette_token_set, typography_token_set,
};

/// テスト用の空状態型（各ウィジェットはトークン解決の検証にのみ使うため、
/// アプリ状態そのものは参照しない）。
struct AppState;

fn ref_to(path: &str) -> TokenRef {
    TokenRef::new(TokenPath::from_dotted(path).expect("テスト用の正当なパス"))
}

/// パレット + セマンティックマッピング + タイポグラフィスケールを束ねた基底 `TokenSet`
/// を構築する（本テストが実装計画 §4 で要求する「ローカルで組み立てた TokenSet」）。
fn base_token_set(primary: ColorValue, on_primary: ColorValue) -> TokenSet {
    let mut base = palette_token_set().expect("パレット構築は成功する");
    base.extend(typography_token_set().expect("タイポグラフィ構築は成功する"));

    // パレットに存在しない架空のブランド色をテーマ切替の検証用に追加する。
    base.insert(
        TokenPath::from_dotted("color.palette.brand-a").expect("正当なパス"),
        fandhe_core::TokenDefinition::Value {
            value: fandhe_core::TokenValue::Color(primary),
        },
    );
    base.insert(
        TokenPath::from_dotted("color.palette.brand-a-on").expect("正当なパス"),
        fandhe_core::TokenDefinition::Value {
            value: fandhe_core::TokenValue::Color(on_primary),
        },
    );

    SemanticColorMapping::new()
        .with(SemanticColorRole::Primary, ref_to("color.palette.brand-a"))
        .with(
            SemanticColorRole::OnPrimary,
            ref_to("color.palette.brand-a-on"),
        )
        .with(SemanticColorRole::Surface, ref_to("color.palette.gray-100"))
        .with(
            SemanticColorRole::OnSurface,
            ref_to("color.palette.gray-700"),
        )
        .with(SemanticColorRole::Background, ref_to("color.palette.white"))
        .with(
            SemanticColorRole::OnBackground,
            ref_to("color.palette.gray-900"),
        )
        .merge_into(&mut base)
        .expect("セマンティックマッピングをマージできる");

    base
}

fn increment(_state: &mut AppState) {}

#[test]
fn button_paint_styled_carries_resolved_concrete_values() {
    let base = base_token_set(palette::BLUE_500, palette::WHITE);
    let resolver = TokenResolver::new(base, TokenSet::new());

    let button: Button<AppState> = Button::new(WidgetId::from_raw(1), "送信", increment);
    let commands = button.paint_styled(
        (0.0, 0.0),
        Measurement {
            width: 100.0,
            height: 32.0,
        },
        &resolver,
    );

    assert_eq!(
        commands,
        vec![
            PaintCommand::StyledRect {
                fill: palette::BLUE_500
            },
            match &commands[1] {
                PaintCommand::StyledText { typography, .. } => PaintCommand::StyledText {
                    content: "送信".to_string(),
                    color: palette::WHITE,
                    typography: typography.clone(),
                },
                other => panic!("StyledText を期待: {other:?}"),
            }
        ]
    );
}

#[test]
fn checkbox_widget_paint_styled_carries_resolved_concrete_values() {
    let base = base_token_set(palette::BLUE_500, palette::WHITE);
    let resolver = TokenResolver::new(base, TokenSet::new());

    let checkbox: CheckboxWidget<AppState> =
        CheckboxWidget::new_static(WidgetId::from_raw(2), "agree", true);
    let commands = checkbox.paint_styled(
        (0.0, 0.0),
        Measurement {
            width: 100.0,
            height: 20.0,
        },
        &resolver,
    );

    // `CheckboxWidget::paint` は `[Rect（ボックス）, Text("[x] label")]` の構造
    // （チェックマークとラベルは 1 つの Text にまとめて表現する、`checkbox.rs`
    // モジュール doc 参照）を保ったまま、全要素が解決済み具体値
    // （StyledRect/StyledText）へ変換されていること。
    assert_eq!(commands.len(), 2);
    assert!(matches!(commands[0], PaintCommand::StyledRect { .. }));
    assert!(matches!(commands[1], PaintCommand::StyledText { .. }));
}

#[test]
fn label_widget_paint_styled_carries_resolved_typography() {
    let base = base_token_set(palette::BLUE_500, palette::WHITE);
    let resolver = TokenResolver::new(base, TokenSet::new());

    let label: LabelWidget<AppState> = LabelWidget::new_static(WidgetId::from_raw(3), "ラベル");
    let commands = label.paint_styled(
        (0.0, 0.0),
        Measurement {
            width: 100.0,
            height: 20.0,
        },
        &resolver,
    );

    match &commands[..] {
        [
            PaintCommand::StyledText {
                content,
                color,
                typography,
            },
        ] => {
            assert_eq!(content, "ラベル");
            assert_eq!(*color, palette::GRAY_900);
            assert_eq!(typography.family(), fandhe_core::family::DEFAULT);
        }
        other => panic!("単一の StyledText を期待: {other:?}"),
    }
}

#[test]
fn theme_override_changes_resolved_button_background() {
    // テーマ切替（S5、#161 のスコープ）契約: `set_theme_overrides` で Primary の
    // 参照先を差し替えると、同じウィジェットの `paint_styled` 出力が追従すること。
    let base = base_token_set(palette::BLUE_500, palette::WHITE);
    let mut resolver = TokenResolver::new(base, TokenSet::new());

    let button: Button<AppState> = Button::new(WidgetId::from_raw(4), "送信", increment);
    let size = Measurement {
        width: 100.0,
        height: 32.0,
    };

    let light = button.paint_styled((0.0, 0.0), size, &resolver);
    assert_eq!(
        light[0],
        PaintCommand::StyledRect {
            fill: palette::BLUE_500
        }
    );

    // ダークテーマ差分: Primary を別パレット色（RED_900）へ差し替える。
    let mut dark_overrides = TokenSet::new();
    dark_overrides.insert(
        TokenPath::from_dotted("color.primary").expect("正当なパス"),
        fandhe_core::TokenDefinition::Reference {
            category: fandhe_core::TokenCategory::Color,
            reference: ref_to("color.palette.red-900"),
        },
    );
    resolver.set_theme_overrides(dark_overrides);

    let dark = button.paint_styled((0.0, 0.0), size, &resolver);
    assert_eq!(
        dark[0],
        PaintCommand::StyledRect {
            fill: palette::RED_900
        }
    );
    assert_ne!(light[0], dark[0]);
}

#[test]
fn undefined_semantic_role_falls_back_to_unstyled_paint() {
    // セマンティックロール（Primary 等）が一切登録されていないリゾルバでは、
    // `paint_styled` が `paint()` と同一の無スタイル列へフォールバックすること
    // （未定義キー時の安全側フォールバック契約、panic しない）。
    let empty_resolver = TokenResolver::new(TokenSet::new(), TokenSet::new());

    let button: Button<AppState> = Button::new(WidgetId::from_raw(5), "送信", increment);
    let size = Measurement {
        width: 100.0,
        height: 32.0,
    };

    let styled = button.paint_styled((0.0, 0.0), size, &empty_resolver);
    let plain = button.paint((0.0, 0.0), size);
    assert_eq!(styled, plain);
    assert_eq!(
        plain,
        vec![PaintCommand::Rect, PaintCommand::Text("送信".to_string())]
    );
}
