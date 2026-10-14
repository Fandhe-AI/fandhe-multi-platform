//! トークン解決エンジンが依存するスキーマ型。
//!
//! 各バリアントの担当カテゴリと `docs/architecture/design-token-architecture.md`
//! §4.1・§4.3・§5.2 の対応は次のとおり: `Color`（[`super::color::ColorValue`]、
//! TASK-8.4-S6、#162）、`Typography`（[`super::typography::TypographyValue`]、
//! フォントファミリ・サイズ・行間・ウェイト・スタイルを統合した値型、
//! TASK-8.4-S8/S9、#164・#165）、`Spacing`（[`super::spacing::SpacingValue`]、
//! TASK-8.4-S10、#166）、`Size` / `Radius` / `Elevation`
//! （[`super::dimension::SizeValue`] / [`super::dimension::RadiusValue`] /
//! [`super::dimension::ElevationValue`]、TASK-8.4-S11、#167）、`Border` /
//! `Shadow`（[`super::border::BorderValue`] / [`super::shadow::ShadowValue`]、
//! TASK-8.4-S12、#168）、`Duration` / `Animation`
//! （[`super::animation::DurationValue`] / [`super::animation::AnimationValue`]、
//! TASK-8.4-S13、#169）。
//!
//! `Dimension` / `Text` は暫定バリアントとして残置し、`Size` / `Radius` /
//! `Elevation` の専用カテゴリとは分離する（型不一致検出のためカテゴリを分離する
//! 必要があり、既に他 sub-issue が使用中の可能性があるため削除・整理は対象外）。
//!
//! `TokenValue::as_spacing` / `as_size` / `as_radius`（TASK-8.4-S15、#171）は
//! `ui/` 配下のコンテナ・レイアウト配線（`Stack::with_spacing_token` 等）向けの
//! 最小ブリッジであり、`TokenResolver::resolve` の解決結果から型安全に値を
//! 取り出す。文字列キーの実行時解決は持たない。

use std::collections::HashMap;

use super::animation::{AnimationValue, DurationValue};
use super::border::BorderValue;
use super::color::ColorValue;
use super::dimension::{ElevationValue, RadiusValue, SizeValue};
use super::error::TokenError;
use super::shadow::ShadowValue;
use super::spacing::SpacingValue;
use super::typography::TypographyValue;

/// `TokenPath` の構築時に許可する文字集合を判定する。
///
/// 設計文書 §3.2 が S17（外部定義ファイル読み込み）のパース契約として定める
/// 「英数字・`-`・`_` のみ」を、型の不変条件としても最小限保証するために使う
/// （本格的な構文パース・DTCG 準拠度決定は S17 のスコープ）。
fn is_allowed_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// トークンキーの階層パス（例: `color.primary` → `["color", "primary"]`）。
///
/// 設計文書 §3.1 が定める「生の `String` を解決エンジンの境界を越えて受け渡さない」
/// 契約に従い、セグメント列を保持するパース済み表現として保持する。外部シリアライズ
/// 表現（`{group.token}` 構文）の本格的なパースは S17（#173）のスコープであり、本型は
/// S17 が生成する構造化データの受け皿、または S3 内部でのテスト用構築手段として
/// 存在する。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenPath(Vec<String>);

impl TokenPath {
    /// セグメント列から `TokenPath` を構築する。
    ///
    /// 設計文書 §3.2 の契約（許可文字集合・`..` セグメント拒否）を、型の
    /// 不変条件として検証する。パス長・セグメント数の上限は S17 が実装時に
    /// 決定するスコープのため、本コンストラクタでは検証しない（§3.2「上限の
    /// 具体値は S17 が実装時に決定」）。
    ///
    /// # Errors
    ///
    /// セグメント列が空、いずれかのセグメントが空文字列・許可文字集合外の文字を
    /// 含む・`..` に一致する場合に `TokenError::InvalidPath` を返す。
    pub fn new(segments: Vec<String>) -> Result<Self, TokenError> {
        if segments.is_empty() {
            return Err(TokenError::InvalidPath {
                reason: "トークンパスは 1 つ以上のセグメントを持つ必要があります".to_string(),
            });
        }
        for segment in &segments {
            if segment.is_empty() {
                return Err(TokenError::InvalidPath {
                    reason: "空のセグメントを含むトークンパスです".to_string(),
                });
            }
            if segment == ".." {
                return Err(TokenError::InvalidPath {
                    reason: format!("パス走査に相当するセグメントは許可されません: {segment}"),
                });
            }
            if !segment.chars().all(is_allowed_segment_char) {
                return Err(TokenError::InvalidPath {
                    reason: format!(
                        "許可文字集合（英数字・-・_）外の文字を含むセグメントです: {segment}"
                    ),
                });
            }
        }
        Ok(Self(segments))
    }

