//! 基本ウィジェット（Action/Input/Display カテゴリ）へのトークン適用配線
//! （TASK-8.4-S14、Issue #170）。
//!
//! 親 issue #156（TASK-8.4「デザイントークン基盤」）の分解 sub-issue。S1〜S13 が
//! `crate::token` に実装したスキーマ・解決エンジン・色/タイポグラフィ/スペーシング/
//! アニメーション値型と、TASK-8.3 が `ui::widget` に実装したウィジェット共通トレイト
//! （[`super::widget::Widget`]・[`super::widget::StateStyles`]）を橋渡しする
//! 中核モジュールである。
//!
//! - [`StyleTokenRefs`]: 1 状態分のスタイルスロット（背景・前景・タイポグラフィ）を
//!   `TokenRef` の束として表現する。[`super::widget::StateStyles`] を
//!   `T = StyleTokenRefs` でインスタンス化することで、状態別（Idle/Hovered/Pressed/
//!   Disabled）のトークン参照を表現できる。
//! - [`ResolvedWidgetStyle`]: 解決済み具体値（`ColorValue` / `TypographyValue`）の束。
//!   [`super::widget::PaintCommand::StyledRect`] / `StyledText` へそのまま渡せる形。
//! - [`resolve_widget_style`]: `StateStyles<StyleTokenRefs>` から `VisualState` に
//!   応じたスロットを選び、`TokenResolver` 経由で具体値まで解決する。
//! - `action_style_refs` / `input_style_refs` / `display_style_refs`: ウィジェット種別
//!   ごとの既定マッピング（セマンティックロール → スロットの構造のみを定義し、
//!   具体色値はテーマの `TokenSet`（S16、#172 以降のスコープ）が決める）。
//! - [`style_paint_commands`]: 無スタイルの `paint()` 出力（`Rect` / `Text` のみから
//!   成る列）を、解決済み具体値へ機械的に写像するヘルパー。個々の描画構造（矩形の
//!   個数・テキストの有無条件）は各ウィジェットの `paint()` が既に確定しているため、
//!   `paint_styled` は「同じ構造のまま色・タイポグラフィだけを差し替える」薄い
//!   オーバーライドに徹する（`Button` のみ `style` フィールドで種別を明示保持する
//!   例示実装とし、他の基本ウィジェットは本ヘルパー経由の軽量配線に留める設計判断、
//!   `.claude/rules/out-of-scope-tracking.md` 準拠で PR 本文に記録する）。
//!
//! 解決失敗（未定義キー・型不一致等）は `Err` を返し panic しない
//! （coding-rust.md の panic 禁止方針）。呼び出し元（各ウィジェットの
//! [`super::widget::Widget::paint_styled`] オーバーライド）は `Err` を
//! 無スタイル描画列へのフォールバックとして扱う契約とする。
//!
//! TASK-8.4-S19（#175）以降、内部の色・タイポグラフィ解決は
//! `crate::token::typed::TypedTokenRef` へ委譲し（`resolve_color` /
//! `resolve_typography` 参照）、重複していた型不一致検出ロジックを一本化した。
//! `StyleTokenRefs` の公開フィールド型（`Option<TokenRef>`）自体は破壊的変更を
//! 避けるため変更せず、`with_background_typed` 等の加法的な型付きビルダーを
//! 追加するに留める（型付きフィールド化は別 issue の申し送り事項）。

#[cfg(test)]
use crate::token::TokenValue;
use crate::token::{
    ColorTokenRef, ColorValue, SemanticColorRole, TokenError, TokenPath, TokenRef, TokenResolver,
    TypographyTokenRef, TypographyValue,
};

use super::widget::{PaintCommand, StateStyles, VisualState};

/// 1 状態分のスタイルスロット → トークン参照の束（TASK-8.4-S14 の核心）。
///
/// 各フィールドは `Option` とし、未指定スロットは「このウィジェットはこの見た目を
/// 持たない」ことを表す（例: `Label` は背景を持たない）。クロージャを取らない
/// （REQ-13 整合、`docs/architecture/design-token-architecture.md` §5.3）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleTokenRefs {
    /// 背景色トークン参照。
    pub background: Option<TokenRef>,
    /// 前景（文字・アイコン）色トークン参照。
    pub foreground: Option<TokenRef>,
    /// タイポグラフィトークン参照。
    pub typography: Option<TokenRef>,
}

