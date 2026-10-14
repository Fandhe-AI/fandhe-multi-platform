//! タイポグラフィトークンの専用値型・既定スケール定義（TASK-8.4-S8、#164 /
//! TASK-8.4-S9、#165）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4.1・
//! §4.3・§5.2 が定めるタイポグラフィトークンの値モデルを実装契約として引き継ぐ。
//! 同文書 §4.1 の割り当て表により typography カテゴリは S8（フォントファミリ・
//! サイズ、#164）と S9（行間・ウェイト・スタイル、#165）の 2 issue に分割されて
//! 実装されたが、両者は同一の [`TypographyValue`] のフィールド群として本モジュール
//! で統合済みである。
//!
//! `schema::TokenValue::Typography` は S8 で新設したバリアントであり、
//! [`ColorValue`](super::color::ColorValue) が確立した「専用値型 + 検証付き
//! 構築 + 既定トークンセット関数」というパターンを踏襲する。`TypographyValue`
//! は非公開フィールド + 検証付きコンストラクタ [`TypographyValue::new`] +
//! アクセサという構成を取る（`ColorValue` は全域が正当な `u8` のため公開
//! フィールドで足りるが、`TypographyValue` は空文字列・非有限値等の不変条件を
//! 持つため、構築経路を型で閉じている）。
//!
//! プラットフォーム別の具体値調整（iOS HIG / Material 準拠）は S16（#172）の
//! スコープであり、本モジュールの既定値はフレームワーク中立に留める。外部定義
//! ファイル読み込み時の構文パース（DTCG 等）は S17（#173）のスコープ、AI 型
//! 安全 DSL からの型付き参照 API は S19（#175）のスコープ。いずれも本モジュールは
//! 検証済みの `TypographyValue` を受け渡すデータ表現としてのみ関与する。

use super::error::TokenError;
use super::schema::{TokenDefinition, TokenPath, TokenSet, TokenValue};

/// フォントファミリ名として許容する最大バイト長。
///
/// `family` が外部入力（S17 経由）由来になり得ることを見据え、無制限の
/// 巨大文字列がトークンセットへ混入しないよう上限を設ける（OWASP A03/A04
/// 対策、security.md）。具体値は実運用上十分に長いフォント名（フォールバック
/// リストを連結した文字列等）を許容しつつ、明らかな異常値を拒否する目安として
/// 定めた暫定値であり、本格的な上限設計は S17 のスコープ。
const MAX_FAMILY_LEN: usize = 256;

/// フォントウェイト（100..=900、設計文書 §4.3）。
///
/// 検証付きコンストラクタ [`FontWeightValue::new`] を経由してのみ構築でき、域外の
/// 数値（99 以下・901 以上）は構築時点で `Result` により拒否する
/// （coding-rust.md の panic 禁止方針。設計文書の素案では `weight: u16` の生値だが、
/// `ColorValue::from_hex` の検証パターンに合わせて newtype 化した precision 向上
/// であり、100..=900 の数値域という値モデル自体は変えていない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontWeightValue(u16);

impl FontWeightValue {
    /// フォントウェイトの最小許容値（Thin 相当）。
    pub const MIN: u16 = 100;
    /// フォントウェイトの最大許容値（Black 相当）。
    pub const MAX: u16 = 900;

    /// Thin（100）。
    pub const THIN: Self = Self(100);
    /// ExtraLight（200）。
    pub const EXTRA_LIGHT: Self = Self(200);
    /// Light（300）。
    pub const LIGHT: Self = Self(300);
    /// Regular（400、既定のウェイト）。
    pub const REGULAR: Self = Self(400);
    /// Medium（500）。
    pub const MEDIUM: Self = Self(500);
    /// SemiBold（600）。
    pub const SEMI_BOLD: Self = Self(600);
    /// Bold（700）。
    pub const BOLD: Self = Self(700);
    /// ExtraBold（800）。
    pub const EXTRA_BOLD: Self = Self(800);
    /// Black（900）。
    pub const BLACK: Self = Self(900);

    /// 数値からフォントウェイトを構築する。
    ///
    /// # Errors
    ///
    /// `100..=900` の範囲外の値が渡された場合に
    /// [`TokenError::InvalidTypography`] を返す。
    pub fn new(value: u16) -> Result<Self, TokenError> {
        if !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(TokenError::InvalidTypography {
                reason: format!(
                    "フォントウェイトは {}..={} の範囲である必要があります: {value}",
                    Self::MIN,
                    Self::MAX
                ),
            });
        }
        Ok(Self(value))
    }

    /// 内部で保持する数値表現（100..=900）を返す。
    pub fn value(self) -> u16 {
        self.0
    }
}