    /// ドット区切りの単一文字列から構築する簡易コンストラクタ（テスト・サンプル用）。
    ///
    /// 本格的な参照文字列パース（DTCG `{group.token}` 構文からの変換）は S17 の
    /// スコープであり、本メソッドは単純なドット分割のみを行う簡易版に留まる。
    pub fn from_dotted(path: &str) -> Result<Self, TokenError> {
        Self::new(path.split('.').map(str::to_string).collect())
    }

    /// パスを構成するセグメント列を参照で返す。
    pub fn segments(&self) -> &[String] {
        &self.0
    }
}

impl std::fmt::Display for TokenPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join("."))
    }
}

/// `TokenPath` の newtype（設計文書 §4「型シグネチャ案」）。
///
/// 参照先を指し示す構造化データであり、解決エンジン内部・テスト・S19 の DSL 経由の
/// 参照は一貫して本型を介して行う（生文字列を解決エンジン本体へ渡さない、§3.1）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenRef(TokenPath);

impl TokenRef {
    /// パースの済んだ `TokenPath` から `TokenRef` を構築する。
    pub fn new(path: TokenPath) -> Self {
        Self(path)
    }

    /// 参照先のトークンパスを返す。
    pub fn path(&self) -> &TokenPath {
        &self.0
    }
}

impl std::fmt::Display for TokenRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// トークンのカテゴリ（型不一致検出の根拠、設計文書 §4.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenCategory {
    /// 色トークン（例: `color.primary`）。
    Color,
    /// 寸法トークン（例: `spacing.md`）。
    Dimension,
    /// 文字列トークン（例: フォントファミリー名）。
    Text,
    /// タイポグラフィトークン（例: `typography.body`、設計文書 §4.1・§4.3。
    /// TASK-8.4-S8、#164、残余フィールドは TASK-8.4-S9、#165）。
    Typography,
    /// スペーシングトークン（例: `spacing.md`。TASK-8.4-S10、#166）。
    Spacing,
    /// サイズトークン（例: `size.md`、設計文書 §4.1・§4.3。TASK-8.4-S11、
    /// #167。論理ピクセル相当の [`SizeValue`]）。
    Size,
    /// 角丸（radius）トークン（例: `radius.md`、設計文書 §4.1・§4.3。
    /// TASK-8.4-S11、#167。論理ピクセル相当の [`RadiusValue`]）。
    Radius,
    /// 高さ（elevation、z 軸的な階層表現）トークン（例: `elevation.raised`、
    /// 設計文書 §4.1・§4.3。TASK-8.4-S11、#167。実際の陰影表現は
    /// shadow カテゴリ（S12、#168）と合成する契約）。
    Elevation,
    /// ボーダートークン（例: `border.default`、設計文書 §4.1・§4.3。
    /// TASK-8.4-S12、#168）。
    Border,
    /// シャドウトークン（例: `shadow.sm`、設計文書 §4.1・§4.3。
    /// TASK-8.4-S12、#168）。
    Shadow,
    /// duration トークン（例: `duration.normal`。TASK-8.4-S13、#169）。
    Duration,
    /// アニメーショントークン（例: `animation.standard`。duration + easing の
    /// 組み合わせ。TASK-8.4-S13、#169）。
    Animation,
}