impl StyleTokenRefs {
    /// 全スロット未設定の空の束を構築する。
    pub fn new() -> Self {
        Self::default()
    }

    /// 背景色トークン参照を設定する（ビルダー形式）。
    pub fn with_background(mut self, token_ref: TokenRef) -> Self {
        self.background = Some(token_ref);
        self
    }

    /// 前景色トークン参照を設定する（ビルダー形式）。
    pub fn with_foreground(mut self, token_ref: TokenRef) -> Self {
        self.foreground = Some(token_ref);
        self
    }

    /// タイポグラフィトークン参照を設定する（ビルダー形式）。
    pub fn with_typography(mut self, token_ref: TokenRef) -> Self {
        self.typography = Some(token_ref);
        self
    }

    /// 背景色トークン参照を型付き参照から設定する（TASK-8.4-S19、#175。
    /// [`Self::with_background`] の加法的な型安全版。フィールドの型自体は
    /// 無型 `Option<TokenRef>` のまま変更しない — 破壊的変更を避けるため
    /// `TypedTokenRef<ColorKind>` を内部で無型化して格納する。DSL 生成コードが
    /// カテゴリを型で保証したうえで渡せる入口として追加した）。
    pub fn with_background_typed(mut self, token_ref: ColorTokenRef) -> Self {
        self.background = Some(token_ref.into_ref());
        self
    }

    /// 前景色トークン参照を型付き参照から設定する（[`Self::with_background_typed`]
    /// と同型）。
    pub fn with_foreground_typed(mut self, token_ref: ColorTokenRef) -> Self {
        self.foreground = Some(token_ref.into_ref());
        self
    }

    /// タイポグラフィトークン参照を型付き参照から設定する
    /// （[`Self::with_background_typed`] と同型）。
    pub fn with_typography_typed(mut self, token_ref: TypographyTokenRef) -> Self {
        self.typography = Some(token_ref.into_ref());
        self
    }
}

/// 解決済みの具体値の束（TASK-8.4-S14）。
///
/// [`super::widget::PaintCommand::StyledRect`] / `StyledText` を組み立てる際の
/// 入力として使う。`TokenRef` を一切保持しない（描画バックエンドへトークンの存在を
/// 波及させない契約、設計文書 §5.4）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWidgetStyle {
    /// 解決済みの背景色。
    pub background: Option<ColorValue>,
    /// 解決済みの前景色。
    pub foreground: Option<ColorValue>,
    /// 解決済みのタイポグラフィ値。
    pub typography: Option<TypographyValue>,
}

/// 単一のトークン参照を色値として解決する。
///
/// TASK-8.4-S19（#175）以降は `token::typed::TypedTokenRef<ColorKind>` への
/// 一時的な持ち上げ（`ColorTokenRef::from_ref`）を経由して解決する薄いラッパーへ
/// 委譲しており、型不一致検出ロジックの実体は `typed.rs` の 1 箇所に集約されている
/// （`resolver.resolve` の結果が `TokenValue::Color` であることを検証し、不一致は
/// `TokenError::TypeMismatch` として報告する挙動自体は変更しない。
/// `semantic_color::resolve_semantic_color` と同じ安全網、`token/semantic_color.rs`
/// 参照）。
fn resolve_color(resolver: &TokenResolver, token_ref: &TokenRef) -> Result<ColorValue, TokenError> {
    ColorTokenRef::from_ref(token_ref.clone()).resolve(resolver)
}

/// 単一のトークン参照をタイポグラフィ値として解決する（[`resolve_color`] と同型。
/// TASK-8.4-S19（#175）以降は `TypographyTokenRef` への委譲）。
fn resolve_typography(
    resolver: &TokenResolver,
    token_ref: &TokenRef,
) -> Result<TypographyValue, TokenError> {
    TypographyTokenRef::from_ref(token_ref.clone()).resolve(resolver)
}