/// フォントスタイル（閉じた列挙、設計文書 §5.2 の `FontStyleValue`）。
///
/// 斜体表現は normal / italic の 2 値に閉じる（oblique 等の追加は設計文書の
/// スコープ外）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontStyleValue {
    /// 通常のスタイル（既定値）。
    Normal,
    /// イタリック体。
    Italic,
}

/// タイポグラフィトークンの値（設計文書 §4.3・§5.2 準拠）。
///
/// フィールドは非公開とし、[`TypographyValue::new`] による検証付き構築と
/// アクセサ（[`TypographyValue::family`] / [`TypographyValue::size`] /
/// [`TypographyValue::line_height`] / [`TypographyValue::weight`] /
/// [`TypographyValue::style`]）経由でのみ値へアクセスできる（`ColorValue` と
/// 異なり不変条件を持つため、構築経路を型で閉じる）。フォントファミリ・サイズ
/// （S8、#164）と行間・ウェイト・スタイル（S9、#165）は設計文書 §4.1 の割り当て表
/// が定める同一カテゴリの分割であり、本型として統合済み。
#[derive(Debug, Clone, PartialEq)]
pub struct TypographyValue {
    family: String,
    size: f32,
    line_height: f32,
    weight: FontWeightValue,
    style: FontStyleValue,
}

impl TypographyValue {
    /// 全フィールドを検証したうえで構築する。
    ///
    /// `family` は将来 S17（外部定義ファイル読み込み）経由で外部入力になり
    /// 得る文字列であり、最終的にプラットフォーム側のテキスト API・CSS
    /// `font-family` へ渡る（OWASP A03 対策として、検証済みの構築経路のみを
    /// 公開する）。`line_height` は「有限かつ正の f32」であることを検証する
    /// （NaN・無限大・0 以下は拒否し、後段のレイアウト計算（taffy）への
    /// 異常値伝播を防ぐ、security.md の DoS 耐性観点）。
    ///
    /// # Errors
    ///
    /// 以下のいずれかに該当する場合 [`TokenError::InvalidTypography`] を返す。
    /// - `family` が空文字列
    /// - `family` が制御文字を含む
    /// - `family` が [`MAX_FAMILY_LEN`] バイトを超える
    /// - `size` が非有限（`NaN` / `±inf`）
    /// - `size` が `0.0` 以下
    /// - `line_height` が非有限、または `0.0` 以下
    pub fn new(
        family: impl Into<String>,
        size: f32,
        line_height: f32,
        weight: FontWeightValue,
        style: FontStyleValue,
    ) -> Result<Self, TokenError> {
        let family = family.into();

        if family.is_empty() {
            return Err(TokenError::InvalidTypography {
                reason: "フォントファミリ名は空文字列にできません".to_string(),
            });
        }
        if family.len() > MAX_FAMILY_LEN {
            return Err(TokenError::InvalidTypography {
                reason: format!(
                    "フォントファミリ名が長すぎます（上限 {MAX_FAMILY_LEN} バイト）: {} バイト",
                    family.len()
                ),
            });
        }
        if family.chars().any(|c| c.is_control()) {
            return Err(TokenError::InvalidTypography {
                reason: format!("フォントファミリ名に制御文字が含まれています: {family:?}"),
            });
        }

        if !size.is_finite() {
            return Err(TokenError::InvalidTypography {
                reason: format!("サイズは有限の数値である必要があります: {size}"),
            });
        }
        if size <= 0.0 {
            return Err(TokenError::InvalidTypography {
                reason: format!("サイズは正の数値である必要があります: {size}"),
            });
        }

        if !(line_height.is_finite() && line_height > 0.0) {
            return Err(TokenError::InvalidTypography {
                reason: format!(
                    "行間は有限かつ正の値である必要があります（NaN・無限大・0 以下は不可）: {line_height}"
                ),
            });
        }

        Ok(Self {
            family,
            size,
            line_height,
            weight,
            style,
        })
    }

    /// フォントファミリ名を返す。
    pub fn family(&self) -> &str {
        &self.family
    }

    /// サイズ（論理ピクセル相当、`geometry.rs` と単位系を揃える）を返す。
    pub fn size(&self) -> f32 {
        self.size
    }

    /// 行間（論理ピクセル相当）を返す。
    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    /// フォントウェイトを返す。
    pub fn weight(&self) -> FontWeightValue {
        self.weight
    }

