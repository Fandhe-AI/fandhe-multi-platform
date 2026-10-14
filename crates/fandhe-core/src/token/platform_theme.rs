//! プラットフォーム別既定テーマ（iOS HIG / Material Design 準拠値、
//! TASK-8.4-S16、#172）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§1 の
//! 分担表・§7 は「プラットフォーム別既定テーマの具体値（iOS HIG / Material
//! Design 準拠値）は S16 のスコープ」と定める。`docs/architecture/theme-switching-architecture.md`
//! §5 も同様に「`Theme` 構造に iOS/Android 準拠の具体値を載せる」を S16（#172）に
//! 委ねている。S6〜S13（色パレット・タイポグラフィ・スペーシング・サイズ/角丸/
//! 高さ・ボーダー/シャドウ・アニメーション/duration の各既定値）はすべて
//! フレームワーク中立の値に留められており、本モジュールがその上にプラットフォーム
//! 準拠の具体値を後勝ちで重ねる「合成レイヤ」を担う。
//!
//! ## 表現形式（テーマ切替 S5・#161 との契約）
//!
//! `TokenResolver::new(base, theme_overrides)` の「ベース → テーマ差分 →
//! エイリアス解決」の 3 段階契約（`token-resolution-architecture.md` §3.3）に
//! 合わせ、次の 2 関数でプラットフォーム別既定テーマを表現する。
//!
//! - [`platform_base_token_set`][]: 中立の各カテゴリ既定値（`color::palette_token_set`
//!   等）を合成した上に、プラットフォーム準拠のパレット・セマンティック参照・
//!   タイポグラフィ・radius・duration を後勝ちで重ねた完全な `TokenSet`。
//!   `TokenResolver::new` の `base` 引数にそのまま渡せる。
//! - [`platform_dark_overrides`][]: パレット値の上書きのみを含む最小の
//!   `TokenSet`（`theme-switching-architecture.md` §2.3「ダークテーマがベースの
//!   全キーを再定義することを強制しない」との整合。ライト/ダークで値が変わらない
//!   キー（例: iOS の `secondary` / `on-primary`）は意図的に含めず、resolver の
//!   フォールバック意味論に委ねる）。`TokenResolver::new` の `theme_overrides`
//!   引数、または `set_theme_overrides` に渡す想定。
//!
//! セマンティックカラー（`color.primary` 等）は常にプラットフォームパレット
//! （`color.palette.ios-blue` 等）への参照（エイリアス）とし、ライト/ダークの
//! 切替はパレット側の値上書きのみで表現する（iOS の dynamic color / Material の
//! tonal palette の意味論と一致し、resolver の「テーマ差分適用 → エイリアス解決」
//! の順序をそのまま活用できる）。
//!
//! ## `ThemeVariant` / `Theme` との統合（S5、#161）
//!
//! `platform_base_token_set` / `platform_dark_overrides` は関数名でライト/ダークを
//! 区別する低レベル API である（既存呼び出し元との後方互換維持）。[`platform_theme`]
//! 関数が、これら 2 関数と `theme::ThemeVariant` / `theme::Theme` との統合窓口
//! （ブリッジ）を提供する:
//! `ThemeVariant::Light` は空差分の `Theme`、`ThemeVariant::Dark` は
//! `platform_dark_overrides` の差分を持つ `Theme` を返す。
//!
//! ## 呼び出し文脈
//!
//! S14/S15（ウィジェット・レイアウトへのトークン適用配線、#170/#171）が
//! `platform_base_token_set` / `platform_dark_overrides` の戻り値を
//! `TokenResolver` へ渡す入口として利用し、S19（AI 型安全 DSL からのトークン
//! 参照 API、#175）が型付き参照の対象として `ThemePlatform` を扱う想定。
//! [`platform_theme`] は `TokenResolver::with_themes` へそのまま渡せる
//! `Theme` を返すため、`ui::theme_demo`（S23、#179）の `demo_resolver` が
//! この経路で `TokenResolver` を構築する。`ThemePlatform` の選択自体は
//! プラットフォーム統合レイヤ・アプリコードの責務であり、本クレートは OS 検出を
//! 行わない（REQ-2 の一方向依存、security.md）。
//!
//! ## スコープ外
//!
//! - elevation × shadow のプラットフォーム別合成表現（`shadow.rs` が言及する
//!   残余分）は対象外
//! - 外部ファイルからのテーマ読み込み（S17、#173）
//! - テーマ切替・適用の網羅テスト（S22、#178）

use super::animation::DurationValue;
use super::animation::duration_token_set;
use super::animation::{AnimationValue, presets as animation_presets};
use super::border::border_token_set;
use super::color::{ColorValue, palette, palette_token_set};
use super::dimension::{DimensionValue, elevation_token_set, radius_token_set, size_token_set};
use super::error::TokenError;
use super::schema::{TokenDefinition, TokenPath, TokenSet, TokenValue};
use super::semantic_color::{SemanticColorMapping, SemanticColorRole};
use super::shadow::shadow_token_set;
use super::spacing::spacing_token_set;
use super::theme::{Theme, ThemeVariant};
use super::typography::{FontStyleValue, FontWeightValue, TypographyValue, typography_token_set};

/// 既定テーマの対象プラットフォーム（閉じた列挙）。
///
/// どのプラットフォームを選ぶかはプラットフォーム統合レイヤ／アプリコードの
/// 責務であり、コアは OS 検出を行わない（REQ-2 の一方向依存、モジュール doc
/// 参照）。文字列でプラットフォームを指定する公開 API は設けず、閉じた列挙の
/// みを経路とする（OWASP A03 対策、security.md）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemePlatform {
    /// iOS（Apple Human Interface Guidelines 準拠）。
    Ios,
    /// Android（Material Design 3 baseline scheme 準拠）。
    Android,
}

