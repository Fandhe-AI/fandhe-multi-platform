//! 色トークンの専用値型・ベースパレット定義（TASK-8.4-S6、#162）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4.3・§5.2 が
//! 定める色トークンの値モデルを実装契約として引き継ぐ。同文書 §4.1 の割り当て表に
//! よりカテゴリ `color` は S6（#162、パレット定義）と S7（#163、セマンティック
//! カラー = エイリアス参照）の 2 issue に分割されており、本モジュールはパレット
//! （基礎色）と `ColorValue` 型の実装のみを担う。役割別エイリアス（`color.primary`
//! 等が `color.palette.*` を参照する構成）は S7 のスコープ。
//!
//! `schema::TokenValue::Color` は本型（`ColorValue`）を保持する。プラットフォーム別
//! 既定テーマの具体値（iOS HIG / Material 準拠）調整は S16（#172）のスコープであり、
//! 本モジュールのパレット値はフレームワーク中立の既定値に留める。

use super::error::TokenError;
use super::schema::{TokenDefinition, TokenPath, TokenSet, TokenValue};

/// 色トークンの値（設計文書 §4.3・§5.2 準拠）。
///
/// 8bit 整数チャンネル（`0..=255`）を正とし、`wgpu` 等の描画バックエンドが要求する
/// ことが多い f32 正規化値（`0.0..=1.0`）への変換 API（[`ColorValue::to_normalized`]）
/// を併せ持つ。プラットフォーム統合レイヤ（platform-builder が担当する描画バック
/// エンド接続層）がこの変換 API 経由で消費する想定であり、本型自体は
/// `fandhe-core` に閉じ、特定の描画 API には依存しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColorValue {
    /// 赤チャンネル（0..=255）。
    pub r: u8,
    /// 緑チャンネル（0..=255）。
    pub g: u8,
    /// 青チャンネル（0..=255）。
    pub b: u8,
    /// アルファチャンネル（0..=255、0 が完全透明・255 が完全不透明）。
    pub a: u8,
}

impl ColorValue {
    /// 4 チャンネルすべてを指定して構築する。
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// アルファを不透明（255）固定で構築する。
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    /// 0.0..=1.0 の f32 正規化値へ変換する（`[r, g, b, a]` 順）。
    ///
    /// `wgpu` 等、f32 系の色表現を要求する描画バックエンドへ橋渡しする際に
    /// プラットフォーム統合レイヤから呼び出される想定の変換 API（設計文書 §4.3）。
    pub fn to_normalized(self) -> [f32; 4] {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            f32::from(self.a) / 255.0,
        ]
    }

    /// `#RRGGBB` / `#RRGGBBAA` 形式の 16 進文字列から構築する。
    ///
    /// 許可する形式はこの 2 つのみ（3 桁短縮記法・`rgb()` 関数記法等は受理しない）。
    /// 外部入力（S17、#173 の外部定義ファイル読み込みが将来渡し得る文字列）を
    /// 検証せず解釈しないための最小防御であり（OWASP A03 対策、security.md）、
    /// 不正な入力は panic せず [`TokenError::InvalidColor`] で拒否する
    /// （coding-rust.md の panic 禁止方針）。
    pub fn from_hex(s: &str) -> Result<Self, TokenError> {
        let digits = s
            .strip_prefix('#')
            .ok_or_else(|| TokenError::InvalidColor {
                reason: format!("先頭に '#' がありません: {s}"),
            })?;

        if !(digits.len() == 6 || digits.len() == 8) {
            return Err(TokenError::InvalidColor {
                reason: format!("長さが不正です（#RRGGBB か #RRGGBBAA のみ受理します）: {s}"),
            });
        }
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(TokenError::InvalidColor {
                reason: format!("16 進文字以外が含まれています: {s}"),
            });
        }

        let byte = |offset: usize| -> Result<u8, TokenError> {
            u8::from_str_radix(&digits[offset..offset + 2], 16).map_err(|_| {
                TokenError::InvalidColor {
                    reason: format!("16 進数として解釈できません: {s}"),
                }
            })
        };

        let r = byte(0)?;
        let g = byte(2)?;
        let b = byte(4)?;
        let a = if digits.len() == 8 { byte(6)? } else { 255 };

        Ok(Self::rgba(r, g, b, a))
    }

    /// `#RRGGBBAA` 形式の 16 進文字列へ変換する。
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }
}