    /// フォントスタイルを返す。
    pub fn style(&self) -> FontStyleValue {
        self.style
    }
}

/// フォントファミリ定数（`typography.*` の `family` として使う論理名）。
///
/// CSS 汎用ファミリ名相当のプラットフォーム中立な論理名であり、実フォントへの
/// 解決（フォントフォールバック・実体フォントファイルの選定）はプラットフォーム
/// 統合レイヤの責務とする（本クレートは論理名の受け渡しのみを担う）。
pub mod family {
    /// 既定のフォントファミリ（サンセリフ系）。
    pub const DEFAULT: &str = "sans-serif";
    /// セリフ系フォントファミリ。
    pub const SERIF: &str = "serif";
    /// 等幅フォントファミリ。
    pub const MONOSPACE: &str = "monospace";
}

/// サイズスケール定数（論理ピクセル相当、設計文書 §4.3 準拠）。
///
/// AI 型安全 DSL（S19、#175）からの型付き参照を見据え、`pub const` として
/// 公開する（`color::palette::*` と同方針）。プラットフォーム準拠の具体値
/// 調整は S16（#172）のスコープであり、ここではフレームワーク中立の既定値
/// のみを定める。
pub mod scale {
    /// 見出し級の大きな表示サイズ。
    pub const DISPLAY: f32 = 36.0;
    /// 大見出し。
    pub const HEADLINE: f32 = 28.0;
    /// 中見出し。
    pub const TITLE: f32 = 22.0;
    /// 本文サイズ。
    pub const BODY: f32 = 16.0;
    /// 補助ラベル。
    pub const LABEL: f32 = 14.0;
    /// キャプション（最小サイズ）。
    pub const CAPTION: f32 = 12.0;
}

/// 既定の行間比率（フォントサイズに対する倍率、一般的な本文組版の目安値）。
///
/// 設計文書は既定テーマの具体値調整を S16（#172）のスコープとしているため、
/// ここではフレームワーク中立な暫定係数として定義し、`typography_token_set` の
/// 既定スケール生成にのみ用いる。
const DEFAULT_LINE_HEIGHT_RATIO: f32 = 1.4;