/// タイポグラフィ既定値の行間比率（`typography.rs` の
/// `DEFAULT_LINE_HEIGHT_RATIO` と同じ一般的な本文組版の目安値）。
///
/// `typography.rs` 側の定数は非公開のため、本モジュールで独立して定義する
/// （プラットフォーム別サイズ上書き時の行間算出にのみ用いる中立係数）。
const PLATFORM_LINE_HEIGHT_RATIO: f32 = 1.4;

/// iOS（Apple Human Interface Guidelines）準拠の具体値定数群。
///
/// すべてコンパイル時定数リテラルのみで構成し、外部入力を一切受け取らない
/// （OWASP A03 対策）。値は公開ガイドライン（Apple HIG「Color」「Typography」）
/// 記載の値を採用し、乖離が判明した場合は値表とあわせて更新すること。
pub mod ios {
    use super::ColorValue;

    /// `color.primary` が参照するパレット色（ライト）。
    pub const BLUE_LIGHT: ColorValue = ColorValue::rgb(0x00, 0x7a, 0xff);
    /// `color.primary` が参照するパレット色（ダーク）。
    pub const BLUE_DARK: ColorValue = ColorValue::rgb(0x0a, 0x84, 0xff);
    /// `color.secondary` が参照するパレット色（ライト/ダーク共通）。
    pub const GRAY: ColorValue = ColorValue::rgb(0x8e, 0x8e, 0x93);
    /// `color.background` が参照するパレット色（ライト）。
    pub const BACKGROUND_LIGHT: ColorValue = ColorValue::rgb(0xff, 0xff, 0xff);
    /// `color.background` が参照するパレット色（ダーク）。
    pub const BACKGROUND_DARK: ColorValue = ColorValue::rgb(0x00, 0x00, 0x00);
    /// `color.surface` が参照するパレット色（ライト）。
    pub const SURFACE_LIGHT: ColorValue = ColorValue::rgb(0xf2, 0xf2, 0xf7);
    /// `color.surface` が参照するパレット色（ダーク）。
    pub const SURFACE_DARK: ColorValue = ColorValue::rgb(0x1c, 0x1c, 0x1e);
    /// `color.error` が参照するパレット色（ライト）。
    pub const RED_LIGHT: ColorValue = ColorValue::rgb(0xff, 0x3b, 0x30);
    /// `color.error` が参照するパレット色（ダーク）。
    pub const RED_DARK: ColorValue = ColorValue::rgb(0xff, 0x45, 0x3a);
    /// `color.warning` が参照するパレット色（ライト）。
    pub const ORANGE_LIGHT: ColorValue = ColorValue::rgb(0xff, 0x95, 0x00);
    /// `color.warning` が参照するパレット色（ダーク）。
    pub const ORANGE_DARK: ColorValue = ColorValue::rgb(0xff, 0x9f, 0x0a);
    /// `color.success` が参照するパレット色（ライト）。
    pub const GREEN_LIGHT: ColorValue = ColorValue::rgb(0x34, 0xc7, 0x59);
    /// `color.success` が参照するパレット色（ダーク）。
    pub const GREEN_DARK: ColorValue = ColorValue::rgb(0x30, 0xd1, 0x58);
    /// `color.on-primary` が参照するパレット色（ライト/ダーク共通）。
    pub const ON_ACCENT: ColorValue = ColorValue::rgb(0xff, 0xff, 0xff);
    /// `color.on-background` / `color.on-surface` が参照するパレット色（ライト）。
    pub const LABEL_LIGHT: ColorValue = ColorValue::rgb(0x00, 0x00, 0x00);
    /// `color.on-background` / `color.on-surface` が参照するパレット色（ダーク）。
    pub const LABEL_DARK: ColorValue = ColorValue::rgb(0xff, 0xff, 0xff);

    /// タイポグラフィの論理フォントファミリ名（実フォント解決はプラットフォーム
    /// レイヤの責務、`typography::family` の前例に従う）。
    pub const FONT_FAMILY: &str = "SF Pro";

    /// タイポグラフィスケールの具体サイズ（Apple HIG の Large Title / Title1 /
    /// Title2 / Body / Footnote / Caption1 相当）。
    pub mod typography_scale {
        /// `typography.display`（Large Title 相当）。
        pub const DISPLAY: f32 = 34.0;
        /// `typography.headline`（Title1 相当）。
        pub const HEADLINE: f32 = 28.0;
        /// `typography.title`（Title2 相当）。
        pub const TITLE: f32 = 22.0;
        /// `typography.body`。
        pub const BODY: f32 = 17.0;
        /// `typography.label`。
        pub const LABEL: f32 = 13.0;
        /// `typography.caption`。
        pub const CAPTION: f32 = 12.0;
    }

    /// `radius.md` の上書き値（iOS 標準角丸相当）。
    pub const RADIUS_MD: f32 = 10.0;
}

/// Android（Material Design 3 baseline scheme）準拠の具体値定数群。
///
/// iOS と同じ理由（OWASP A03 対策）で外部入力を受け取らない定数群のみとする。
pub mod android {
    use super::ColorValue;