/// `StateStyles<StyleTokenRefs>` を `visual` で選択し、`resolver` 経由で解決済みの
/// 具体値の束へ変換する（TASK-8.4-S14 の核心 API）。
///
/// いずれかのスロットの解決に失敗した場合、最初に検出したエラーを返す
/// （呼び出し元は `Err` を無スタイルフォールバックとして扱う契約、モジュール doc 参照）。
///
/// # Errors
///
/// 設定済みのいずれかのスロットが未定義参照・型不一致・循環参照・深さ上限超過の
/// いずれかに該当する場合、`TokenResolver::resolve` が返すエラーをそのまま伝播する。
pub fn resolve_widget_style(
    resolver: &TokenResolver,
    styles: &StateStyles<StyleTokenRefs>,
    visual: VisualState,
) -> Result<ResolvedWidgetStyle, TokenError> {
    let refs = styles.resolve(visual);

    let background = refs
        .background
        .as_ref()
        .map(|token_ref| resolve_color(resolver, token_ref))
        .transpose()?;
    let foreground = refs
        .foreground
        .as_ref()
        .map(|token_ref| resolve_color(resolver, token_ref))
        .transpose()?;
    let typography = refs
        .typography
        .as_ref()
        .map(|token_ref| resolve_typography(resolver, token_ref))
        .transpose()?;

    Ok(ResolvedWidgetStyle {
        background,
        foreground,
        typography,
    })
}

/// 既定のタイポグラフィトークン参照（`typography.body`、`typography::typography_token_set`
/// が定義するパス、TASK-8.4-S8/S9 参照）。
fn body_typography_ref() -> Result<TokenRef, TokenError> {
    Ok(TokenRef::new(TokenPath::from_dotted("typography.body")?))
}

/// Action カテゴリ（`Button` 等）の既定スタイルマッピングを構築する。
///
/// 背景 = `Primary`、前景 = `OnPrimary`、タイポグラフィ = `typography.body`。
/// Disabled はセマンティックロールを `Surface`/`OnSurface`（減衰表現）へ切り替える
/// 構造のみを定義し、実際の減衰量（不透明度調整等）の最終調整は S16（#172）に委ねる。
///
/// # Errors
///
/// セマンティックロールのパス生成・`typography.body` のパス生成が失敗した場合に
/// 伝播する（閉じた列挙・固定リテラルのみから構築するため通常は起こらない）。
pub fn action_style_refs() -> Result<StateStyles<StyleTokenRefs>, TokenError> {
    let typography = body_typography_ref()?;
    let base = StyleTokenRefs::new()
        .with_background(SemanticColorRole::Primary.token_ref()?)
        .with_foreground(SemanticColorRole::OnPrimary.token_ref()?)
        .with_typography(typography.clone());
    let disabled = StyleTokenRefs::new()
        .with_background(SemanticColorRole::Surface.token_ref()?)
        .with_foreground(SemanticColorRole::OnSurface.token_ref()?)
        .with_typography(typography);
    Ok(StateStyles::new(base).with_disabled(disabled))
}

/// Input カテゴリ（`Checkbox` / `Slider` / `TextField` 等）の既定スタイルマッピングを
/// 構築する。
///
/// 背景 = `Surface`、前景（チェックマーク・フィル等のアクセント） = `Primary`、
/// タイポグラフィ = `typography.body`。
///
/// # Errors
///
/// [`action_style_refs`] と同じ理由により通常は起こらない。
pub fn input_style_refs() -> Result<StateStyles<StyleTokenRefs>, TokenError> {
    let typography = body_typography_ref()?;
    let base = StyleTokenRefs::new()
        .with_background(SemanticColorRole::Surface.token_ref()?)
        .with_foreground(SemanticColorRole::Primary.token_ref()?)
        .with_typography(typography.clone());
    let disabled = StyleTokenRefs::new()
        .with_background(SemanticColorRole::Surface.token_ref()?)
        .with_foreground(SemanticColorRole::OnSurface.token_ref()?)
        .with_typography(typography);
    Ok(StateStyles::new(base).with_disabled(disabled))
}