impl TokenCategory {
    /// 全バリアントを宣言順で列挙した配列。
    ///
    /// TASK-8.4-S20（#176、トークンドキュメント自動生成）の `token::docgen` が、
    /// 生成文書のカテゴリ一覧・命名規則要約をこの配列から機械的に導出するために
    /// 使う（enum とドキュメントの二重管理を避ける契約）。バリアント追加時は
    /// 本配列も更新する必要があり、`all_covers_every_variant_exactly_once`
    /// テストが要素数・重複なしを検証する。
    pub const ALL: [TokenCategory; 12] = [
        Self::Color,
        Self::Dimension,
        Self::Text,
        Self::Typography,
        Self::Spacing,
        Self::Size,
        Self::Radius,
        Self::Elevation,
        Self::Border,
        Self::Shadow,
        Self::Duration,
        Self::Animation,
    ];

    /// エラーメッセージ・型不一致検出に使う人間可読な名前を返す。
    pub fn name(self) -> &'static str {
        match self {
            Self::Color => "Color",
            Self::Dimension => "Dimension",
            Self::Text => "Text",
            Self::Typography => "Typography",
            Self::Spacing => "Spacing",
            Self::Size => "Size",
            Self::Radius => "Radius",
            Self::Elevation => "Elevation",
            Self::Border => "Border",
            Self::Shadow => "Shadow",
            Self::Duration => "Duration",
            Self::Animation => "Animation",
        }
    }
}

/// カテゴリ対応の型付き値（設計文書 §2「バリアントを跨いだ暗黙変換を行わない」）。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenValue {
    /// 色値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S6、#162）。
    Color(ColorValue),
    /// 数値の寸法値。
    Dimension(f64),
    /// 文字列値。
    Text(String),
    /// タイポグラフィ値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S8、
    /// #164（フォントファミリ・サイズ）と TASK-8.4-S9、#165（行間・
    /// ウェイト・スタイル）を統合した完全な値型）。
    Typography(TypographyValue),
    /// スペーシング値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S10、#166）。
    Spacing(SpacingValue),
    /// サイズ値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S11、#167。
    /// `geometry.rs` の `Point` / `Size` と単位系を揃える論理ピクセル
    /// 相当の f32）。
    Size(SizeValue),
    /// 角丸（radius）値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S11、
    /// #167。`SizeValue` と同じ `DimensionValue` を型エイリアスとして
    /// 共有するが、`TokenValue` バリアントとして分離することで型不一致検出
    /// （`Size` と `Radius` の取り違え）を可能にする）。
    Radius(RadiusValue),
    /// 高さ（elevation）値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S11、
    /// #167。整数の階層値。実際の陰影表現は shadow カテゴリ
    /// （S12、#168）と合成する）。
    Elevation(ElevationValue),
    /// ボーダー値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S12、#168）。
    Border(BorderValue),
    /// シャドウ値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S12、#168）。
    Shadow(ShadowValue),
    /// duration 値（設計文書 §4.3・§5.2 準拠の専用値型、TASK-8.4-S13、#169）。
    Duration(DurationValue),
    /// アニメーション値（duration + easing の組み合わせ、設計文書 §4.3・§5.2
    /// 準拠の専用値型、TASK-8.4-S13、#169）。
    Animation(AnimationValue),
}

impl TokenValue {
    /// この値が属するカテゴリを返す。
    pub fn category(&self) -> TokenCategory {
        match self {
            Self::Color(_) => TokenCategory::Color,
            Self::Dimension(_) => TokenCategory::Dimension,
            Self::Text(_) => TokenCategory::Text,
            Self::Typography(_) => TokenCategory::Typography,
            Self::Spacing(_) => TokenCategory::Spacing,
            Self::Size(_) => TokenCategory::Size,
            Self::Radius(_) => TokenCategory::Radius,
            Self::Elevation(_) => TokenCategory::Elevation,
            Self::Border(_) => TokenCategory::Border,
            Self::Shadow(_) => TokenCategory::Shadow,
            Self::Duration(_) => TokenCategory::Duration,
            Self::Animation(_) => TokenCategory::Animation,
        }
    }