impl std::fmt::Display for ColorValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// ベースパレット（`color.palette.*`、設計文書 §4.2 の命名規則準拠）。
///
/// 5 系統（gray / blue / red / green / yellow）× 10 階調（50, 100..=900）に加え
/// white / black / transparent の単独色から成る。値はフレームワーク中立の既定値
/// であり、プラットフォーム準拠（iOS HIG / Material）の具体値調整は S16（#172）の
/// スコープ。AI 型安全 DSL（S19、#175）からの型付き参照を見据え、各色を
/// `pub const` として公開する（文字列キー経由の実行時解決に閉じない経路）。
pub mod palette {
    use super::ColorValue;

    // gray 系統。
    pub const GRAY_50: ColorValue = ColorValue::rgb(0xfa, 0xfa, 0xfa);
    pub const GRAY_100: ColorValue = ColorValue::rgb(0xf5, 0xf5, 0xf5);
    pub const GRAY_200: ColorValue = ColorValue::rgb(0xe5, 0xe5, 0xe5);
    pub const GRAY_300: ColorValue = ColorValue::rgb(0xd4, 0xd4, 0xd4);
    pub const GRAY_400: ColorValue = ColorValue::rgb(0xa3, 0xa3, 0xa3);
    pub const GRAY_500: ColorValue = ColorValue::rgb(0x73, 0x73, 0x73);
    pub const GRAY_600: ColorValue = ColorValue::rgb(0x52, 0x52, 0x52);
    pub const GRAY_700: ColorValue = ColorValue::rgb(0x40, 0x40, 0x40);
    pub const GRAY_800: ColorValue = ColorValue::rgb(0x26, 0x26, 0x26);
    pub const GRAY_900: ColorValue = ColorValue::rgb(0x17, 0x17, 0x17);

    // blue 系統。
    pub const BLUE_50: ColorValue = ColorValue::rgb(0xef, 0xf6, 0xff);
    pub const BLUE_100: ColorValue = ColorValue::rgb(0xdb, 0xea, 0xfe);
    pub const BLUE_200: ColorValue = ColorValue::rgb(0xbf, 0xdb, 0xfe);
    pub const BLUE_300: ColorValue = ColorValue::rgb(0x93, 0xc5, 0xfd);
    pub const BLUE_400: ColorValue = ColorValue::rgb(0x60, 0xa5, 0xfa);
    pub const BLUE_500: ColorValue = ColorValue::rgb(0x3b, 0x82, 0xf6);
    pub const BLUE_600: ColorValue = ColorValue::rgb(0x25, 0x63, 0xeb);
    pub const BLUE_700: ColorValue = ColorValue::rgb(0x1d, 0x4e, 0xd8);
    pub const BLUE_800: ColorValue = ColorValue::rgb(0x1e, 0x40, 0xaf);
    pub const BLUE_900: ColorValue = ColorValue::rgb(0x1e, 0x3a, 0x8a);

    // red 系統。
    pub const RED_50: ColorValue = ColorValue::rgb(0xfe, 0xf2, 0xf2);
    pub const RED_100: ColorValue = ColorValue::rgb(0xfe, 0xe2, 0xe2);
    pub const RED_200: ColorValue = ColorValue::rgb(0xfe, 0xca, 0xca);
    pub const RED_300: ColorValue = ColorValue::rgb(0xfc, 0xa5, 0xa5);
    pub const RED_400: ColorValue = ColorValue::rgb(0xf8, 0x71, 0x71);
    pub const RED_500: ColorValue = ColorValue::rgb(0xef, 0x44, 0x44);
    pub const RED_600: ColorValue = ColorValue::rgb(0xdc, 0x26, 0x26);
    pub const RED_700: ColorValue = ColorValue::rgb(0xb9, 0x1c, 0x1c);
    pub const RED_800: ColorValue = ColorValue::rgb(0x99, 0x1b, 0x1b);
    pub const RED_900: ColorValue = ColorValue::rgb(0x7f, 0x1d, 0x1d);