/// Display カテゴリ（`Label` / `Icon` / `Image` / `ProgressBar` 等）の既定スタイル
/// マッピングを構築する。
///
/// 背景 = `Surface`（`Label` のようにテキストのみを描画するウィジェットは
/// `paint()` が `Rect` を持たないため単純に未使用になるが、`Icon` / `Image` /
/// `ProgressBar` 等のプレースホルダ枠・トラックの塗り色として使われる。前景色を
/// `Rect` の塗りへ流用してしまう `style_paint_commands` のフォールバックを避ける
/// ための明示スロットであり、`action_style_refs`/`input_style_refs` と同じ
/// 「背景と前景を必ず区別する」構造に揃える）、前景 = `OnBackground`、
/// タイポグラフィ = `typography.body`。
///
/// # Errors
///
/// [`action_style_refs`] と同じ理由により通常は起こらない。
pub fn display_style_refs() -> Result<StateStyles<StyleTokenRefs>, TokenError> {
    let typography = body_typography_ref()?;
    let base = StyleTokenRefs::new()
        .with_background(SemanticColorRole::Surface.token_ref()?)
        .with_foreground(SemanticColorRole::OnBackground.token_ref()?)
        .with_typography(typography.clone());
    let disabled = StyleTokenRefs::new()
        .with_background(SemanticColorRole::Surface.token_ref()?)
        .with_foreground(SemanticColorRole::OnSurface.token_ref()?)
        .with_typography(typography);
    Ok(StateStyles::new(base).with_disabled(disabled))
}

