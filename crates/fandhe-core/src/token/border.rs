//! ボーダートークンの専用値型・既定トークンセット定義（TASK-8.4-S12、#168）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4.1・
//! §4.3・§5.2 が定めるボーダートークンの値モデルを実装契約として引き継ぐ。
//! 同文書 §4.1 の割り当て表により border カテゴリは S12（#168）が単独で担う。
//!
//! [`BorderValue`] は `TypographyValue`（S8/S9、#164/#165）が確立した「専用値型 +
//! 検証付きコンストラクタ + アクセサ」というパターンを踏襲する。色フィールドは
//! 直接 [`super::color::ColorValue`] を持たず、設計文書 §4.4「色を要求するフィールドは
//! color カテゴリのトークンのみを参照できる」という制約に従い [`super::schema::TokenRef`]
//! として保持する（`semantic_color::semantic_color_entry` と同様、参照先の実カテゴリは
//! 構築時点では未解決のため、解決ヘルパー [`resolve_border_color`] で検証する）。
//!
//! プラットフォーム別の具体値調整（iOS/Android 準拠）は S16（#172）のスコープであり、
//! 本モジュールの既定値はフレームワーク中立の暫定値に留める。外部定義ファイル読み込み
//! （DTCG 等の構文パース）は S17（#173）のスコープ、AI 型安全 DSL からの型付き参照 API は
//! S19（#175）のスコープ。いずれも本モジュールは検証済みの `BorderValue` を受け渡す
//! データ表現としてのみ関与する。

use super::error::TokenError;
use super::resolver::TokenResolver;
use super::schema::{TokenCategory, TokenDefinition, TokenPath, TokenRef, TokenSet, TokenValue};

/// ボーダースタイル（閉じた列挙、設計文書 §5.2 の `BorderStyleValue`）。
///
/// `Solid` / `Dashed` の 2 値に閉じる（点線以外の拡張は設計文書のスコープ外）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BorderStyleValue {
    /// 実線。
    Solid,
    /// 破線。
    Dashed,
}

/// ボーダートークンの値（設計文書 §4.3・§5.2 準拠）。
///
/// フィールドは非公開とし、[`BorderValue::new`] による検証付き構築とアクセサ
/// （[`BorderValue::width`] / [`BorderValue::style`] / [`BorderValue::color`]）経由
/// でのみ値へアクセスできる（`TypographyValue` と同じ判断。`width` に非有限値・
/// 負値という不変条件を持つため、構築経路を型で閉じる）。
#[derive(Debug, Clone, PartialEq)]
pub struct BorderValue {
    width: f32,
    style: BorderStyleValue,
    color: TokenRef,
}

impl BorderValue {
    /// 全フィールドを検証したうえで構築する。
    ///
    /// `width` は `0.0`（枠線なし）を許容しつつ、非有限値・負値は後段のレイアウト・
    /// 描画計算（taffy 等）への異常値伝播を防ぐため拒否する（`TypographyValue::new` の
    /// `size` 検証と同じ DoS 耐性観点、security.md）。`color` は構築時点では参照先の
    /// 実カテゴリを解決できないため、色カテゴリであることの検証は行わない
    /// （[`resolve_border_color`] が解決時に検証する、設計文書 §4.4）。
    ///
    /// # Errors
    ///
    /// 以下のいずれかに該当する場合 [`TokenError::InvalidBorder`] を返す。
    /// - `width` が非有限（`NaN` / `±inf`）
    /// - `width` が `0.0` 未満
    pub fn new(width: f32, style: BorderStyleValue, color: TokenRef) -> Result<Self, TokenError> {
        if !width.is_finite() {
            return Err(TokenError::InvalidBorder {
                reason: format!("幅は有限の数値である必要があります: {width}"),
            });
        }
        if width < 0.0 {
            return Err(TokenError::InvalidBorder {
                reason: format!("幅は 0 以上である必要があります: {width}"),
            });
        }

        Ok(Self {
            width,
            style,
            color,
        })
    }