    // green 系統。
    pub const GREEN_50: ColorValue = ColorValue::rgb(0xf0, 0xfd, 0xf4);
    pub const GREEN_100: ColorValue = ColorValue::rgb(0xdc, 0xfc, 0xe7);
    pub const GREEN_200: ColorValue = ColorValue::rgb(0xbb, 0xf7, 0xd0);
    pub const GREEN_300: ColorValue = ColorValue::rgb(0x86, 0xef, 0xac);
    pub const GREEN_400: ColorValue = ColorValue::rgb(0x4a, 0xde, 0x80);
    pub const GREEN_500: ColorValue = ColorValue::rgb(0x22, 0xc5, 0x5e);
    pub const GREEN_600: ColorValue = ColorValue::rgb(0x16, 0xa3, 0x4a);
    pub const GREEN_700: ColorValue = ColorValue::rgb(0x15, 0x80, 0x3d);
    pub const GREEN_800: ColorValue = ColorValue::rgb(0x16, 0x65, 0x34);
    pub const GREEN_900: ColorValue = ColorValue::rgb(0x14, 0x53, 0x2d);

    // yellow 系統。
    pub const YELLOW_50: ColorValue = ColorValue::rgb(0xfe, 0xfc, 0xe8);
    pub const YELLOW_100: ColorValue = ColorValue::rgb(0xfe, 0xf9, 0xc3);
    pub const YELLOW_200: ColorValue = ColorValue::rgb(0xfe, 0xf0, 0x8a);
    pub const YELLOW_300: ColorValue = ColorValue::rgb(0xfd, 0xe0, 0x47);
    pub const YELLOW_400: ColorValue = ColorValue::rgb(0xfa, 0xcc, 0x15);
    pub const YELLOW_500: ColorValue = ColorValue::rgb(0xea, 0xb3, 0x08);
    pub const YELLOW_600: ColorValue = ColorValue::rgb(0xca, 0x8a, 0x04);
    pub const YELLOW_700: ColorValue = ColorValue::rgb(0xa1, 0x62, 0x07);
    pub const YELLOW_800: ColorValue = ColorValue::rgb(0x85, 0x4d, 0x0e);
    pub const YELLOW_900: ColorValue = ColorValue::rgb(0x71, 0x3f, 0x12);

    // 単独色。
    pub const WHITE: ColorValue = ColorValue::rgb(0xff, 0xff, 0xff);
    pub const BLACK: ColorValue = ColorValue::rgb(0x00, 0x00, 0x00);
    pub const TRANSPARENT: ColorValue = ColorValue::rgba(0x00, 0x00, 0x00, 0x00);
}

