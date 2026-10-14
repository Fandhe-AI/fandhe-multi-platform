//! シャドウトークンの専用値型・既定トークンセット定義（TASK-8.4-S12、#168）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4.1・
//! §4.3・§5.2 が定めるシャドウトークンの値モデルを実装契約として引き継ぐ。
//! 同文書 §4.1 の割り当て表により shadow カテゴリは S12（#168）が単独で担う。
//! 実際の陰影表現を elevation カテゴリと合成する処理（§4.3）は S11/S16 のスコープで
//! あり、本モジュールは shadow 単体の値モデルのみを実装する（elevation には依存しない）。
//!
//! [`ShadowValue`] は [`super::border::BorderValue`] と同じ「専用値型 + 検証付き
//! コンストラクタ + アクセサ」パターンを踏襲し、色フィールドも同様に
//! [`super::schema::TokenRef`] として保持する（設計文書 §4.4 の型制約は解決ヘルパー
//! [`resolve_shadow_color`] が実行時に検証する）。
//!
//! プラットフォーム別の具体値調整（iOS/Android 準拠）は S16（#172）のスコープであり、
//! 本モジュールの既定値はフレームワーク中立の暫定値に留める。半透明の影色（透過黒）を
//! 用いた本格的な既定テーマ調整も S16 のスコープであり、本モジュールの既定トークン
//! セットは不透明な `color.palette.black` を暫定的に参照する。

use super::error::TokenError;
use super::resolver::TokenResolver;
use super::schema::{TokenCategory, TokenDefinition, TokenPath, TokenRef, TokenSet, TokenValue};

/// シャドウトークンの値（設計文書 §4.3・§5.2 準拠）。
///
/// フィールドは非公開とし、[`ShadowValue::new`] による検証付き構築とアクセサ
/// （[`ShadowValue::offset_x`] / [`ShadowValue::offset_y`] / [`ShadowValue::blur_radius`] /
/// [`ShadowValue::color`]）経由でのみ値へアクセスできる（`BorderValue` と同じ判断）。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowValue {
    offset_x: f32,
    offset_y: f32,
    blur_radius: f32,
    color: TokenRef,
}

impl ShadowValue {
    /// 全フィールドを検証したうえで構築する。
    ///
    /// `offset_x` / `offset_y` は上方向・左方向の影表現に必要なため負値を許容するが、
    /// 非有限値（`NaN` / `±inf`）は後段のレイアウト・描画計算への異常値伝播を防ぐため
    /// 拒否する。`blur_radius` は非有限値に加えて負値も拒否する（ぼかし半径は物理的に
    /// 非負でなければ意味を持たないため、`TypographyValue::size` と同じ判断）。
    ///
    /// # Errors
    ///
    /// 以下のいずれかに該当する場合 [`TokenError::InvalidShadow`] を返す。
    /// - `offset_x` / `offset_y` が非有限（`NaN` / `±inf`）
    /// - `blur_radius` が非有限、または `0.0` 未満
    pub fn new(
        offset_x: f32,
        offset_y: f32,
        blur_radius: f32,
        color: TokenRef,
    ) -> Result<Self, TokenError> {
        if !offset_x.is_finite() {
            return Err(TokenError::InvalidShadow {
                reason: format!("X オフセットは有限の数値である必要があります: {offset_x}"),
            });
        }
        if !offset_y.is_finite() {
            return Err(TokenError::InvalidShadow {
                reason: format!("Y オフセットは有限の数値である必要があります: {offset_y}"),
            });
        }
        if !blur_radius.is_finite() {
            return Err(TokenError::InvalidShadow {
                reason: format!("ぼかし半径は有限の数値である必要があります: {blur_radius}"),
            });
        }
        if blur_radius < 0.0 {
            return Err(TokenError::InvalidShadow {
                reason: format!("ぼかし半径は 0 以上である必要があります: {blur_radius}"),
            });
        }

        Ok(Self {
            offset_x,
            offset_y,
            blur_radius,
            color,
        })
    }

    /// X オフセット（論理ピクセル相当。負値は左方向）を返す。
    pub fn offset_x(&self) -> f32 {
        self.offset_x
    }

    /// Y オフセット（論理ピクセル相当。負値は上方向）を返す。
    pub fn offset_y(&self) -> f32 {
        self.offset_y
    }

    /// ぼかし半径（論理ピクセル相当、`0.0` 以上）を返す。
    pub fn blur_radius(&self) -> f32 {
        self.blur_radius
    }

    /// 色トークンへの参照を返す。
    pub fn color(&self) -> &TokenRef {
        &self.color
    }
}

/// シャドウの色参照を解決エンジン経由で具体値まで解決する。
///
/// `border::resolve_border_color` と同型の安全網であり、`resolver.resolve` の結果が
/// `TokenValue::Color` であることを検証し、不一致は `TokenError::TypeMismatch` として
/// 報告する（設計文書 §4.4）。戻り値は `ColorValue` を直接晒さず `TokenValue` のまま返す。
///
/// `ShadowValue` は自身が属するトークンパス（`TokenPath`）を保持しないため、
/// `TypeMismatch::origin`（参照を保持しているトークン自身）を厳密には特定できない。
/// `border::resolve_border_color` と同じ判断で、`origin` には固定文字列
/// `"shadow.color()"` を、`reference`（実際の参照先パス）には `shadow.color()` の
/// 参照先を設定し、両者を診断情報として区別できるようにする。
///
/// # Errors
///
/// 参照先が未定義の場合は `resolver.resolve` の `TokenError::UndefinedReference` を、
/// 参照先が color カテゴリ以外の場合は `TokenError::TypeMismatch` をそのまま伝播する。
pub fn resolve_shadow_color(
    resolver: &TokenResolver,
    shadow: &ShadowValue,
) -> Result<TokenValue, TokenError> {
    let resolved = resolver.resolve(shadow.color())?;
    let value = resolved.value().clone();
    match value.category() {
        TokenCategory::Color => Ok(value),
        actual => Err(TokenError::TypeMismatch {
            origin: "shadow.color()".to_string(),
            reference: shadow.color().to_string(),
            expected: TokenCategory::Color.name(),
            actual: actual.name(),
        }),
    }
}