    /// 幅（論理ピクセル相当、`0.0` は枠線なしを表す）を返す。
    pub fn width(&self) -> f32 {
        self.width
    }

    /// スタイルを返す。
    pub fn style(&self) -> BorderStyleValue {
        self.style
    }

    /// 色トークンへの参照を返す。
    pub fn color(&self) -> &TokenRef {
        &self.color
    }
}

/// ボーダーの色参照を解決エンジン経由で具体値まで解決する。
///
/// `resolver.resolve` の結果が `TokenValue::Color` であることを検証し、不一致は
/// `TokenError::TypeMismatch` として報告する（設計文書 §4.4「色を要求するフィールドは
/// color カテゴリのトークンのみを参照できる」という型制約の実行時検証、
/// `semantic_color::resolve_semantic_color` と同じ「安全網」設計）。戻り値は
/// `ColorValue` を直接晒さず `TokenValue` のまま返し、色の内部表現の変更が呼び出し元へ
/// 波及する範囲を局所化する。
///
/// `BorderValue` は自身が属するトークンパス（`TokenPath`）を保持しないため、
/// `TypeMismatch::origin`（参照を保持しているトークン自身）を厳密には特定できない。
/// 呼び出しコンテキストが分かるよう `origin` には固定文字列 `"border.color()"` を、
/// `reference`（実際の参照先パス）には `border.color()` の参照先を設定し、両者を
/// 診断情報として区別できるようにする（`semantic_color::resolve_semantic_color` の
/// ように参照元パスを保持する設計とは異なる）。
///
/// # Errors
///
/// 参照先が未定義の場合は `resolver.resolve` の `TokenError::UndefinedReference` を、
/// 参照先が color カテゴリ以外の場合は `TokenError::TypeMismatch` をそのまま伝播する。
pub fn resolve_border_color(
    resolver: &TokenResolver,
    border: &BorderValue,
) -> Result<TokenValue, TokenError> {
    let resolved = resolver.resolve(border.color())?;
    let value = resolved.value().clone();
    match value.category() {
        TokenCategory::Color => Ok(value),
        actual => Err(TokenError::TypeMismatch {
            origin: "border.color()".to_string(),
            reference: border.color().to_string(),
            expected: TokenCategory::Color.name(),
            actual: actual.name(),
        }),
    }
}