/// パレット全色を束ねた `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`color.palette.<色名>-<階調>`）に従い、
/// 検証付きコンストラクタ `TokenPath::from_dotted` 経由でのみ生成する
/// （生文字列を解決エンジンの境界へ渡さない契約、`token_resolution-architecture.md`
/// §3.1）。定義済みリテラルのみから生成するため実行時に検証が失敗することは
/// ないが、`TokenPath::new` が `Result` を返す契約を守り `unwrap`/`expect` を
/// 使わずに `?` で伝播する（coding-rust.md の panic 禁止方針）。呼び出し元は
/// `TokenResolver::new` の `base` 引数としてこの戻り値をそのまま利用できる。
pub fn palette_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, ColorValue)] = &[
        ("color.palette.gray-50", palette::GRAY_50),
        ("color.palette.gray-100", palette::GRAY_100),
        ("color.palette.gray-200", palette::GRAY_200),
        ("color.palette.gray-300", palette::GRAY_300),
        ("color.palette.gray-400", palette::GRAY_400),
        ("color.palette.gray-500", palette::GRAY_500),
        ("color.palette.gray-600", palette::GRAY_600),
        ("color.palette.gray-700", palette::GRAY_700),
        ("color.palette.gray-800", palette::GRAY_800),
        ("color.palette.gray-900", palette::GRAY_900),
        ("color.palette.blue-50", palette::BLUE_50),
        ("color.palette.blue-100", palette::BLUE_100),
        ("color.palette.blue-200", palette::BLUE_200),
        ("color.palette.blue-300", palette::BLUE_300),
        ("color.palette.blue-400", palette::BLUE_400),
        ("color.palette.blue-500", palette::BLUE_500),
        ("color.palette.blue-600", palette::BLUE_600),
        ("color.palette.blue-700", palette::BLUE_700),
        ("color.palette.blue-800", palette::BLUE_800),
        ("color.palette.blue-900", palette::BLUE_900),
        ("color.palette.red-50", palette::RED_50),
        ("color.palette.red-100", palette::RED_100),
        ("color.palette.red-200", palette::RED_200),
        ("color.palette.red-300", palette::RED_300),
        ("color.palette.red-400", palette::RED_400),
        ("color.palette.red-500", palette::RED_500),
        ("color.palette.red-600", palette::RED_600),
        ("color.palette.red-700", palette::RED_700),
        ("color.palette.red-800", palette::RED_800),
        ("color.palette.red-900", palette::RED_900),
        ("color.palette.green-50", palette::GREEN_50),
        ("color.palette.green-100", palette::GREEN_100),
        ("color.palette.green-200", palette::GREEN_200),
        ("color.palette.green-300", palette::GREEN_300),
        ("color.palette.green-400", palette::GREEN_400),
        ("color.palette.green-500", palette::GREEN_500),
        ("color.palette.green-600", palette::GREEN_600),
        ("color.palette.green-700", palette::GREEN_700),
        ("color.palette.green-800", palette::GREEN_800),
        ("color.palette.green-900", palette::GREEN_900),
        ("color.palette.yellow-50", palette::YELLOW_50),
        ("color.palette.yellow-100", palette::YELLOW_100),
        ("color.palette.yellow-200", palette::YELLOW_200),
        ("color.palette.yellow-300", palette::YELLOW_300),
        ("color.palette.yellow-400", palette::YELLOW_400),
        ("color.palette.yellow-500", palette::YELLOW_500),
        ("color.palette.yellow-600", palette::YELLOW_600),
        ("color.palette.yellow-700", palette::YELLOW_700),
        ("color.palette.yellow-800", palette::YELLOW_800),
        ("color.palette.yellow-900", palette::YELLOW_900),
        ("color.palette.white", palette::WHITE),
        ("color.palette.black", palette::BLACK),
        ("color.palette.transparent", palette::TRANSPARENT),
    ];

    let mut set = TokenSet::new();
    for (key, value) in entries {
        let path = TokenPath::from_dotted(key)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Color(*value),
            },
        );
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_parses_six_digit_rrggbb() {
        let color = ColorValue::from_hex("#FF0000").expect("正当な #RRGGBB は解決できる");
        assert_eq!(color, ColorValue::rgba(255, 0, 0, 255));
    }

    #[test]
    fn from_hex_parses_eight_digit_rrggbbaa_lowercase() {
        let color = ColorValue::from_hex("#11223344").expect("正当な #RRGGBBAA は解決できる");
        assert_eq!(color, ColorValue::rgba(0x11, 0x22, 0x33, 0x44));
    }

    #[test]
    fn from_hex_rejects_missing_hash_prefix() {
        let err = ColorValue::from_hex("FF0000").expect_err("先頭 '#' 欠落は拒否される");
        assert!(matches!(err, TokenError::InvalidColor { .. }));
    }

    #[test]
    fn from_hex_rejects_invalid_length() {
        let err = ColorValue::from_hex("#FFF").expect_err("3 桁短縮記法は拒否される");
        assert!(matches!(err, TokenError::InvalidColor { .. }));

        let err = ColorValue::from_hex("#FF00000").expect_err("7 桁は拒否される");
        assert!(matches!(err, TokenError::InvalidColor { .. }));
    }

    #[test]
    fn from_hex_rejects_non_hex_characters() {
        let err = ColorValue::from_hex("#GG0000").expect_err("16 進文字以外は拒否される");
        assert!(matches!(err, TokenError::InvalidColor { .. }));
    }

    #[test]
    fn from_hex_rejects_empty_string() {
        let err = ColorValue::from_hex("").expect_err("空文字列は拒否される");
        assert!(matches!(err, TokenError::InvalidColor { .. }));
    }

    #[test]
    fn to_normalized_maps_zero_and_max_channel_values() {
        let black = ColorValue::rgba(0, 0, 0, 0);
        assert_eq!(black.to_normalized(), [0.0, 0.0, 0.0, 0.0]);

        let white = ColorValue::rgba(255, 255, 255, 255);
        assert_eq!(white.to_normalized(), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn to_normalized_maps_intermediate_value_approximately() {
        let mid = ColorValue::rgb(128, 128, 128);
        let normalized = mid.to_normalized();
        let expected = 128.0 / 255.0;
        for channel in &normalized[..3] {
            assert!((channel - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn to_hex_from_hex_round_trip() {
        let original = ColorValue::rgba(0x9a, 0x1b, 0xef, 0x80);
        let hex = original.to_hex();
        let parsed = ColorValue::from_hex(&hex).expect("自身が出力した hex は自身で解釈できる");
        assert_eq!(parsed, original);
    }

    #[test]
    fn display_matches_to_hex() {
        let color = ColorValue::rgb(1, 2, 3);
        assert_eq!(color.to_string(), color.to_hex());
    }

    #[test]
    fn palette_constants_are_pairwise_distinct_within_each_scale() {
        // 各系統内で 10 階調が重複しないことを確認する（パレットとしての意味を持つため）。
        let blue = [
            palette::BLUE_50,
            palette::BLUE_100,
            palette::BLUE_200,
            palette::BLUE_300,
            palette::BLUE_400,
            palette::BLUE_500,
            palette::BLUE_600,
            palette::BLUE_700,
            palette::BLUE_800,
            palette::BLUE_900,
        ];
        for (i, a) in blue.iter().enumerate() {
            for (j, b) in blue.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "blue パレットの階調 {i} と {j} が重複しています");
                }
            }
        }
    }

    #[test]
    fn white_black_transparent_are_distinct() {
        assert_ne!(palette::WHITE, palette::BLACK);
        assert_ne!(palette::WHITE, palette::TRANSPARENT);
        assert_ne!(palette::BLACK, palette::TRANSPARENT);
        assert_eq!(palette::TRANSPARENT.a, 0);
        assert_eq!(palette::WHITE.a, 255);
    }

    #[test]
    fn palette_token_set_builds_without_error() {
        // 全キーが TokenPath::from_dotted の検証（許可文字集合・`..` 拒否）を
        // 通過することの確認（§6 検証方法「全キーが TokenPath 検証を通過」）。
        let set = palette_token_set().expect("定義済みリテラルのみからの構築は必ず成功する");
        // 5 系統 × 10 階調 + white/black/transparent = 53 件。
        assert_eq!(set.len(), 53);
    }

    #[test]
    fn palette_token_set_has_no_duplicate_keys() {
        let set = palette_token_set().expect("パレット構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn palette_token_set_all_values_are_color_category() {
        let set = palette_token_set().expect("パレット構築は成功する");
        for definition in set.values() {
            assert_eq!(
                definition.category(),
                super::super::schema::TokenCategory::Color
            );
        }
    }
}
