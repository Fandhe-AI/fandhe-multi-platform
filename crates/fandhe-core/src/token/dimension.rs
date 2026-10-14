//! サイズ・radius・elevation トークンの専用値型・既定スケール定義
//! （TASK-8.4-S11、#167）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4.1 の
//! 割り当て表は「`size` / `radius` / `elevation` | サイズ・角丸・高さ（z 軸的な
//! 階層表現） | S11（#167）」と定めており、本モジュールがこの 3 カテゴリを
//! 実装する。§4.3 の値モデルの契約は次のとおり:
//! - size / radius: 論理ピクセル相当の f32（`ui::geometry` の `Point` / `Size`
//!   と単位系を揃える）
//! - elevation: 整数の階層値（0 から始まる段階）。実際の陰影表現は shadow
//!   カテゴリ（S12、#168）と合成する契約であり、本モジュールは階層順位のみを
//!   表現する
//!
//! `color::ColorValue` / `typography::TypographyValue` が確立した「専用値型 +
//! 検証付き構築 + 既定トークンセット関数」というパターンを踏襲する。
//!
//! 他 sub-issue との契約:
//! - S10（#166、spacing）: 設計文書 §5.2 は spacing / size / radius が
//!   `DimensionValue` を共有する想定を示すが、size / radius が本モジュールの
//!   `DimensionValue` を使うのに対し、spacing は独自の `SpacingValue`
//!   （`spacing.rs`）を使う（カテゴリ取り違えを型で検出するための分離）。
//! - S12（#168、border / shadow）: elevation の階層値と shadow の陰影表現を
//!   合成する。本モジュールは階層値の受け渡しのみを担う。
//! - S14/S15（#170/#171、ウィジェット配線）・S16（#172、プラットフォーム別
//!   既定テーマ）: 本モジュールが公開する既定スケール定数・トークンセットを
//!   消費する。
//! - S19（#175、AI 型安全 DSL）: `size_scale` / `radius_scale` /
//!   `elevation_level` モジュールの `pub const` を型付き参照の対象とする。

use super::error::TokenError;
use super::schema::{TokenDefinition, TokenPath, TokenSet, TokenValue};

/// サイズ・radius トークンの共有値型（設計文書 §5.2 の `DimensionValue` 案）。
///
/// フィールドは非公開とし、[`DimensionValue::new`] による検証付き構築と
/// アクセサ [`DimensionValue::value`] 経由でのみ値へアクセスできる
/// （`TypographyValue` と同じカプセル化方針）。単位は論理ピクセル相当であり、
/// `ui::geometry` の `Point` / `Size` と単位系を揃える契約（設計文書 §4.3）。
///
/// `size` / `radius` はカテゴリとしては別物だが値モデルは同一のため、
/// [`SizeValue`] / [`RadiusValue`] は本型への型エイリアスとして定義する
/// （設計文書 §5.2 の型シグネチャ案どおり）。カテゴリの区別は
/// `TokenValue::Size` / `TokenValue::Radius` のバリアントで担保され、
/// `TokenError::TypeMismatch` の検出が機能する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimensionValue(f32);

impl DimensionValue {
    /// 値を検証したうえで構築する。
    ///
    /// `0.0` は radius では「角丸なし」、size では「折り畳み」を表す有効値
    /// のため許容する。負値は size / radius いずれの値モデルとしても意味を
    /// 持たないため拒否する。
    ///
    /// # Errors
    ///
    /// `value` が非有限（`NaN` / `±inf`）、または負値の場合に
    /// [`TokenError::InvalidDimension`] を返す（後段のレイアウト計算・
    /// 描画バックエンドへの異常値伝播を防ぐ、security.md の DoS 耐性観点）。
    pub fn new(value: f32) -> Result<Self, TokenError> {
        if !value.is_finite() {
            return Err(TokenError::InvalidDimension {
                reason: format!("寸法値は有限の数値である必要があります: {value}"),
            });
        }
        if value < 0.0 {
            return Err(TokenError::InvalidDimension {
                reason: format!("寸法値は 0 以上である必要があります: {value}"),
            });
        }
        Ok(Self(value))
    }

    /// 内部で保持する数値表現（論理ピクセル相当）を返す。
    ///
    /// `const fn` 化（TASK-8.4-S15、#171）: `spacing::SpacingValue::value` と
    /// 同じ理由で定数コンテキストからの呼び出しを可能にする互換変更。
    pub const fn value(self) -> f32 {
        self.0
    }
}

/// サイズトークンの値型（設計文書 §5.2 の型エイリアス案）。
pub type SizeValue = DimensionValue;

/// 角丸（radius）トークンの値型（設計文書 §5.2 の型エイリアス案）。
pub type RadiusValue = DimensionValue;

/// 高さ（elevation）トークンの値（設計文書 §4.3・§5.2 準拠）。
///
/// 0 から始まる整数の階層値。`u8` を採用することで非負性を型として保証し、
/// `DimensionValue` と異なり検証は不要（infallible なコンストラクタ）。
/// 実際の陰影表現（ぼかし・色・オフセット）は shadow カテゴリ（S12、#168）が
/// 別途持ち、本値は階層順位・陰影の強さの示唆としてのみ機能する契約。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElevationValue {
    level: u8,
}