    /// `color.primary` が参照するパレット色（ライト）。
    pub const PRIMARY_LIGHT: ColorValue = ColorValue::rgb(0x67, 0x50, 0xa4);
    /// `color.primary` が参照するパレット色（ダーク）。
    pub const PRIMARY_DARK: ColorValue = ColorValue::rgb(0xd0, 0xbc, 0xff);
    /// `color.secondary` が参照するパレット色（ライト）。
    pub const SECONDARY_LIGHT: ColorValue = ColorValue::rgb(0x62, 0x5b, 0x71);
    /// `color.secondary` が参照するパレット色（ダーク）。
    pub const SECONDARY_DARK: ColorValue = ColorValue::rgb(0xcc, 0xc2, 0xdc);
    /// `color.background` が参照するパレット色（ライト）。
    pub const BACKGROUND_LIGHT: ColorValue = ColorValue::rgb(0xff, 0xfb, 0xfe);
    /// `color.background` が参照するパレット色（ダーク）。
    pub const BACKGROUND_DARK: ColorValue = ColorValue::rgb(0x1c, 0x1b, 0x1f);
    /// `color.surface` が参照するパレット色（ライト、Material 3 baseline では
    /// background と同値）。
    pub const SURFACE_LIGHT: ColorValue = ColorValue::rgb(0xff, 0xfb, 0xfe);
    /// `color.surface` が参照するパレット色（ダーク、Material 3 baseline では
    /// background と同値）。
    pub const SURFACE_DARK: ColorValue = ColorValue::rgb(0x1c, 0x1b, 0x1f);
    /// `color.error` が参照するパレット色（ライト）。
    pub const ERROR_LIGHT: ColorValue = ColorValue::rgb(0xb3, 0x26, 0x1e);
    /// `color.error` が参照するパレット色（ダーク）。
    pub const ERROR_DARK: ColorValue = ColorValue::rgb(0xf2, 0xb8, 0xb5);
    /// `color.warning` が参照するパレット色（ライト）。
    ///
    /// Material 3 baseline には warning ロールが存在しないため、既存の中立
    /// パレット（`color::palette::YELLOW_600`）から採る妥協値
    /// （設計文書 §3.3 に明記した判断）。
    pub const WARNING_LIGHT: ColorValue = super::palette::YELLOW_600;
    /// `color.warning` が参照するパレット色（ダーク、同じく妥協値）。
    pub const WARNING_DARK: ColorValue = super::palette::YELLOW_400;
    /// `color.success` が参照するパレット色（ライト）。
    ///
    /// Material 3 baseline には success ロールが存在しないため、既存の中立
    /// パレット（`color::palette::GREEN_600`）から採る妥協値。
    pub const SUCCESS_LIGHT: ColorValue = super::palette::GREEN_600;
    /// `color.success` が参照するパレット色（ダーク、同じく妥協値）。
    pub const SUCCESS_DARK: ColorValue = super::palette::GREEN_400;
    /// `color.on-primary` が参照するパレット色（ライト）。
    pub const ON_PRIMARY_LIGHT: ColorValue = ColorValue::rgb(0xff, 0xff, 0xff);
    /// `color.on-primary` が参照するパレット色（ダーク）。
    pub const ON_PRIMARY_DARK: ColorValue = ColorValue::rgb(0x38, 0x1e, 0x72);
    /// `color.on-background` / `color.on-surface` が参照するパレット色（ライト）。
    pub const ON_SURFACE_LIGHT: ColorValue = ColorValue::rgb(0x1c, 0x1b, 0x1f);
    /// `color.on-background` / `color.on-surface` が参照するパレット色（ダーク）。
    pub const ON_SURFACE_DARK: ColorValue = ColorValue::rgb(0xe6, 0xe1, 0xe5);

    /// タイポグラフィの論理フォントファミリ名。
    pub const FONT_FAMILY: &str = "Roboto";

    /// `radius.md` の上書き値（M3 shape scale medium 相当）。
    pub const RADIUS_MD: f32 = 12.0;

    /// `duration.normal` の上書き値（M3 medium2 相当）。
    pub const DURATION_NORMAL_MS: f32 = 300.0;
}

/// 中立既定値の全カテゴリを合成した `TokenSet` を構築する。
///
/// `color` / `typography` / `spacing` / `size` / `radius` / `elevation` /
/// `border` / `shadow` / `duration` / `animation` の各既定トークンセットを
/// マージする（キー空間はカテゴリごとに分離されているため衝突しない）。
/// プラットフォーム別の値上書き前の土台として [`platform_base_token_set`] から
/// 呼び出される。
fn neutral_base_token_set() -> Result<TokenSet, TokenError> {
    let mut set = TokenSet::new();
    set.extend(palette_token_set()?);
    set.extend(typography_token_set()?);
    set.extend(spacing_token_set()?);
    set.extend(size_token_set()?);
    set.extend(radius_token_set()?);
    set.extend(elevation_token_set()?);
    set.extend(border_token_set()?);
    set.extend(shadow_token_set()?);
    set.extend(duration_token_set()?);
    set.extend(super::animation::animation_token_set()?);
    Ok(set)
}

/// `TokenSet` へパレット色 1 件を挿入する（キーは `TokenPath::from_dotted`
/// 経由で検証済みリテラルからのみ生成する、`color::palette_token_set` と同じ
/// 契約）。
fn insert_palette_color(
    set: &mut TokenSet,
    key: &str,
    color: ColorValue,
) -> Result<(), TokenError> {
    let path = TokenPath::from_dotted(key)?;
    set.insert(
        path,
        TokenDefinition::Value {
            value: TokenValue::Color(color),
        },
    );
    Ok(())
}

/// `TokenSet` へタイポグラフィ 1 件（フォントファミリ・サイズの上書き）を
/// 挿入する。行間は [`PLATFORM_LINE_HEIGHT_RATIO`] からサイズに対する比率で
/// 算出する（`typography::typography_token_set` の既定値と同じ算出方針）。
fn insert_typography_override(
    set: &mut TokenSet,
    key: &str,
    family: &str,
    size: f32,
) -> Result<(), TokenError> {
    let path = TokenPath::from_dotted(key)?;
    let value = TypographyValue::new(
        family,
        size,
        size * PLATFORM_LINE_HEIGHT_RATIO,
        FontWeightValue::REGULAR,
        FontStyleValue::Normal,
    )?;
    set.insert(
        path,
        TokenDefinition::Value {
            value: TokenValue::Typography(value),
        },
    );
    Ok(())
}