/// 既定のシャドウトークン全件（`shadow.sm` / `shadow.md` / `shadow.lg`）を束ねた
/// `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`shadow.<name>`）に従い、検証付きコンストラクタ
/// `TokenPath::from_dotted` 経由でのみ生成する（`border::border_token_set` と同じ契約）。
/// オフセット・ぼかし半径は段階的に増加させ、色は不透明な `color.palette.black` を
/// 暫定的に参照する（透明度付き影色の本格設計は S16 のスコープ、モジュール doc 参照）。
/// 返す `TokenSet` は `color::palette_token_set()` と合成した場合にのみ色参照が解決可能になる。
///
/// # Errors
///
/// パス構築・`ShadowValue::new` のいずれかが失敗した場合にそのまま伝播する
/// （既定値のみからの構築が失敗することは通常ない）。
pub fn shadow_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, f32, f32, f32)] = &[
        ("shadow.sm", 0.0, 1.0, 2.0),
        ("shadow.md", 0.0, 4.0, 8.0),
        ("shadow.lg", 0.0, 8.0, 16.0),
    ];

    let mut set = TokenSet::new();
    for (key, offset_x, offset_y, blur_radius) in entries {
        let path = TokenPath::from_dotted(key)?;
        let color_ref = TokenRef::new(TokenPath::from_dotted("color.palette.black")?);
        let value = ShadowValue::new(*offset_x, *offset_y, *blur_radius, color_ref)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Shadow(value),
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
        let value = ShadowValue::new(0.0, 4.0, 8.0, color_ref("color.primary"))
            .expect("正当な値の構築は成功する");
        assert_eq!(value.offset_x(), 0.0);
        assert_eq!(value.offset_y(), 4.0);
        assert_eq!(value.blur_radius(), 8.0);
        assert_eq!(value.color().to_string(), "color.primary");
    }

    #[test]
    fn new_accepts_negative_offsets() {
        let value = ShadowValue::new(-2.0, -3.0, 4.0, color_ref("color.primary"))
            .expect("負のオフセットは許容される");
        assert_eq!(value.offset_x(), -2.0);
        assert_eq!(value.offset_y(), -3.0);
    }

    #[test]
    fn new_rejects_negative_blur_radius() {
        let err = ShadowValue::new(0.0, 0.0, -1.0, color_ref("color.primary"))
            .expect_err("負のぼかし半径は拒否される");
        assert!(matches!(err, TokenError::InvalidShadow { .. }));
    }

    #[test]
    fn new_rejects_nan_offset() {
        let err = ShadowValue::new(f32::NAN, 0.0, 1.0, color_ref("color.primary"))
            .expect_err("NaN のオフセットは拒否される");
        assert!(matches!(err, TokenError::InvalidShadow { .. }));
    }

    #[test]
    fn new_rejects_infinite_blur_radius() {
        let err = ShadowValue::new(0.0, 0.0, f32::INFINITY, color_ref("color.primary"))
            .expect_err("無限大のぼかし半径は拒否される");
        assert!(matches!(err, TokenError::InvalidShadow { .. }));
    }

    #[test]
    fn shadow_token_set_builds_without_error() {
        let set = shadow_token_set().expect("既定値のみからの構築は必ず成功する");
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn shadow_token_set_has_no_duplicate_keys() {
        let set = shadow_token_set().expect("構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn shadow_token_set_all_values_are_shadow_category() {
        let set = shadow_token_set().expect("構築は成功する");
        for definition in set.values() {
            assert_eq!(definition.category(), TokenCategory::Shadow);
        }
    }

    #[test]
    fn resolve_shadow_color_resolves_to_palette_color() {
        let mut base = palette_token_set().expect("パレット構築は成功する");
        shadow_token_set()
            .expect("シャドウトークン構築は成功する")
            .into_iter()
            .for_each(|(k, v)| {
                base.insert(k, v);
            });

        let resolver = TokenResolver::new(base, TokenSet::new());
        let shadow =
            ShadowValue::new(0.0, 4.0, 8.0, color_ref("color.palette.black")).expect("正当な値");
        let resolved =
            resolve_shadow_color(&resolver, &shadow).expect("色参照は color カテゴリへ解決できる");
        assert_eq!(resolved, TokenValue::Color(palette::BLACK));
    }

    #[test]
    fn resolve_shadow_color_type_mismatch_when_target_is_not_color() {
        let mut base = TokenSet::new();
        base.insert(
            path("spacing.md"),
            TokenDefinition::Value {
                value: TokenValue::Dimension(8.0),
            },
        );
        let resolver = TokenResolver::new(base, TokenSet::new());
        let shadow = ShadowValue::new(0.0, 0.0, 1.0, color_ref("spacing.md")).expect("正当な値");
        let err = resolve_shadow_color(&resolver, &shadow)
            .expect_err("色以外のカテゴリを参照した場合は型不一致になる");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }
}