impl ElevationValue {
    /// 階層値から `ElevationValue` を構築する。
    ///
    /// `u8` の値域全体が正当な階層値であるため、本コンストラクタは
    /// infallible（`Result` を返さない）。
    pub fn new(level: u8) -> Self {
        Self { level }
    }

    /// 階層値を返す。
    pub fn level(self) -> u8 {
        self.level
    }
}

/// サイズスケール定数（論理ピクセル相当、アイコン・コントロール寸法の目安）。
///
/// AI 型安全 DSL（S19、#175）からの型付き参照を見据え、`pub const` として
/// 公開する（`typography::scale` と同方針）。プラットフォーム準拠の具体値
/// 調整は S16（#172）のスコープであり、ここではフレームワーク中立の既定値
/// のみを定める。
pub mod size_scale {
    /// 極小サイズ（小アイコン等）。
    pub const XS: f32 = 16.0;
    /// 小サイズ。
    pub const SM: f32 = 24.0;
    /// 中サイズ（既定のコントロール寸法の目安）。
    pub const MD: f32 = 32.0;
    /// 大サイズ。
    pub const LG: f32 = 48.0;
    /// 特大サイズ。
    pub const XL: f32 = 64.0;
}

/// 角丸（radius）スケール定数（論理ピクセル相当）。
pub mod radius_scale {
    /// 角丸なし。
    pub const NONE: f32 = 0.0;
    /// 小さい角丸。
    pub const SM: f32 = 4.0;
    /// 中程度の角丸。
    pub const MD: f32 = 8.0;
    /// 大きい角丸。
    pub const LG: f32 = 16.0;
    /// ピル形状用の実用上限値（矩形の短辺以上を指定すれば完全な丸みになる
    /// ため、上限として十分大きな値を定めておく）。
    pub const FULL: f32 = 9999.0;
}

/// 高さ（elevation）階層定数。
///
/// 段階は 0（`FLAT`）から始まる（設計文書 §4.3「0 から始まる段階」）。
pub mod elevation_level {
    /// 平面（陰影なし）。
    pub const FLAT: u8 = 0;
    /// わずかに持ち上がった要素（カード等）。
    pub const RAISED: u8 = 1;
    /// 浮遊する要素（FAB 等）。
    pub const FLOATING: u8 = 2;
    /// 常設のオーバーレイ要素。
    pub const OVERLAY: u8 = 3;
    /// モーダルダイアログ。
    pub const MODAL: u8 = 4;
    /// 最前面（トースト・トーストに準ずる通知等）。
    pub const TOP: u8 = 5;
}

/// 既定サイズスケール全件を束ねた `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`size.<スケール名>`）に従い、検証付き
/// コンストラクタ `TokenPath::from_dotted` 経由でのみ生成する
/// （`color::palette_token_set` と同じ契約、生文字列を解決エンジンの境界へ
/// 渡さない、`token_resolution-architecture.md` §3.1）。`unwrap`/`expect` を
/// 使わず `?` で伝播する（coding-rust.md の panic 禁止方針）。
pub fn size_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, f32)] = &[
        ("size.xs", size_scale::XS),
        ("size.sm", size_scale::SM),
        ("size.md", size_scale::MD),
        ("size.lg", size_scale::LG),
        ("size.xl", size_scale::XL),
    ];

    let mut set = TokenSet::new();
    for (key, value) in entries {
        let path = TokenPath::from_dotted(key)?;
        let dimension = DimensionValue::new(*value)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Size(dimension),
            },
        );
    }
    Ok(set)
}

/// 既定角丸スケール全件を束ねた `TokenSet` を構築する（`size_token_set` と
/// 同契約）。
pub fn radius_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, f32)] = &[
        ("radius.none", radius_scale::NONE),
        ("radius.sm", radius_scale::SM),
        ("radius.md", radius_scale::MD),
        ("radius.lg", radius_scale::LG),
        ("radius.full", radius_scale::FULL),
    ];

    let mut set = TokenSet::new();
    for (key, value) in entries {
        let path = TokenPath::from_dotted(key)?;
        let dimension = DimensionValue::new(*value)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Radius(dimension),
            },
        );
    }
    Ok(set)
}