/// 既定のボーダートークン全件を束ねた `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`border.<name>`）に従い、検証付きコンストラクタ
/// `TokenPath::from_dotted` 経由でのみ生成する（`color::palette_token_set` /
/// `typography::typography_token_set` と同じ契約、生文字列を解決エンジンの境界へ
/// 渡さない、`token_resolution-architecture.md` §3.1）。色参照パスも同様に検証済み
/// リテラルのみから生成する。返す `TokenSet` は `color::palette_token_set()` と合成
/// （マージ）した場合にのみ色参照が解決可能になる（色定義自体は本関数に含まない）。
///
/// # Errors
///
/// パス構築・`BorderValue::new` のいずれかが失敗した場合にそのまま伝播する
/// （既定値のみからの構築が失敗することは通常ない）。
pub fn border_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, f32, BorderStyleValue, &str)] = &[
        (
            "border.default",
            1.0,
            BorderStyleValue::Solid,
            "color.palette.gray-300",
        ),
        (
            "border.strong",
            2.0,
            BorderStyleValue::Solid,
            "color.palette.gray-500",
        ),
        (
            "border.dashed",
            1.0,
            BorderStyleValue::Dashed,
            "color.palette.gray-300",
        ),
    ];

    let mut set = TokenSet::new();
    for (key, width, style, color_key) in entries {
        let path = TokenPath::from_dotted(key)?;
        let color_ref = TokenRef::new(TokenPath::from_dotted(color_key)?);
        let value = BorderValue::new(*width, *style, color_ref)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Border(value),
            },
        );
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::super::color::{palette, palette_token_set};
    use super::*;

    fn path(s: &str) -> TokenPath {
        TokenPath::from_dotted(s).expect("テスト用の正当なパス")
    }

    fn color_ref(s: &str) -> TokenRef {
        TokenRef::new(path(s))
    }

    #[test]
    fn new_succeeds_with_valid_values() {
        let value = BorderValue::new(1.0, BorderStyleValue::Solid, color_ref("color.primary"))
            .expect("正当な値の構築は成功する");
        assert_eq!(value.width(), 1.0);
        assert_eq!(value.style(), BorderStyleValue::Solid);
        assert_eq!(value.color().to_string(), "color.primary");
    }

    #[test]
    fn new_accepts_zero_width_as_no_border() {
        let value = BorderValue::new(0.0, BorderStyleValue::Solid, color_ref("color.primary"))
            .expect("0 は枠線なしとして許容される");
        assert_eq!(value.width(), 0.0);
    }

    #[test]
    fn new_rejects_negative_width() {
        let err = BorderValue::new(-1.0, BorderStyleValue::Solid, color_ref("color.primary"))
            .expect_err("負値は拒否される");
        assert!(matches!(err, TokenError::InvalidBorder { .. }));
    }

    #[test]
    fn new_rejects_nan_width() {
        let err = BorderValue::new(
            f32::NAN,
            BorderStyleValue::Solid,
            color_ref("color.primary"),
        )
        .expect_err("NaN は拒否される");
        assert!(matches!(err, TokenError::InvalidBorder { .. }));
    }

    #[test]
    fn new_rejects_infinite_width() {
        let err = BorderValue::new(
            f32::INFINITY,
            BorderStyleValue::Solid,
            color_ref("color.primary"),
        )
        .expect_err("正の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidBorder { .. }));

        let err = BorderValue::new(
            f32::NEG_INFINITY,
            BorderStyleValue::Solid,
            color_ref("color.primary"),
        )
        .expect_err("負の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidBorder { .. }));
    }

    #[test]
    fn border_style_variants_are_distinct() {
        assert_ne!(BorderStyleValue::Solid, BorderStyleValue::Dashed);
        assert_eq!(BorderStyleValue::Solid, BorderStyleValue::Solid);
    }

    #[test]
    fn border_token_set_builds_without_error() {
        let set = border_token_set().expect("既定値のみからの構築は必ず成功する");
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn border_token_set_has_no_duplicate_keys() {
        let set = border_token_set().expect("構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn border_token_set_all_values_are_border_category() {
        let set = border_token_set().expect("構築は成功する");
        for definition in set.values() {
            assert_eq!(definition.category(), TokenCategory::Border);
        }
    }

    #[test]
    fn resolve_border_color_resolves_to_palette_color() {
        let mut base = palette_token_set().expect("パレット構築は成功する");
        border_token_set()
            .expect("ボーダートークン構築は成功する")
            .into_iter()
            .for_each(|(k, v)| {
                base.insert(k, v);
            });

        let resolver = TokenResolver::new(base, TokenSet::new());
        let border = BorderValue::new(
            1.0,
            BorderStyleValue::Solid,
            color_ref("color.palette.gray-300"),
        )
        .expect("正当な値");
        let resolved =
            resolve_border_color(&resolver, &border).expect("色参照は color カテゴリへ解決できる");
        assert_eq!(resolved, TokenValue::Color(palette::GRAY_300));
    }

    #[test]
    fn resolve_border_color_type_mismatch_when_target_is_not_color() {
        let mut base = TokenSet::new();
        base.insert(
            path("spacing.md"),
            TokenDefinition::Value {
                value: TokenValue::Dimension(8.0),
            },
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let border = BorderValue::new(1.0, BorderStyleValue::Solid, color_ref("spacing.md"))
            .expect("正当な値");
        let err = resolve_border_color(&resolver, &border)
            .expect_err("色以外のカテゴリを参照した場合は型不一致になる");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }
}