/// `radius.md` の値を上書きする。
fn override_radius_md(set: &mut TokenSet, value_px: f32) -> Result<(), TokenError> {
    let path = TokenPath::from_dotted("radius.md")?;
    let dimension = DimensionValue::new(value_px)?;
    set.insert(
        path,
        TokenDefinition::Value {
            value: TokenValue::Radius(dimension),
        },
    );
    Ok(())
}

/// `duration.normal` の値を上書きする。
fn override_duration_normal(set: &mut TokenSet, milliseconds: f32) -> Result<(), TokenError> {
    let path = TokenPath::from_dotted("duration.normal")?;
    let duration = DurationValue::new(milliseconds)?;
    set.insert(
        path,
        TokenDefinition::Value {
            value: TokenValue::Duration(duration),
        },
    );
    Ok(())
}

/// `animation.<key>` トークンの duration のみを上書きする（easing は
/// `preset` の値を維持する）。
///
/// `animation.standard` / `animation.enter` は `animation::presets::STANDARD`
/// / `ENTER` の値（`duration::scale::NORMAL` を値コピーで埋め込んだ定数）を
/// そのまま採用しているため、`override_duration_normal` で `duration.normal`
/// のみを上書きしても両トークンには反映されない（`AnimationValue` が
/// duration を参照ではなく値として保持するため）。`duration.normal` を
/// 上書きするプラットフォーム（Android）では、本関数で `animation.standard`
/// / `animation.enter` の duration も同じ値へ揃え、`platform_base_token_set`
/// が返す `TokenSet` 内で「normal」相当の duration が完全に一致するようにする。
fn override_animation_duration(
    set: &mut TokenSet,
    key: &str,
    preset: AnimationValue,
    milliseconds: f32,
) -> Result<(), TokenError> {
    let path = TokenPath::from_dotted(key)?;
    let duration = DurationValue::new(milliseconds)?;
    let value = AnimationValue::new(duration, preset.easing());
    set.insert(
        path,
        TokenDefinition::Value {
            value: TokenValue::Animation(value),
        },
    );
    Ok(())
}

/// [`SemanticColorRole::ALL`] 全役割を、対応するパレットキーへのエイリアスと
/// して構築し `set` へマージする。役割 → パレットキーの対応は呼び出し元が
/// `role_to_palette_key` で与える（iOS/Android で対応表が異なるため）。
fn merge_semantic_roles(
    set: &mut TokenSet,
    role_to_palette_key: impl Fn(SemanticColorRole) -> &'static str,
) -> Result<(), TokenError> {
    let mut mapping = SemanticColorMapping::new();
    for role in SemanticColorRole::ALL {
        let key = role_to_palette_key(role);
        let path = TokenPath::from_dotted(key)?;
        mapping = mapping.with(role, super::schema::TokenRef::new(path));
    }
    mapping.merge_into(set)
}

/// 役割 → iOS パレットキーの対応（設計文書 §3.3 の値表）。
fn ios_role_to_palette_key(role: SemanticColorRole) -> &'static str {
    match role {
        SemanticColorRole::Primary => "color.palette.ios-blue",
        SemanticColorRole::Secondary => "color.palette.ios-gray",
        SemanticColorRole::Background => "color.palette.ios-background",
        SemanticColorRole::Surface => "color.palette.ios-surface",
        SemanticColorRole::Error => "color.palette.ios-red",
        SemanticColorRole::Warning => "color.palette.ios-orange",
        SemanticColorRole::Success => "color.palette.ios-green",
        SemanticColorRole::OnPrimary => "color.palette.ios-on-accent",
        SemanticColorRole::OnBackground | SemanticColorRole::OnSurface => "color.palette.ios-label",
    }
}

/// 役割 → Android パレットキーの対応（設計文書 §3.3 の値表）。
fn android_role_to_palette_key(role: SemanticColorRole) -> &'static str {
    match role {
        SemanticColorRole::Primary => "color.palette.android-primary",
        SemanticColorRole::Secondary => "color.palette.android-secondary",
        SemanticColorRole::Background => "color.palette.android-background",
        SemanticColorRole::Surface => "color.palette.android-surface",
        SemanticColorRole::Error => "color.palette.android-error",
        SemanticColorRole::Warning => "color.palette.android-warning",
        SemanticColorRole::Success => "color.palette.android-success",
        SemanticColorRole::OnPrimary => "color.palette.android-on-primary",
        SemanticColorRole::OnBackground | SemanticColorRole::OnSurface => {
            "color.palette.android-on-surface"
        }
    }
}

/// iOS 準拠のライト既定値をベースセットへ重ねる。
fn apply_ios_base(set: &mut TokenSet) -> Result<(), TokenError> {
    insert_palette_color(set, "color.palette.ios-blue", ios::BLUE_LIGHT)?;
    insert_palette_color(set, "color.palette.ios-gray", ios::GRAY)?;
    insert_palette_color(set, "color.palette.ios-background", ios::BACKGROUND_LIGHT)?;
    insert_palette_color(set, "color.palette.ios-surface", ios::SURFACE_LIGHT)?;
    insert_palette_color(set, "color.palette.ios-red", ios::RED_LIGHT)?;
    insert_palette_color(set, "color.palette.ios-orange", ios::ORANGE_LIGHT)?;
    insert_palette_color(set, "color.palette.ios-green", ios::GREEN_LIGHT)?;
    insert_palette_color(set, "color.palette.ios-on-accent", ios::ON_ACCENT)?;
    insert_palette_color(set, "color.palette.ios-label", ios::LABEL_LIGHT)?;

    merge_semantic_roles(set, ios_role_to_palette_key)?;

    insert_typography_override(
        set,
        "typography.display",
        ios::FONT_FAMILY,
        ios::typography_scale::DISPLAY,
    )?;
    insert_typography_override(
        set,
        "typography.headline",
        ios::FONT_FAMILY,
        ios::typography_scale::HEADLINE,
    )?;
    insert_typography_override(
        set,
        "typography.title",
        ios::FONT_FAMILY,
        ios::typography_scale::TITLE,
    )?;
    insert_typography_override(
        set,
        "typography.body",
        ios::FONT_FAMILY,
        ios::typography_scale::BODY,
    )?;
    insert_typography_override(
        set,
        "typography.label",
        ios::FONT_FAMILY,
        ios::typography_scale::LABEL,
    )?;
    insert_typography_override(
        set,
        "typography.caption",
        ios::FONT_FAMILY,
        ios::typography_scale::CAPTION,
    )?;

    override_radius_md(set, ios::RADIUS_MD)?;
    // duration.normal（250ms）は UIKit 標準（0.25s）と一致するため上書きしない。

    Ok(())
}