    /// `Spacing` バリアントであれば内包する [`SpacingValue`] を返す（TASK-8.4-S15、
    /// #171）。
    ///
    /// `TokenResolver::resolve` の解決結果（[`super::resolver::ResolvedValue`]）から
    /// `ui/` 配下のコンテナ・レイアウト配線（`Stack::with_spacing_token` 等）へ
    /// 型安全に値を受け渡すための最小ブリッジ。文字列キーの実行時解決を新設せず、
    /// 型付き `Option` のみで判別する（S19、#175 の型付き参照 API を先取りしない
    /// 範囲に留める、設計文書 §7 の方針）。
    pub fn as_spacing(&self) -> Option<SpacingValue> {
        match self {
            Self::Spacing(value) => Some(*value),
            _ => None,
        }
    }

    /// `Size` バリアントであれば内包する [`SizeValue`] を返す（[`Self::as_spacing`]
    /// と同契約）。`SizeValue` / `RadiusValue` は同一の `DimensionValue` を型
    /// エイリアスとして共有するため、カテゴリの取り違え検出はバリアント一致
    /// （`Self::Size` か `Self::Radius` か）でのみ行う。
    pub fn as_size(&self) -> Option<SizeValue> {
        match self {
            Self::Size(value) => Some(*value),
            _ => None,
        }
    }

    /// `Radius` バリアントであれば内包する [`RadiusValue`] を返す（[`Self::as_size`]
    /// と同契約）。
    pub fn as_radius(&self) -> Option<RadiusValue> {
        match self {
            Self::Radius(value) => Some(*value),
            _ => None,
        }
    }
}

/// トークン定義（値そのもの、または他トークンへの参照）。
///
/// 参照エントリの期待型を表現し `TokenError::TypeMismatch` の検出を可能にする
/// （設計文書 §4.4）。`Value` はカテゴリを別フィールドとして持たず常に
/// `value.category()` から導出する（宣言カテゴリと実値の
/// カテゴリが食い違う不整合な定義を型として構築不能にするため。`Reference` は
/// 参照先を実際に解決するまで実値のカテゴリが分からないため、参照元が期待する
/// カテゴリを明示的なフィールドとして保持する）。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenDefinition {
    /// 直接値を持つ定義。カテゴリは常に `value.category()` に一致する。
    Value {
        /// 実値。
        value: TokenValue,
    },
    /// 他トークンへの参照を持つ定義（エイリアス）。
    Reference {
        /// この参照元が期待するカテゴリ。
        category: TokenCategory,
        /// 参照先のトークンパス。
        reference: TokenRef,
    },
}

impl TokenDefinition {
    /// この定義が宣言するカテゴリを返す。
    pub fn category(&self) -> TokenCategory {
        match self {
            Self::Value { value } => value.category(),
            Self::Reference { category, .. } => *category,
        }
    }
}