/// 無スタイルの `paint()` 出力を解決済み具体値へ写像する（TASK-8.4-S14）。
///
/// `PaintCommand::Rect` は `background`（未設定なら `foreground`、双方 `None` なら
/// 写像不能）を塗り色として `StyledRect` へ、`PaintCommand::Text(content)` は
/// `foreground` + `typography`（いずれかが `None` なら写像不能）を伴う `StyledText`
/// へ変換する。`resolved` が必要なスロットを持たない場合は `None` を返し、
/// 呼び出し元（各ウィジェットの `paint_styled`）が無スタイル列へフォールバックする
/// 契約とする（panic しない、coding-rust.md 準拠）。列の構造（要素数・順序）自体は
/// 変更しないため、既存 `paint()` の完全一致テストはそのまま温存される。
pub fn style_paint_commands(
    plain: Vec<PaintCommand>,
    resolved: &ResolvedWidgetStyle,
) -> Option<Vec<PaintCommand>> {
    let rect_fill = resolved.background.or(resolved.foreground);
    let mut styled = Vec::with_capacity(plain.len());
    for command in plain {
        match command {
            PaintCommand::Rect => {
                let fill = rect_fill?;
                styled.push(PaintCommand::StyledRect { fill });
            }
            PaintCommand::Text(content) => {
                let color = resolved.foreground?;
                let typography = resolved.typography.clone()?;
                styled.push(PaintCommand::StyledText {
                    content,
                    color,
                    typography,
                });
            }
            // StyledRect/StyledText は既に解決済みのため素通しし、二重変換しない。
            other => styled.push(other),
        }
    }
    Some(styled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{
        SemanticColorMapping, TokenDefinition, TokenSet, palette, palette_token_set,
        typography_token_set,
    };

    /// テスト用の完結した `TokenResolver` を構築する（パレット + セマンティック
    /// マッピング + タイポグラフィスケールを 1 つの `TokenSet` にマージする）。
    fn test_resolver() -> TokenResolver {
        let mut base = palette_token_set().expect("パレット構築は成功する");
        base.extend(typography_token_set().expect("タイポグラフィ構築は成功する"));

        SemanticColorMapping::new()
            .with(
                SemanticColorRole::Primary,
                TokenRef::new(TokenPath::from_dotted("color.palette.blue-500").unwrap()),
            )
            .with(
                SemanticColorRole::OnPrimary,
                TokenRef::new(TokenPath::from_dotted("color.palette.white").unwrap()),
            )
            .with(
                SemanticColorRole::Surface,
                TokenRef::new(TokenPath::from_dotted("color.palette.gray-100").unwrap()),
            )
            .with(
                SemanticColorRole::OnSurface,
                TokenRef::new(TokenPath::from_dotted("color.palette.gray-700").unwrap()),
            )
            .with(
                SemanticColorRole::Background,
                TokenRef::new(TokenPath::from_dotted("color.palette.white").unwrap()),
            )
            .with(
                SemanticColorRole::OnBackground,
                TokenRef::new(TokenPath::from_dotted("color.palette.gray-900").unwrap()),
            )
            .merge_into(&mut base)
            .expect("セマンティックマッピングをマージできる");

        TokenResolver::new(base, TokenSet::new())
    }

    #[test]
    fn action_style_refs_builds_without_error() {
        action_style_refs().expect("action マッピングの構築は成功する");
    }

    #[test]
    fn input_style_refs_builds_without_error() {
        input_style_refs().expect("input マッピングの構築は成功する");
    }

    #[test]
    fn display_style_refs_builds_without_error() {
        display_style_refs().expect("display マッピングの構築は成功する");
    }

    #[test]
    fn resolve_widget_style_action_idle_resolves_primary_background() {
        let resolver = test_resolver();
        let styles = action_style_refs().expect("構築は成功する");

        let resolved = resolve_widget_style(&resolver, &styles, VisualState::Idle)
            .expect("Idle 状態は解決できる");

        assert_eq!(resolved.background, Some(palette::BLUE_500));
        assert_eq!(resolved.foreground, Some(palette::WHITE));
        assert!(resolved.typography.is_some());
    }

    #[test]
    fn resolve_widget_style_action_disabled_switches_to_surface_role() {
        let resolver = test_resolver();
        let styles = action_style_refs().expect("構築は成功する");

        let resolved = resolve_widget_style(&resolver, &styles, VisualState::Disabled)
            .expect("Disabled 状態は解決できる");

        assert_eq!(resolved.background, Some(palette::GRAY_100));
        assert_eq!(resolved.foreground, Some(palette::GRAY_700));
    }

    #[test]
    fn resolve_widget_style_hovered_falls_back_to_base_when_no_override() {
        let resolver = test_resolver();
        let styles = action_style_refs().expect("構築は成功する");

        // Hovered/Pressed のオーバーライドは未設定のため base（Idle 相当）へ
        // フォールバックする（StateStyles::resolve の既存契約）。
        let resolved = resolve_widget_style(&resolver, &styles, VisualState::Hovered)
            .expect("Hovered 状態は base へフォールバックして解決できる");

        assert_eq!(resolved.background, Some(palette::BLUE_500));
    }

    #[test]
    fn resolve_widget_style_display_resolves_surface_background_and_on_background_foreground() {
        let resolver = test_resolver();
        let styles = display_style_refs().expect("構築は成功する");

        let resolved = resolve_widget_style(&resolver, &styles, VisualState::Idle)
            .expect("Idle 状態は解決できる");

        assert_eq!(resolved.background, Some(palette::GRAY_100));
        assert_eq!(resolved.foreground, Some(palette::GRAY_900));
    }

    #[test]
    fn resolve_widget_style_reports_error_for_undefined_role() {
        // セマンティックロールが未登録のリゾルバでは UndefinedReference を返す。
        let base = palette_token_set().expect("パレット構築は成功する");
        let resolver = TokenResolver::new(base, TokenSet::new());
        let styles = action_style_refs().expect("構築は成功する");

        let err = resolve_widget_style(&resolver, &styles, VisualState::Idle)
            .expect_err("セマンティックロール未登録は解決に失敗する");
        assert!(matches!(err, TokenError::UndefinedReference { .. }));
    }

    #[test]
    fn resolve_widget_style_reports_type_mismatch_for_wrong_category() {
        // foreground スロットが誤って spacing カテゴリを指す場合、TypeMismatch になる。
        let mut base = palette_token_set().expect("パレット構築は成功する");
        base.insert(
            TokenPath::from_dotted("spacing.md").unwrap(),
            TokenDefinition::Value {
                value: TokenValue::Dimension(8.0),
            },
        );
        let resolver = TokenResolver::new(base, TokenSet::new());

        let styles = StateStyles::new(
            StyleTokenRefs::new()
                .with_foreground(TokenRef::new(TokenPath::from_dotted("spacing.md").unwrap())),
        );

        let err = resolve_widget_style(&resolver, &styles, VisualState::Idle)
            .expect_err("color 以外のカテゴリを参照した場合は型不一致になる");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }

    #[test]
    fn style_paint_commands_maps_rect_and_text_preserving_structure() {
        let plain = vec![PaintCommand::Rect, PaintCommand::Text("送信".to_string())];
        let resolved = ResolvedWidgetStyle {
            background: Some(palette::BLUE_500),
            foreground: Some(palette::WHITE),
            typography: Some(
                typography_token_set()
                    .unwrap()
                    .get(&TokenPath::from_dotted("typography.body").unwrap())
                    .and_then(|def| match def {
                        TokenDefinition::Value {
                            value: TokenValue::Typography(t),
                        } => Some(t.clone()),
                        _ => None,
                    })
                    .expect("typography.body は存在する"),
            ),
        };

        let styled =
            style_paint_commands(plain, &resolved).expect("必要なスロットが揃っているため成功する");

        assert_eq!(
            styled,
            vec![
                PaintCommand::StyledRect {
                    fill: palette::BLUE_500
                },
                PaintCommand::StyledText {
                    content: "送信".to_string(),
                    color: palette::WHITE,
                    typography: resolved.typography.unwrap(),
                }
            ]
        );
    }

    #[test]
    fn style_paint_commands_rect_falls_back_to_foreground_when_no_background() {
        let plain = vec![PaintCommand::Rect];
        let resolved = ResolvedWidgetStyle {
            background: None,
            foreground: Some(palette::GRAY_900),
            typography: None,
        };

        let styled = style_paint_commands(plain, &resolved)
            .expect("foreground を rect fill のフォールバックに使えるため成功する");

        assert_eq!(
            styled,
            vec![PaintCommand::StyledRect {
                fill: palette::GRAY_900
            }]
        );
    }

    #[test]
    fn style_paint_commands_returns_none_when_no_fill_available_for_rect() {
        let plain = vec![PaintCommand::Rect];
        let resolved = ResolvedWidgetStyle {
            background: None,
            foreground: None,
            typography: None,
        };

        assert_eq!(style_paint_commands(plain, &resolved), None);
    }

    #[test]
    fn style_paint_commands_returns_none_when_typography_missing_for_text() {
        let plain = vec![PaintCommand::Text("ラベル".to_string())];
        let resolved = ResolvedWidgetStyle {
            background: None,
            foreground: Some(palette::GRAY_900),
            typography: None,
        };

        assert_eq!(style_paint_commands(plain, &resolved), None);
    }

    #[test]
    fn style_token_refs_default_has_all_slots_unset() {
        let refs = StyleTokenRefs::new();
        assert!(refs.background.is_none());
        assert!(refs.foreground.is_none());
        assert!(refs.typography.is_none());
    }

    #[test]
    fn with_typed_builders_populate_same_slots_as_untyped_builders() {
        // TASK-8.4-S19（#175）: with_*_typed が既存の with_* と同一の TokenRef を
        // 格納すること（フィールド型 Option<TokenRef> 自体は変更しない契約）を確認する。
        let color_ref = SemanticColorRole::Primary
            .typed_ref()
            .expect("型付き参照の生成に成功する");
        let raw_color_ref = SemanticColorRole::Primary
            .token_ref()
            .expect("無型参照の生成に成功する");
        let typography_ref: crate::token::TypographyTokenRef =
            crate::token::TypedTokenRef::from_dotted("typography.body").expect("正当なパス");
        let raw_typography_ref =
            TokenRef::new(TokenPath::from_dotted("typography.body").expect("正当なパス"));

        let refs = StyleTokenRefs::new()
            .with_background_typed(color_ref.clone())
            .with_foreground_typed(color_ref)
            .with_typography_typed(typography_ref);

        assert_eq!(refs.background, Some(raw_color_ref.clone()));
        assert_eq!(refs.foreground, Some(raw_color_ref));
        assert_eq!(refs.typography, Some(raw_typography_ref));
    }
}
