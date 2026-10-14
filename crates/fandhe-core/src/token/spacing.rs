//! スペーシングトークンの専用値型・スケール定数（TASK-8.4-S10、#166）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4.3・§5.2 が
//! 定めるスペーシング値モデルを実装契約として引き継ぐ。同文書 §5.2 の素案は
//! spacing / size / radius の 3 カテゴリで共有する `DimensionValue { value: f32 }`
//! の型エイリアスを示すが、カテゴリ取り違えを型で検出するため、本モジュールは
//! size / radius（`dimension::DimensionValue`）とは分離した spacing 専用の
//! 検証付き newtype `SpacingValue(f32)` として実装する（値モデル自体＝論理ピクセル
//! 相当の f32 という契約は変えていない）。同文書 §4.1 の割り当て表によりカテゴリ
//! `spacing`（余白・間隔）は S10（#166）に単独で割り当てられている。
//!
//! 値は論理ピクセル相当の f32 とし、`ui/geometry.rs` の `Point` / `Size` と単位系を
//! 揃える（§4.3）。プラットフォーム準拠（iOS HIG / Material）の具体値調整は S16
//! （#172）のスコープであり、本モジュールのスケール定数はフレームワーク中立の
//! 既定値に留める。本モジュールは値型・定数・`TokenSet` ビルダーの提供のみを担う。
//! キー命名（`spacing.md` 等の t シャツサイズ表記）は §4.2 の例示キーに準拠する。
//!
//! `ui/stack_grid.rs`（`Stack::with_spacing_token` / `Grid::with_spacing_token`）・
//! `ui/card.rs`（`Card::with_padding_token`）・`ui/layout/style.rs`
//! （`LayoutStyle::with_gap_token` / `with_padding_token`）が spacing トークンを
//! 消費する（TASK-8.4-S15、#171）。本モジュールは値型・スケール定数の提供に留まり、
//! 配線先（`ui/` 配下）は解決済みの具体値のみを受け取る契約は変えていない
//! （`ui/widget.rs` の `PaintCommand` / `StateStyles` 等ウィジェット本体の配線は
//! S14、#170 が別途担う）。

use super::error::TokenError;
use super::schema::{TokenDefinition, TokenPath, TokenSet, TokenValue};

/// スペーシングトークンの値（設計文書 §4.3・§5.2 準拠）。
///
/// `TypographyValue` の行間検証（`TypographyValue::new`）と同じ防御方針で、
/// 有限（`is_finite()`）かつ 0.0 以上の値のみを構築時点で受理する
/// （NaN・無限大・負値は後段のレイアウト計算（taffy）へ伝播させない、
/// security.md の DoS 耐性観点）。行間とは異なり `spacing.none = 0` が意味を
/// 持つ値であるため、0.0 を境界値として許容する点が
/// [`super::typography::TypographyValue`] の行間検証と異なる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpacingValue(f32);

impl SpacingValue {
    /// 数値からスペーシング値を構築する。
    ///
    /// # Errors
    ///
    /// `value` が有限な 0.0 以上の値でない場合（NaN・無限大・負値）に
    /// [`TokenError::InvalidSpacing`] を返す。
    pub fn new(value: f32) -> Result<Self, TokenError> {
        if !(value.is_finite() && value >= 0.0) {
            return Err(TokenError::InvalidSpacing {
                reason: format!(
                    "スペーシング値は有限かつ 0.0 以上である必要があります（NaN・無限大・負値は不可）: {value}"
                ),
            });
        }
        Ok(Self(value))
    }

    /// 内部で保持する数値表現（論理ピクセル相当）を返す。
    ///
    /// `const fn` 化（TASK-8.4-S15、#171）: `ui/dialog.rs` 等の定数コンテキスト
    /// （`const PANEL_PADDING: f32 = spacing_scale::MD.value();`）から呼び出せる
    /// ようにするための互換変更。既存呼び出しへの挙動変化はない。
    pub const fn value(self) -> f32 {
        self.0
    }
}

/// スペーシングスケール定数（4px ベースライングリッド、設計文書 §4.2 の
/// 命名規則準拠のキーは [`spacing_token_set`] 側で付与する）。
///
/// AI 型安全 DSL（S19、#175）からの型付き参照を見据え、`color::palette` の
/// 前例に倣い各値を `pub const` として公開する（文字列キー経由の実行時解決に
/// 閉じない経路）。値はフレームワーク中立の既定値であり、プラットフォーム準拠
/// の具体値調整は S16（#172）のスコープ。
///
/// 各定数は `SpacingValue` のプライベートフィールドへ直接アクセスして構築する
/// （`color::palette` が `ColorValue::rgb` という公開 `const fn` 経由で構築する
/// のと異なり、`SpacingValue::new` は非 `const fn`（`Result` を返す）ため
/// 定数コンテキストで呼べない）。ここで列挙する値はすべてリテラルの有限
/// 非負値であり、[`SpacingValue::new`] が課す不変条件（有限かつ 0.0 以上）を
/// 満たすことをコード上で確認済み（下記 `scale_constants_are_ascending_and_pairwise_distinct`
/// テストが昇順性を担保する）。
pub mod scale {
    use super::SpacingValue;