/// Android 準拠のライト既定値をベースセットへ重ねる。
fn apply_android_base(set: &mut TokenSet) -> Result<(), TokenError> {
    insert_palette_color(set, "color.palette.android-primary", android::PRIMARY_LIGHT)?;
    insert_palette_color(
        set,
        "color.palette.android-secondary",
        android::SECONDARY_LIGHT,
    )?;
    insert_palette_color(
        set,
        "color.palette.android-background",
        android::BACKGROUND_LIGHT,
    )?;
    insert_palette_color(set, "color.palette.android-surface", android::SURFACE_LIGHT)?;
    insert_palette_color(set, "color.palette.android-error", android::ERROR_LIGHT)?;
    insert_palette_color(set, "color.palette.android-warning", android::WARNING_LIGHT)?;
    insert_palette_color(set, "color.palette.android-success", android::SUCCESS_LIGHT)?;
    insert_palette_color(
        set,
        "color.palette.android-on-primary",
        android::ON_PRIMARY_LIGHT,
    )?;
    insert_palette_color(
        set,
        "color.palette.android-on-surface",
        android::ON_SURFACE_LIGHT,
    )?;

    merge_semantic_roles(set, android_role_to_palette_key)?;

    // Material 3 の type scale（display-small/headline-medium/title-large/
    // body-large/label-large 相当）は既存の中立スケール定数と数値が一致する
    // ため、フォントファミリのみを上書きしフォントサイズは変更しない。
    insert_typography_override(
        set,
        "typography.display",
        android::FONT_FAMILY,
        super::typography::scale::DISPLAY,
    )?;
    insert_typography_override(
        set,
        "typography.headline",
        android::FONT_FAMILY,
        super::typography::scale::HEADLINE,
    )?;
    insert_typography_override(
        set,
        "typography.title",
        android::FONT_FAMILY,
        super::typography::scale::TITLE,
    )?;
    insert_typography_override(
        set,
        "typography.body",
        android::FONT_FAMILY,
        super::typography::scale::BODY,
    )?;
    insert_typography_override(
        set,
        "typography.label",
        android::FONT_FAMILY,
        super::typography::scale::LABEL,
    )?;
    insert_typography_override(
        set,
        "typography.caption",
        android::FONT_FAMILY,
        super::typography::scale::CAPTION,
    )?;

    override_radius_md(set, android::RADIUS_MD)?;
    override_duration_normal(set, android::DURATION_NORMAL_MS)?;
    // animation.standard / animation.enter は NORMAL（250ms）を値コピーで
    // 埋め込んでいるため、上記の duration.normal 上書きだけでは反映されない
    // （override_animation_duration のドキュメント参照）。Bugbot 指摘（PR #428）
    // により発覚した「normal」duration の 300ms/250ms 不整合を解消する。
    override_animation_duration(
        set,
        "animation.standard",
        animation_presets::STANDARD,
        android::DURATION_NORMAL_MS,
    )?;
    override_animation_duration(
        set,
        "animation.enter",
        animation_presets::ENTER,
        android::DURATION_NORMAL_MS,
    )?;

    Ok(())
}

/// プラットフォーム準拠のライト既定テーマ（ベース `TokenSet`）を構築する。
///
/// `TokenResolver::new` の `base` 引数へそのまま渡せる完全なトークンセット
/// （中立既定値 + プラットフォーム準拠の上書き）を返す。S14/S15（ウィジェット・
/// レイアウトへのトークン適用配線）・S19（AI 型安全 DSL）が消費する想定。
///
/// # Errors
///
/// 各カテゴリの既定トークンセット構築、またはプラットフォーム別上書きの
/// いずれかが失敗した場合にそのままエラーを伝播する（本モジュール内の実装
/// ミスでない限り、定義済みリテラルのみから構築するため通常は起こらない）。
pub fn platform_base_token_set(platform: ThemePlatform) -> Result<TokenSet, TokenError> {
    let mut set = neutral_base_token_set()?;
    match platform {
        ThemePlatform::Ios => apply_ios_base(&mut set)?,
        ThemePlatform::Android => apply_android_base(&mut set)?,
    }
    Ok(set)
}