/// 既定高さ階層全件を束ねた `TokenSet` を構築する（`size_token_set` と
/// 同契約）。`ElevationValue::new` が infallible なため本関数が失敗するのは
/// `TokenPath::from_dotted` のパース失敗時のみ。
pub fn elevation_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, u8)] = &[
        ("elevation.flat", elevation_level::FLAT),
        ("elevation.raised", elevation_level::RAISED),
        ("elevation.floating", elevation_level::FLOATING),
        ("elevation.overlay", elevation_level::OVERLAY),
        ("elevation.modal", elevation_level::MODAL),
        ("elevation.top", elevation_level::TOP),
    ];

    let mut set = TokenSet::new();
    for (key, level) in entries {
        let path = TokenPath::from_dotted(key)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Elevation(ElevationValue::new(*level)),
            },
        );
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::resolver::TokenResolver;
    use crate::token::schema::{TokenCategory, TokenRef};

    #[test]
    fn dimension_value_accepts_positive_value() {
        let value = DimensionValue::new(8.0).expect("正当な値の構築は成功する");
        assert_eq!(value.value(), 8.0);
    }

    #[test]
    fn dimension_value_accepts_zero() {
        let value = DimensionValue::new(0.0).expect("0 は角丸なし・折り畳みとして有効");
        assert_eq!(value.value(), 0.0);
    }

    #[test]
    fn dimension_value_rejects_negative() {
        let err = DimensionValue::new(-1.0).expect_err("負値は拒否される");
        assert!(matches!(err, TokenError::InvalidDimension { .. }));
    }

    #[test]
    fn dimension_value_rejects_nan() {
        let err = DimensionValue::new(f32::NAN).expect_err("NaN は拒否される");
        assert!(matches!(err, TokenError::InvalidDimension { .. }));
    }

    #[test]
    fn dimension_value_rejects_infinite() {
        let err = DimensionValue::new(f32::INFINITY).expect_err("正の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidDimension { .. }));

        let err = DimensionValue::new(f32::NEG_INFINITY).expect_err("負の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidDimension { .. }));
    }

    #[test]
    fn elevation_value_level_roundtrips() {
        let value = ElevationValue::new(3);
        assert_eq!(value.level(), 3);
    }

    #[test]
    fn token_value_size_category_is_size() {
        let value = DimensionValue::new(8.0).expect("正当な値は構築できる");
        let token_value = TokenValue::Size(value);
        assert_eq!(token_value.category(), TokenCategory::Size);
    }

    #[test]
    fn token_value_radius_category_is_radius() {
        let value = DimensionValue::new(8.0).expect("正当な値は構築できる");
        let token_value = TokenValue::Radius(value);
        assert_eq!(token_value.category(), TokenCategory::Radius);
    }

    #[test]
    fn token_value_elevation_category_is_elevation() {
        let token_value = TokenValue::Elevation(ElevationValue::new(1));
        assert_eq!(token_value.category(), TokenCategory::Elevation);
    }

    #[test]
    fn size_token_set_builds_without_error() {
        let set = size_token_set().expect("既定値のみからの構築は必ず成功する");
        assert_eq!(set.len(), 5);
    }

    #[test]
    fn size_token_set_has_no_duplicate_keys() {
        let set = size_token_set().expect("構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn size_token_set_all_values_are_size_category() {
        let set = size_token_set().expect("構築は成功する");
        for definition in set.values() {
            assert_eq!(definition.category(), TokenCategory::Size);
        }
    }

    #[test]
    fn radius_token_set_builds_without_error() {
        let set = radius_token_set().expect("既定値のみからの構築は必ず成功する");
        assert_eq!(set.len(), 5);
    }

    #[test]
    fn radius_token_set_all_values_are_radius_category() {
        let set = radius_token_set().expect("構築は成功する");
        for definition in set.values() {
            assert_eq!(definition.category(), TokenCategory::Radius);
        }
    }

    #[test]
    fn elevation_token_set_builds_without_error() {
        let set = elevation_token_set().expect("既定値のみからの構築は必ず成功する");
        assert_eq!(set.len(), 6);
    }

    #[test]
    fn elevation_token_set_all_values_are_elevation_category() {
        let set = elevation_token_set().expect("構築は成功する");
        for definition in set.values() {
            assert_eq!(definition.category(), TokenCategory::Elevation);
        }
    }

    #[test]
    fn radius_md_resolves_via_alias() {
        let base = radius_token_set().expect("構築は成功する");
        let resolver = TokenResolver::new(base, TokenSet::new());
        let path = TokenPath::from_dotted("radius.md").expect("パスは有効");
        let reference = TokenRef::new(path);
        let resolved = resolver
            .resolve(&reference)
            .expect("radius.md は既定トークンセットに存在する");
        match resolved.value() {
            TokenValue::Radius(value) => assert_eq!(value.value(), radius_scale::MD),
            other => panic!("radius.md は Radius 値であるべきです: {other:?}"),
        }
    }

    #[test]
    fn reference_expecting_radius_but_pointing_to_size_is_type_mismatch() {
        let mut base = size_token_set().expect("構築は成功する");
        for (key, definition) in radius_token_set().expect("構築は成功する") {
            base.insert(key, definition);
        }
        let alias_path = TokenPath::from_dotted("radius.alias_to_size").expect("パスは有効");
        let target_path = TokenPath::from_dotted("size.md").expect("パスは有効");
        base.insert(
            alias_path.clone(),
            TokenDefinition::Reference {
                category: TokenCategory::Radius,
                reference: TokenRef::new(target_path),
            },
        );

        let resolver = TokenResolver::new(base, TokenSet::new());
        let reference = TokenRef::new(alias_path);
        let err = resolver
            .resolve(&reference)
            .expect_err("size.* を参照する radius 期待の参照は型不一致になる");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }
}