    /// `spacing.none`（0px、余白なしを明示するための値）。
    pub const NONE: SpacingValue = SpacingValue(0.0);
    /// `spacing.xs`（4px）。
    pub const XS: SpacingValue = SpacingValue(4.0);
    /// `spacing.sm`（8px）。
    pub const SM: SpacingValue = SpacingValue(8.0);
    /// `spacing.md`（16px、設計文書 §4.2 の例示キー）。
    pub const MD: SpacingValue = SpacingValue(16.0);
    /// `spacing.lg`（24px）。
    pub const LG: SpacingValue = SpacingValue(24.0);
    /// `spacing.xl`（32px）。
    pub const XL: SpacingValue = SpacingValue(32.0);
    /// `spacing.xxl`（48px）。
    pub const XXL: SpacingValue = SpacingValue(48.0);
    /// `spacing.xxxl`（64px）。
    pub const XXXL: SpacingValue = SpacingValue(64.0);
}

/// スペーシングスケール全体を束ねた `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`spacing.<段階>`）に従い、検証付き
/// コンストラクタ `TokenPath::from_dotted` 経由でのみ生成する（生文字列を
/// 解決エンジンの境界へ渡さない契約、`token_resolution-architecture.md` §3.1、
/// `color::palette_token_set` の前例踏襲）。定義済みリテラルのみから生成する
/// ため実行時に検証が失敗することはないが、`TokenPath::new` が `Result` を
/// 返す契約を守り `unwrap`/`expect` を使わずに `?` で伝播する
/// （coding-rust.md の panic 禁止方針）。呼び出し元は `TokenResolver::new` の
/// `base` 引数としてこの戻り値をそのまま利用できる。
pub fn spacing_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, SpacingValue)] = &[
        ("spacing.none", scale::NONE),
        ("spacing.xs", scale::XS),
        ("spacing.sm", scale::SM),
        ("spacing.md", scale::MD),
        ("spacing.lg", scale::LG),
        ("spacing.xl", scale::XL),
        ("spacing.xxl", scale::XXL),
        ("spacing.xxxl", scale::XXXL),
    ];

    let mut set = TokenSet::new();
    for (key, value) in entries {
        let path = TokenPath::from_dotted(key)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Spacing(*value),
            },
        );
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_zero_boundary_value() {
        let value = SpacingValue::new(0.0).expect("0.0 は境界値として許容される");
        assert_eq!(value.value(), 0.0);
    }

    #[test]
    fn new_accepts_positive_value() {
        let value = SpacingValue::new(16.0).expect("正値は許容される");
        assert_eq!(value.value(), 16.0);
    }

    #[test]
    fn new_rejects_negative_value() {
        let err = SpacingValue::new(-1.0).expect_err("負値は拒否される");
        assert!(matches!(err, TokenError::InvalidSpacing { .. }));
    }

    #[test]
    fn new_rejects_nan() {
        let err = SpacingValue::new(f32::NAN).expect_err("NaN は拒否される");
        assert!(matches!(err, TokenError::InvalidSpacing { .. }));
    }

    #[test]
    fn new_rejects_infinite_value() {
        let err = SpacingValue::new(f32::INFINITY).expect_err("無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidSpacing { .. }));

        let err = SpacingValue::new(f32::NEG_INFINITY).expect_err("負の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidSpacing { .. }));
    }

    #[test]
    fn scale_constants_are_ascending_and_pairwise_distinct() {
        let scale = [
            scale::NONE,
            scale::XS,
            scale::SM,
            scale::MD,
            scale::LG,
            scale::XL,
            scale::XXL,
            scale::XXXL,
        ];
        for window in scale.windows(2) {
            assert!(
                window[0].value() < window[1].value(),
                "スケール定数は昇順である必要があります: {:?} -> {:?}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn spacing_token_set_builds_without_error_and_has_expected_count() {
        let set = spacing_token_set().expect("定義済みリテラルのみからの構築は必ず成功する");
        assert_eq!(set.len(), 8);
    }

    #[test]
    fn spacing_token_set_has_no_duplicate_keys() {
        let set = spacing_token_set().expect("スペーシングトークン構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn spacing_token_set_all_values_are_spacing_category() {
        let set = spacing_token_set().expect("スペーシングトークン構築は成功する");
        for definition in set.values() {
            assert_eq!(
                definition.category(),
                super::super::schema::TokenCategory::Spacing
            );
        }
    }
}