/// トークンパス → トークン定義のマップ（ベーストークンセット・テーマ差分の
/// いずれも同一構造で表現する、設計文書 §3.3）。
pub type TokenSet = HashMap<TokenPath, TokenDefinition>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::dimension::DimensionValue;
    use crate::token::spacing::SpacingValue;

    #[test]
    fn as_spacing_returns_some_for_spacing_variant() {
        let value = TokenValue::Spacing(SpacingValue::new(16.0).expect("正当な値"));
        assert_eq!(value.as_spacing().map(SpacingValue::value), Some(16.0));
    }

    #[test]
    fn as_spacing_returns_none_for_non_spacing_variant() {
        let value = TokenValue::Size(DimensionValue::new(16.0).expect("正当な値"));
        assert_eq!(value.as_spacing(), None);
    }

    #[test]
    fn as_size_returns_some_for_size_variant() {
        let value = TokenValue::Size(DimensionValue::new(32.0).expect("正当な値"));
        assert_eq!(value.as_size().map(DimensionValue::value), Some(32.0));
    }

    #[test]
    fn as_size_returns_none_for_radius_variant_type_mismatch_is_detected() {
        // Size / Radius は同一の DimensionValue を共有するため、値そのものでは
        // 取り違えを検出できない。バリアント一致で判別できることを確認する
        // （実装計画のテスト観点「Size と Radius の取り違え検出」）。
        let value = TokenValue::Radius(DimensionValue::new(8.0).expect("正当な値"));
        assert_eq!(value.as_size(), None);
        assert_eq!(value.as_radius().map(DimensionValue::value), Some(8.0));
    }

    #[test]
    fn as_radius_returns_none_for_non_radius_variant() {
        let value = TokenValue::Elevation(super::super::dimension::ElevationValue::new(2));
        assert_eq!(value.as_radius(), None);
    }

    #[test]
    fn all_covers_every_variant_exactly_once() {
        // `TokenCategory::ALL` は S20（#176）のドキュメント自動生成が唯一の
        // 真実源として使う配列であり、バリアント追加時の更新漏れを検出する
        // ため要素数と重複なしの両方を検証する。
        assert_eq!(TokenCategory::ALL.len(), 12);
        let names: std::collections::HashSet<&str> =
            TokenCategory::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), TokenCategory::ALL.len());
    }

    // 以下、`TokenPath` / `TokenValue` / `TokenDefinition` の単体テスト
    // （TASK-8.4-S21、#177）。`..`・許可文字集合外（`a/b` 等）の拒否は
    // `resolver.rs::invalid_segment_rejects_token_path_construction` と
    // `tests/token_resolver.rs::invalid_reference_syntax_rejected_at_construction`
    // で既に固定化済みのため重複させず、ここでは空セグメント列・空セグメント・
    // 非 ASCII 文字・`from_dotted` のドット位置異常・`Display` の往復のみを
    // 対象とする。

    #[test]
    fn token_path_new_rejects_invalid_inputs() {
        let err = TokenPath::new(Vec::new()).expect_err("空のセグメント列は拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));

        let err = TokenPath::new(vec!["".to_string()]).expect_err("空セグメントは拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));

        let err = TokenPath::new(vec!["いろ".to_string()])
            .expect_err("非 ASCII 文字を含むセグメントは拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));
    }

    #[test]
    fn token_path_new_accepts_allowed_charset() {
        let path = TokenPath::new(vec!["color-primary_01".to_string(), "Body".to_string()])
            .expect("英数字・-・_ のみのセグメントは受理される");
        assert_eq!(path.segments(), &["color-primary_01", "Body"]);
    }

    #[test]
    fn from_dotted_splits_and_rejects_empty_segments() {
        let path = TokenPath::from_dotted("a.b.c").expect("正当なドット区切りは受理される");
        assert_eq!(path.segments(), &["a", "b", "c"]);

        for invalid in [".a", "a.", "a..b", ""] {
            let err = TokenPath::from_dotted(invalid)
                .expect_err(&format!("ドット位置が不正な入力は拒否される: {invalid}"));
            assert!(matches!(err, TokenError::InvalidPath { .. }));
        }
    }

    #[test]
    fn token_path_display_roundtrip() {
        let path = TokenPath::from_dotted("color.primary").expect("正当なパス");
        assert_eq!(path.to_string(), "color.primary");

        let token_ref = TokenRef::new(path);
        assert_eq!(token_ref.to_string(), "color.primary");
    }

    #[test]
    fn token_value_and_definition_category_derivation() {
        // `TokenValue::category` が各バリアントに正しく対応すること、および
        // `TokenDefinition::Value` は値からカテゴリを導出し、`Reference` は
        // 宣言カテゴリ（実際の参照先とは無関係）を返すことを確認する。
        assert_eq!(
            TokenValue::Text("sans-serif".to_string()).category(),
            TokenCategory::Text
        );
        assert_eq!(
            TokenValue::Dimension(4.0).category(),
            TokenCategory::Dimension
        );

        let value_definition = TokenDefinition::Value {
            value: TokenValue::Text("value".to_string()),
        };
        assert_eq!(value_definition.category(), TokenCategory::Text);

        let reference_definition = TokenDefinition::Reference {
            category: TokenCategory::Dimension,
            reference: TokenRef::new(
                TokenPath::from_dotted("some.other.token").expect("正当なパス"),
            ),
        };
        assert_eq!(reference_definition.category(), TokenCategory::Dimension);
    }
}