/// 既定タイポグラフィスケール全件を束ねた `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`typography.<スケール名>`）に従い、
/// 検証付きコンストラクタ `TokenPath::from_dotted` 経由でのみ生成する
/// （`color::palette_token_set` と同じ契約、生文字列を解決エンジンの境界へ
/// 渡さない、`token_resolution-architecture.md` §3.1）。全件 `family::DEFAULT`・
/// 既定ウェイト（`FontWeightValue::REGULAR`）・既定スタイル（`FontStyleValue::Normal`）
/// を使うため `TypographyValue::new` が失敗することはないが、`unwrap`/`expect`
/// を使わず `?` で伝播する（coding-rust.md の panic 禁止方針）。呼び出し元は
/// `TokenResolver::new` の `base` 引数としてこの戻り値をそのまま利用できる。
pub fn typography_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, f32)] = &[
        ("typography.display", scale::DISPLAY),
        ("typography.headline", scale::HEADLINE),
        ("typography.title", scale::TITLE),
        ("typography.body", scale::BODY),
        ("typography.label", scale::LABEL),
        ("typography.caption", scale::CAPTION),
    ];

    let mut set = TokenSet::new();
    for (key, size) in entries {
        let path = TokenPath::from_dotted(key)?;
        let value = TypographyValue::new(
            family::DEFAULT,
            *size,
            size * DEFAULT_LINE_HEIGHT_RATIO,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Typography(value),
            },
        );
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_succeeds_with_valid_family_and_size() {
        let value = TypographyValue::new(
            "sans-serif",
            16.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect("正当な値の構築は成功する");
        assert_eq!(value.family(), "sans-serif");
        assert_eq!(value.size(), 16.0);
        assert_eq!(value.line_height(), 24.0);
        assert_eq!(value.weight(), FontWeightValue::REGULAR);
        assert_eq!(value.style(), FontStyleValue::Normal);
    }

    #[test]
    fn new_rejects_empty_family() {
        let err = TypographyValue::new(
            "",
            16.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("空文字列の family は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn new_rejects_family_with_control_characters() {
        let err = TypographyValue::new(
            "sans\u{0007}serif",
            16.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("制御文字を含む family は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn new_rejects_oversized_family() {
        let family = "a".repeat(MAX_FAMILY_LEN + 1);
        let err = TypographyValue::new(
            family,
            16.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("上限を超える family は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn new_accepts_family_at_max_len() {
        let family = "a".repeat(MAX_FAMILY_LEN);
        let value = TypographyValue::new(
            family,
            16.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect("上限ちょうどの family は許容される");
        assert_eq!(value.family().len(), MAX_FAMILY_LEN);
    }

    #[test]
    fn new_rejects_nan_size() {
        let err = TypographyValue::new(
            "sans-serif",
            f32::NAN,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("NaN は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn new_rejects_infinite_size() {
        let err = TypographyValue::new(
            "sans-serif",
            f32::INFINITY,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("正の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));

        let err = TypographyValue::new(
            "sans-serif",
            f32::NEG_INFINITY,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("負の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn new_rejects_zero_size() {
        let err = TypographyValue::new(
            "sans-serif",
            0.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("0 は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn new_rejects_negative_size() {
        let err = TypographyValue::new(
            "sans-serif",
            -1.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("負値は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn typography_value_rejects_zero_line_height() {
        let err = TypographyValue::new(
            "sans-serif",
            16.0,
            0.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("0 は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn typography_value_rejects_negative_line_height() {
        let err = TypographyValue::new(
            "sans-serif",
            16.0,
            -1.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("負値は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn typography_value_rejects_nan_line_height() {
        let err = TypographyValue::new(
            "sans-serif",
            16.0,
            f32::NAN,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("NaN は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn typography_value_rejects_infinite_line_height() {
        let err = TypographyValue::new(
            "sans-serif",
            16.0,
            f32::INFINITY,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));

        let err = TypographyValue::new(
            "sans-serif",
            16.0,
            f32::NEG_INFINITY,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect_err("負の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn font_weight_accepts_boundary_values() {
        assert_eq!(
            FontWeightValue::new(100).expect("100 は最小許容値").value(),
            100
        );
        assert_eq!(
            FontWeightValue::new(900).expect("900 は最大許容値").value(),
            900
        );
    }

    #[test]
    fn font_weight_rejects_below_minimum() {
        let err = FontWeightValue::new(99).expect_err("99 は域外として拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn font_weight_rejects_above_maximum() {
        let err = FontWeightValue::new(901).expect_err("901 は域外として拒否される");
        assert!(matches!(err, TokenError::InvalidTypography { .. }));
    }

    #[test]
    fn font_weight_named_constants_match_expected_values() {
        assert_eq!(FontWeightValue::THIN.value(), 100);
        assert_eq!(FontWeightValue::REGULAR.value(), 400);
        assert_eq!(FontWeightValue::BOLD.value(), 700);
        assert_eq!(FontWeightValue::BLACK.value(), 900);
    }

    #[test]
    fn font_style_variants_are_distinct() {
        assert_ne!(FontStyleValue::Normal, FontStyleValue::Italic);
        assert_eq!(FontStyleValue::Normal, FontStyleValue::Normal);
    }

    #[test]
    fn typography_token_set_builds_without_error() {
        let set = typography_token_set().expect("既定値のみからの構築は必ず成功する");
        assert_eq!(set.len(), 6);
    }

    #[test]
    fn typography_token_set_has_no_duplicate_keys() {
        let set = typography_token_set().expect("構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn typography_token_set_all_values_are_typography_category() {
        let set = typography_token_set().expect("構築は成功する");
        for definition in set.values() {
            assert_eq!(
                definition.category(),
                super::super::schema::TokenCategory::Typography
            );
        }
    }

    #[test]
    fn typography_token_set_body_uses_default_family_and_body_scale() {
        let set = typography_token_set().expect("構築は成功する");
        let path = TokenPath::from_dotted("typography.body").expect("パスは有効");
        let definition = set.get(&path).expect("typography.body は存在する");
        match definition {
            TokenDefinition::Value {
                value: TokenValue::Typography(value),
            } => {
                assert_eq!(value.family(), family::DEFAULT);
                assert_eq!(value.size(), scale::BODY);
            }
            other => panic!("typography.body は Typography 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn token_value_typography_category_is_typography() {
        let value = TypographyValue::new(
            "sans-serif",
            16.0,
            24.0,
            FontWeightValue::REGULAR,
            FontStyleValue::Normal,
        )
        .expect("正当な値は構築できる");
        let token_value = super::super::schema::TokenValue::Typography(value);
        assert_eq!(
            token_value.category(),
            super::super::schema::TokenCategory::Typography
        );
    }
}