/// プラットフォーム準拠のダークテーマ差分を構築する。
///
/// パレット値の上書きのみを含む最小の `TokenSet` を返す（セマンティック
/// カラーのエイリアス自体はライト/ダークで変わらず、参照先パレットの値だけが
/// 変わる設計、モジュール doc 参照）。ライト/ダークで値が同一のキー
/// （iOS の `secondary` / `on-primary` 等）は意図的に含めず、resolver の
/// フォールバック意味論（ベース値を使う）に委ねる。
/// `TokenResolver::new` の `theme_overrides`引数 / `set_theme_overrides` へ
/// 渡す想定。
///
/// # Errors
///
/// パレットキーの `TokenPath` 構築が失敗した場合にそのままエラーを伝播する
/// （定義済みリテラルのみから構築するため通常は起こらない）。
pub fn platform_dark_overrides(platform: ThemePlatform) -> Result<TokenSet, TokenError> {
    let mut set = TokenSet::new();
    match platform {
        ThemePlatform::Ios => {
            insert_palette_color(&mut set, "color.palette.ios-blue", ios::BLUE_DARK)?;
            insert_palette_color(
                &mut set,
                "color.palette.ios-background",
                ios::BACKGROUND_DARK,
            )?;
            insert_palette_color(&mut set, "color.palette.ios-surface", ios::SURFACE_DARK)?;
            insert_palette_color(&mut set, "color.palette.ios-red", ios::RED_DARK)?;
            insert_palette_color(&mut set, "color.palette.ios-orange", ios::ORANGE_DARK)?;
            insert_palette_color(&mut set, "color.palette.ios-green", ios::GREEN_DARK)?;
            insert_palette_color(&mut set, "color.palette.ios-label", ios::LABEL_DARK)?;
            // secondary（ios-gray）・on-primary（ios-on-accent）はライト/ダークで
            // 値が変わらないため、意図的にダーク差分へ含めない。
        }
        ThemePlatform::Android => {
            insert_palette_color(
                &mut set,
                "color.palette.android-primary",
                android::PRIMARY_DARK,
            )?;
            insert_palette_color(
                &mut set,
                "color.palette.android-secondary",
                android::SECONDARY_DARK,
            )?;
            insert_palette_color(
                &mut set,
                "color.palette.android-background",
                android::BACKGROUND_DARK,
            )?;
            insert_palette_color(
                &mut set,
                "color.palette.android-surface",
                android::SURFACE_DARK,
            )?;
            insert_palette_color(&mut set, "color.palette.android-error", android::ERROR_DARK)?;
            insert_palette_color(
                &mut set,
                "color.palette.android-warning",
                android::WARNING_DARK,
            )?;
            insert_palette_color(
                &mut set,
                "color.palette.android-success",
                android::SUCCESS_DARK,
            )?;
            insert_palette_color(
                &mut set,
                "color.palette.android-on-primary",
                android::ON_PRIMARY_DARK,
            )?;
            insert_palette_color(
                &mut set,
                "color.palette.android-on-surface",
                android::ON_SURFACE_DARK,
            )?;
        }
    }
    Ok(set)
}

/// `platform` の既定テーマを `variant` 区分の [`Theme`] として構築する
/// （S5、#161 が追加した `ThemeVariant` / `Theme` との統合ブリッジ）。
///
/// `ThemeVariant::Light` は空差分（ベース値をそのまま使う。設計文書 §2.3の
/// フォールバック意味論と整合）、`ThemeVariant::Dark` は
/// [`platform_dark_overrides`] の差分を持つ `Theme` を返す。戻り値は
/// `TokenResolver::with_themes` の `light` / `dark` 引数へそのまま渡せる
/// （`ui::theme_demo::demo_resolver` がこの経路で利用する）。
///
/// # Errors
///
/// `ThemeVariant::Dark` の場合、[`platform_dark_overrides`] の構築が失敗すると
/// そのまま伝播する（定義済みリテラルのみから構築するため通常は起こらない）。
pub fn platform_theme(platform: ThemePlatform, variant: ThemeVariant) -> Result<Theme, TokenError> {
    let overrides = match variant {
        ThemeVariant::Light => TokenSet::new(),
        ThemeVariant::Dark => platform_dark_overrides(platform)?,
    };
    Ok(Theme::new(variant, overrides))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::resolver::TokenResolver;
    use crate::token::schema::{TokenCategory, TokenRef};
    use crate::token::semantic_color::resolve_semantic_color;

    fn resolver_for(platform: ThemePlatform, dark: bool) -> TokenResolver {
        let base = platform_base_token_set(platform).expect("ベース構築は成功する");
        let overrides = if dark {
            platform_dark_overrides(platform).expect("ダーク差分構築は成功する")
        } else {
            TokenSet::new()
        };
        TokenResolver::new(base, overrides)
    }

    fn assert_color(resolver: &TokenResolver, role: SemanticColorRole, expected: ColorValue) {
        let value = resolve_semantic_color(resolver, role).expect("役割は解決できる");
        assert_eq!(value, TokenValue::Color(expected));
    }

    #[test]
    fn ios_light_resolves_all_semantic_roles_to_expected_values() {
        let resolver = resolver_for(ThemePlatform::Ios, false);
        assert_color(&resolver, SemanticColorRole::Primary, ios::BLUE_LIGHT);
        assert_color(&resolver, SemanticColorRole::Secondary, ios::GRAY);
        assert_color(
            &resolver,
            SemanticColorRole::Background,
            ios::BACKGROUND_LIGHT,
        );
        assert_color(&resolver, SemanticColorRole::Surface, ios::SURFACE_LIGHT);
        assert_color(&resolver, SemanticColorRole::Error, ios::RED_LIGHT);
        assert_color(&resolver, SemanticColorRole::Warning, ios::ORANGE_LIGHT);
        assert_color(&resolver, SemanticColorRole::Success, ios::GREEN_LIGHT);
        assert_color(&resolver, SemanticColorRole::OnPrimary, ios::ON_ACCENT);
        assert_color(&resolver, SemanticColorRole::OnBackground, ios::LABEL_LIGHT);
        assert_color(&resolver, SemanticColorRole::OnSurface, ios::LABEL_LIGHT);
    }

    #[test]
    fn android_light_resolves_all_semantic_roles_to_expected_values() {
        let resolver = resolver_for(ThemePlatform::Android, false);
        assert_color(
            &resolver,
            SemanticColorRole::Primary,
            android::PRIMARY_LIGHT,
        );
        assert_color(
            &resolver,
            SemanticColorRole::Secondary,
            android::SECONDARY_LIGHT,
        );
        assert_color(
            &resolver,
            SemanticColorRole::Background,
            android::BACKGROUND_LIGHT,
        );
        assert_color(
            &resolver,
            SemanticColorRole::Surface,
            android::SURFACE_LIGHT,
        );
        assert_color(&resolver, SemanticColorRole::Error, android::ERROR_LIGHT);
        assert_color(
            &resolver,
            SemanticColorRole::Warning,
            android::WARNING_LIGHT,
        );
        assert_color(
            &resolver,
            SemanticColorRole::Success,
            android::SUCCESS_LIGHT,
        );
        assert_color(
            &resolver,
            SemanticColorRole::OnPrimary,
            android::ON_PRIMARY_LIGHT,
        );
        assert_color(
            &resolver,
            SemanticColorRole::OnBackground,
            android::ON_SURFACE_LIGHT,
        );
        assert_color(
            &resolver,
            SemanticColorRole::OnSurface,
            android::ON_SURFACE_LIGHT,
        );
    }

    #[test]
    fn ios_dark_overrides_swap_expected_roles() {
        let resolver = resolver_for(ThemePlatform::Ios, true);
        assert_color(&resolver, SemanticColorRole::Primary, ios::BLUE_DARK);
        assert_color(
            &resolver,
            SemanticColorRole::Background,
            ios::BACKGROUND_DARK,
        );
        assert_color(&resolver, SemanticColorRole::Surface, ios::SURFACE_DARK);
        assert_color(&resolver, SemanticColorRole::Error, ios::RED_DARK);
        assert_color(&resolver, SemanticColorRole::Warning, ios::ORANGE_DARK);
        assert_color(&resolver, SemanticColorRole::Success, ios::GREEN_DARK);
        assert_color(&resolver, SemanticColorRole::OnBackground, ios::LABEL_DARK);
        assert_color(&resolver, SemanticColorRole::OnSurface, ios::LABEL_DARK);
    }

    #[test]
    fn ios_dark_fallback_keeps_base_value_for_keys_absent_from_dark_diff() {
        // secondary（ios-gray）・on-primary（ios-on-accent）はダーク差分に
        // 含まれないため、ベース（ライト）値のまま解決されることを確認する
        // （theme-switching-architecture.md §2.3 のフォールバック意味論）。
        let resolver = resolver_for(ThemePlatform::Ios, true);
        assert_color(&resolver, SemanticColorRole::Secondary, ios::GRAY);
        assert_color(&resolver, SemanticColorRole::OnPrimary, ios::ON_ACCENT);

        // 中立トークン（テーマ非依存の spacing.md 等）もベース値のまま解決される。
        let reference = TokenRef::new(TokenPath::from_dotted("spacing.md").expect("パスは有効"));
        let resolved = resolver
            .resolve(&reference)
            .expect("spacing.md は解決できる");
        assert_eq!(
            resolved.value().category(),
            TokenCategory::Spacing,
            "spacing.md はテーマに依存せず中立トークンのまま解決される"
        );
    }

    #[test]
    fn android_dark_overrides_swap_all_roles() {
        let resolver = resolver_for(ThemePlatform::Android, true);
        assert_color(&resolver, SemanticColorRole::Primary, android::PRIMARY_DARK);
        assert_color(
            &resolver,
            SemanticColorRole::Secondary,
            android::SECONDARY_DARK,
        );
        assert_color(
            &resolver,
            SemanticColorRole::Background,
            android::BACKGROUND_DARK,
        );
        assert_color(&resolver, SemanticColorRole::Surface, android::SURFACE_DARK);
        assert_color(&resolver, SemanticColorRole::Error, android::ERROR_DARK);
        assert_color(&resolver, SemanticColorRole::Warning, android::WARNING_DARK);
        assert_color(&resolver, SemanticColorRole::Success, android::SUCCESS_DARK);
        assert_color(
            &resolver,
            SemanticColorRole::OnPrimary,
            android::ON_PRIMARY_DARK,
        );
        assert_color(
            &resolver,
            SemanticColorRole::OnBackground,
            android::ON_SURFACE_DARK,
        );
        assert_color(
            &resolver,
            SemanticColorRole::OnSurface,
            android::ON_SURFACE_DARK,
        );
    }

    #[test]
    fn ios_typography_uses_platform_family_and_sizes() {
        let set = platform_base_token_set(ThemePlatform::Ios).expect("構築は成功する");
        let path = TokenPath::from_dotted("typography.body").expect("パスは有効");
        match set.get(&path) {
            Some(TokenDefinition::Value {
                value: TokenValue::Typography(value),
            }) => {
                assert_eq!(value.family(), ios::FONT_FAMILY);
                assert_eq!(value.size(), ios::typography_scale::BODY);
            }
            other => panic!("typography.body は Typography 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn android_typography_uses_platform_family_with_neutral_sizes() {
        let set = platform_base_token_set(ThemePlatform::Android).expect("構築は成功する");
        let path = TokenPath::from_dotted("typography.body").expect("パスは有効");
        match set.get(&path) {
            Some(TokenDefinition::Value {
                value: TokenValue::Typography(value),
            }) => {
                assert_eq!(value.family(), android::FONT_FAMILY);
                assert_eq!(value.size(), super::super::typography::scale::BODY);
            }
            other => panic!("typography.body は Typography 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn ios_radius_md_is_overridden() {
        let set = platform_base_token_set(ThemePlatform::Ios).expect("構築は成功する");
        let path = TokenPath::from_dotted("radius.md").expect("パスは有効");
        match set.get(&path) {
            Some(TokenDefinition::Value {
                value: TokenValue::Radius(value),
            }) => assert_eq!(value.value(), ios::RADIUS_MD),
            other => panic!("radius.md は Radius 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn android_radius_md_and_duration_normal_are_overridden() {
        let set = platform_base_token_set(ThemePlatform::Android).expect("構築は成功する");

        let radius_path = TokenPath::from_dotted("radius.md").expect("パスは有効");
        match set.get(&radius_path) {
            Some(TokenDefinition::Value {
                value: TokenValue::Radius(value),
            }) => assert_eq!(value.value(), android::RADIUS_MD),
            other => panic!("radius.md は Radius 値であるべきです: {other:?}"),
        }

        let duration_path = TokenPath::from_dotted("duration.normal").expect("パスは有効");
        match set.get(&duration_path) {
            Some(TokenDefinition::Value {
                value: TokenValue::Duration(value),
            }) => assert_eq!(value.milliseconds(), android::DURATION_NORMAL_MS),
            other => panic!("duration.normal は Duration 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn android_animation_standard_and_enter_duration_match_duration_normal() {
        // Bugbot 指摘（PR #428）: apply_android_base が duration.normal のみを
        // 300ms へ上書きすると、animation.standard / animation.enter は
        // NORMAL（250ms）を値コピーで埋め込んだままのため「normal」相当の
        // duration が 300ms と 250ms の 2 通りで食い違っていた。両トークンが
        // duration.normal と同じ値を持つことを固定する回帰テスト。
        let set = platform_base_token_set(ThemePlatform::Android).expect("構築は成功する");

        let duration_path = TokenPath::from_dotted("duration.normal").expect("パスは有効");
        let expected_ms = match set.get(&duration_path) {
            Some(TokenDefinition::Value {
                value: TokenValue::Duration(value),
            }) => value.milliseconds(),
            other => panic!("duration.normal は Duration 値であるべきです: {other:?}"),
        };
        assert_eq!(expected_ms, android::DURATION_NORMAL_MS);

        for key in ["animation.standard", "animation.enter"] {
            let path = TokenPath::from_dotted(key).expect("パスは有効");
            match set.get(&path) {
                Some(TokenDefinition::Value {
                    value: TokenValue::Animation(value),
                }) => assert_eq!(
                    value.duration().milliseconds(),
                    expected_ms,
                    "{key} の duration は duration.normal と一致する必要があります"
                ),
                other => panic!("{key} は Animation 値であるべきです: {other:?}"),
            }
        }
    }

    #[test]
    fn ios_duration_normal_is_not_overridden() {
        // iOS は UIKit 標準（0.25s）と一致するため duration.normal を上書きしない。
        let set = platform_base_token_set(ThemePlatform::Ios).expect("構築は成功する");
        let path = TokenPath::from_dotted("duration.normal").expect("パスは有効");
        match set.get(&path) {
            Some(TokenDefinition::Value {
                value: TokenValue::Duration(value),
            }) => assert_eq!(
                value.milliseconds(),
                super::super::animation::scale::NORMAL.milliseconds()
            ),
            other => panic!("duration.normal は Duration 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn platform_base_token_set_resolves_without_error_via_resolve_all() {
        // 全カテゴリ・両プラットフォームで resolve_all が全件成功することを
        // 確認する（型不一致・未定義参照が混入していないことの回帰確認）。
        for platform in [ThemePlatform::Ios, ThemePlatform::Android] {
            let base = platform_base_token_set(platform).expect("構築は成功する");
            let mut resolver = TokenResolver::new(base, TokenSet::new());
            resolver
                .resolve_all()
                .expect("既定テーマ全件が解決に成功する");
        }
    }

    #[test]
    fn platform_dark_overrides_resolves_without_error_via_resolve_all() {
        for platform in [ThemePlatform::Ios, ThemePlatform::Android] {
            let base = platform_base_token_set(platform).expect("構築は成功する");
            let overrides = platform_dark_overrides(platform).expect("ダーク差分構築は成功する");
            let mut resolver = TokenResolver::new(base, overrides);
            resolver
                .resolve_all()
                .expect("ダークテーマ全件が解決に成功する");
        }
    }

    #[test]
    fn platform_theme_light_returns_empty_overrides() {
        for platform in [ThemePlatform::Ios, ThemePlatform::Android] {
            let theme =
                platform_theme(platform, ThemeVariant::Light).expect("ライトテーマは構築できる");
            assert_eq!(theme.variant(), ThemeVariant::Light);
            assert!(
                theme.overrides().is_empty(),
                "{platform:?}: ライトテーマの差分は空である必要がある"
            );
        }
    }

    #[test]
    fn platform_theme_dark_matches_platform_dark_overrides() {
        for platform in [ThemePlatform::Ios, ThemePlatform::Android] {
            let theme =
                platform_theme(platform, ThemeVariant::Dark).expect("ダークテーマは構築できる");
            let expected = platform_dark_overrides(platform).expect("ダーク差分構築は成功する");
            assert_eq!(theme.variant(), ThemeVariant::Dark);
            assert_eq!(theme.overrides(), &expected);
        }
    }

    #[test]
    fn platform_theme_feeds_with_themes_and_switches_via_set_active_theme() {
        // platform_theme が TokenResolver::with_themes へそのまま渡せる Theme を
        // 返し、set_active_theme 経由での切替が機能することを確認する
        // （demo_resolver の想定利用経路の固定化）。
        for platform in [ThemePlatform::Ios, ThemePlatform::Android] {
            let base = platform_base_token_set(platform).expect("ベース構築は成功する");
            let light = platform_theme(platform, ThemeVariant::Light).expect("ライト構築は成功");
            let dark = platform_theme(platform, ThemeVariant::Dark).expect("ダーク構築は成功");

            let mut resolver = TokenResolver::with_themes(base, light, dark, ThemeVariant::Light);
            let light_primary = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
                .expect("ライトの primary は解決できる");

            resolver
                .set_active_theme(ThemeVariant::Dark)
                .expect("ダークへの切替は成功する");
            let dark_primary = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
                .expect("ダークの primary は解決できる");

            assert_ne!(
                light_primary, dark_primary,
                "{platform:?}: set_active_theme 切替で primary の解決値が変わる必要がある"
            );
        }
    }
}
