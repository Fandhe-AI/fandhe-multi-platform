//! AI 型安全 DSL からのトークン参照 API（TASK-8.4-S19、#175）。
//!
//! 親 issue #156（TASK-8.4「デザイントークン基盤」）の分解 sub-issue（19/24）。
//! `docs/architecture/design-token-architecture.md` §5.3・
//! `docs/architecture/token-resolution-architecture.md` §4・§4.1 が S19 に課す契約:
//! 「文字列キーの実行時解決に閉じず、カテゴリごとに閉じた型を引数に取る型付き
//! アクセサを用意する」「クロージャを引数に取る API を追加しない（REQ-13 整合）」
//! 「`TokenResolver::resolve` を内部で呼び出す公開 API を設計する」「エラー時に
//! panic せず、フォールバック方針は呼び出し元が決定する」。
//!
//! 既存の `TokenValue::as_spacing` / `as_size` / `as_radius`（S15、#171、`schema.rs`
//! 参照）はカテゴリごとの個別関数であり、本モジュールはそれを [`TokenKind`] という
//! 単一のトレイトへ一般化し、`TypedTokenRef<K>` という 1 つの型でカテゴリ 10 種
//! （Color/Typography/Spacing/Size/Radius/Elevation/Border/Shadow/Duration/
//! Animation。`TokenCategory::Dimension` / `Text` は S1 以前の暫定遺産であり
//! 対象外）すべてを表現する。呼び出し元（`ui::style_tokens` の `with_*_typed`
//! ビルダー等、DSL 生成コードの主経路）は「カテゴリを取り違えたトークン参照を
//! 渡す」という誤りを、実行時の `TokenError::TypeMismatch` ではなくコンパイル
//! エラーとして検出できる（`tests/token_typed_compile_fail.rs` が固定化）。

use super::error::TokenError;
use super::resolver::TokenResolver;
use super::schema::{TokenCategory, TokenPath, TokenRef, TokenValue};

use super::animation::{AnimationValue, DurationValue};
use super::border::BorderValue;
use super::color::ColorValue;
use super::dimension::{ElevationValue, RadiusValue, SizeValue};
use super::shadow::ShadowValue;
use super::spacing::SpacingValue;
use super::typography::TypographyValue;

/// 外部実装を遮断する封印モジュール（sealed トレイトパターン）。
///
/// [`TokenKind`] を `pub trait` としつつ、本クレート外からの実装を禁止するために
/// 用いる。マーカー型 10 種以外が `TokenKind` を実装してしまうと「カテゴリと値型の
/// 対応」という本モジュールの中核契約が崩れるため、supertrait として非公開に保つ。
mod sealed {
    /// [`super::TokenKind`] の supertrait。外部クレートはこのトレイトへアクセス
    /// できないため `TokenKind` を実装できない。
    pub trait Sealed {}
}

/// カテゴリマーカー型 → 対応する `TokenValue` バリアントの値型、を結び付ける
/// 封印トレイト（設計文書 §5.3 の「型付きアクセサ」契約の核）。
///
/// 実装はすべて本モジュール内のマーカー型 10 種に限定される（`sealed::Sealed`
/// 参照）。`extract` はバリアント一致でのみ値を取り出し、`SizeValue` /
/// `RadiusValue` のように実体（`DimensionValue`）を共有する型同士でも
/// バリアント判別によりカテゴリの取り違えを検出できる（S15 の `as_size` /
/// `as_radius` と同じ契約、`schema.rs` 参照）。
pub trait TokenKind: sealed::Sealed {
    /// このマーカー型に対応するトークンカテゴリ。
    const CATEGORY: TokenCategory;

    /// このマーカー型に対応する解決済みの値型。
    type Value: Clone;

    /// `TokenValue` から対応バリアントの値を取り出す。バリアント不一致なら
    /// `None`（呼び出し元の [`TypedTokenRef::resolve`] が `TokenError::TypeMismatch`
    /// へ変換する）。
    fn extract(value: &TokenValue) -> Option<Self::Value>;
}

/// `TokenKind` マーカー型 1 種を宣言するマクロ（10 カテゴリ分の定型実装を集約し、
/// カテゴリ追加時の書き漏らし・実装間の不整合を防ぐ）。
macro_rules! define_token_kind {
    ($marker:ident, $category:ident, $value_ty:ty, $variant:ident) => {
        #[doc = concat!(
            "`TokenCategory::", stringify!($category), "` に対応する `TokenKind` マーカー型。"
        )]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $marker;

        impl sealed::Sealed for $marker {}

        impl TokenKind for $marker {
            const CATEGORY: TokenCategory = TokenCategory::$category;
            type Value = $value_ty;

            fn extract(value: &TokenValue) -> Option<Self::Value> {
                match value {
                    TokenValue::$variant(v) => Some(v.clone()),
                    _ => None,
                }
            }
        }
    };
}

define_token_kind!(ColorKind, Color, ColorValue, Color);
define_token_kind!(TypographyKind, Typography, TypographyValue, Typography);
define_token_kind!(SpacingKind, Spacing, SpacingValue, Spacing);
define_token_kind!(SizeKind, Size, SizeValue, Size);
define_token_kind!(RadiusKind, Radius, RadiusValue, Radius);
define_token_kind!(ElevationKind, Elevation, ElevationValue, Elevation);
define_token_kind!(BorderKind, Border, BorderValue, Border);
define_token_kind!(ShadowKind, Shadow, ShadowValue, Shadow);
define_token_kind!(DurationKind, Duration, DurationValue, Duration);
define_token_kind!(AnimationKind, Animation, AnimationValue, Animation);

/// カテゴリを型パラメータに焼き込んだトークン参照（設計文書 §5.3 の中核型）。
///
/// 無型の [`TokenRef`] とは異なり、`K: TokenKind` によってこの参照が指し示す
/// べきカテゴリがコンパイル時に固定される。DSL 生成コード（AI が出力する
/// ウィジェットスタイル定義等）がカテゴリの異なる `TypedTokenRef` 同士を
/// 取り違えて渡した場合、型パラメータの不一致としてコンパイルが拒否される
/// （`tests/token_typed_compile_fail.rs` 参照）。
///
/// クロージャを引数に取るコンストラクタ・メソッドは持たない（REQ-13 整合、
/// 設計文書 §5.3）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedTokenRef<K: TokenKind> {
    inner: TokenRef,
    _kind: core::marker::PhantomData<K>,
}

impl<K: TokenKind> TypedTokenRef<K> {
    /// 検証済みの `TokenPath` から構築する。
    pub fn new(path: TokenPath) -> Self {
        Self {
            inner: TokenRef::new(path),
            _kind: core::marker::PhantomData,
        }
    }

    /// 既存の無型 `TokenRef` を型付き参照へ持ち上げる。
    ///
    /// 呼び出し元がカテゴリを取り違えて持ち上げても、この時点ではコンパイルが
    /// 通ってしまう（`TokenRef` 自体はカテゴリ情報を持たないため）。実際の
    /// カテゴリ不一致は [`Self::resolve`] が実行時に `TokenError::TypeMismatch`
    /// として検出する（構築時点で防げるのは「型付き参照同士の取り違え」であり、
    /// 「無型 `TokenRef` の中身が違うカテゴリを指す」ケースは実行時契約のまま、
    /// 設計文書 §5.3 が定める防御範囲どおり）。
    pub fn from_ref(token_ref: TokenRef) -> Self {
        Self {
            inner: token_ref,
            _kind: core::marker::PhantomData,
        }
    }

    /// ドット区切りの文字列からパースして構築する（検証付き `TokenPath::from_dotted`
    /// 経由。OWASP A03 対策の検証経路を維持し、独自のパース経路を新設しない、
    /// `.claude/rules/security.md` 参照）。
    ///
    /// # Errors
    ///
    /// パスの構文が不正な場合（許可文字集合外の文字・空セグメント等）に
    /// `TokenError::InvalidPath` を返す。
    pub fn from_dotted(path: &str) -> Result<Self, TokenError> {
        Ok(Self::new(TokenPath::from_dotted(path)?))
    }

    /// 内包する無型 `TokenRef` を消費して返す。
    pub fn into_ref(self) -> TokenRef {
        self.inner
    }

    /// `resolver` 経由でこの参照を解決し、`K::Value` へ変換する（設計文書 §5.3・
    /// `token/resolver.rs` の契約どおり `TokenResolver::resolve` へ委譲する薄い
    /// ラッパー）。
    ///
    /// エラー時に既定値へ自動フォールバックしない（呼び出し元がフォールバック
    /// 方針を決定する、`resolver.rs` の契約）。クロージャを引数に取らない
    /// （REQ-13 整合）。
    ///
    /// # Errors
    ///
    /// - 参照先が未定義: `TokenError::UndefinedReference`
    /// - 参照先の実カテゴリが `K::CATEGORY` と一致しない:
    ///   `TokenError::TypeMismatch`（`expected` は `K::CATEGORY.name()`）
    /// - エイリアスチェーンが循環: `TokenError::CircularReference`
    /// - エイリアスチェーンの深さが上限超過: `TokenError::MaxDepthExceeded`
    pub fn resolve(&self, resolver: &TokenResolver) -> Result<K::Value, TokenError> {
        let resolved = resolver.resolve(&self.inner)?;
        K::extract(resolved.value()).ok_or_else(|| TokenError::TypeMismatch {
            origin: self.inner.to_string(),
            reference: self.inner.to_string(),
            expected: K::CATEGORY.name(),
            actual: resolved.value().category().name(),
        })
    }
}

impl<K: TokenKind> AsRef<TokenRef> for TypedTokenRef<K> {
    /// 内包する無型 `TokenRef` を参照で返す（既存 API との相互運用。標準トレイト
    /// `AsRef` を実装することで clippy の `should_implement_trait` 指摘を避けつつ
    /// 同名の慣用的な変換手段を提供する）。
    fn as_ref(&self) -> &TokenRef {
        &self.inner
    }
}

/// 色トークンの型付き参照。
pub type ColorTokenRef = TypedTokenRef<ColorKind>;
/// タイポグラフィトークンの型付き参照。
pub type TypographyTokenRef = TypedTokenRef<TypographyKind>;
/// スペーシングトークンの型付き参照。
pub type SpacingTokenRef = TypedTokenRef<SpacingKind>;
/// サイズトークンの型付き参照。
pub type SizeTokenRef = TypedTokenRef<SizeKind>;
/// 角丸（radius）トークンの型付き参照。
pub type RadiusTokenRef = TypedTokenRef<RadiusKind>;
/// 高さ（elevation）トークンの型付き参照。
pub type ElevationTokenRef = TypedTokenRef<ElevationKind>;
/// ボーダートークンの型付き参照。
pub type BorderTokenRef = TypedTokenRef<BorderKind>;
/// シャドウトークンの型付き参照。
pub type ShadowTokenRef = TypedTokenRef<ShadowKind>;
/// duration トークンの型付き参照。
pub type DurationTokenRef = TypedTokenRef<DurationKind>;
/// アニメーショントークンの型付き参照。
pub type AnimationTokenRef = TypedTokenRef<AnimationKind>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::schema::{TokenDefinition, TokenSet};
    use crate::token::spacing::SpacingValue;

    fn path(s: &str) -> TokenPath {
        TokenPath::from_dotted(s).expect("テスト用の正当なパス")
    }

    fn value_def(value: TokenValue) -> TokenDefinition {
        TokenDefinition::Value { value }
    }

    #[test]
    fn resolves_matching_category_successfully() {
        let mut base = TokenSet::new();
        base.insert(
            path("spacing.md"),
            value_def(TokenValue::Spacing(
                SpacingValue::new(16.0).expect("正当な値"),
            )),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());

        let token_ref: SpacingTokenRef =
            TypedTokenRef::from_dotted("spacing.md").expect("正当なパス");
        let value = token_ref
            .resolve(&resolver)
            .expect("spacing カテゴリは解決できる");
        assert_eq!(value.value(), 16.0);
    }

    #[test]
    fn category_mismatch_is_reported_as_type_mismatch() {
        let mut base = TokenSet::new();
        base.insert(
            path("spacing.md"),
            value_def(TokenValue::Spacing(
                SpacingValue::new(16.0).expect("正当な値"),
            )),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());

        // ColorTokenRef で spacing.md（実際は Spacing カテゴリ）を解決しようとする。
        let token_ref: ColorTokenRef =
            TypedTokenRef::from_dotted("spacing.md").expect("正当なパス");
        let err = token_ref
            .resolve(&resolver)
            .expect_err("カテゴリ不一致は TypeMismatch になる");
        match err {
            TokenError::TypeMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, "Color");
                assert_eq!(actual, "Spacing");
            }
            other => panic!("TypeMismatch を期待したが {other:?} だった"),
        }
    }

    #[test]
    fn size_and_radius_variants_are_not_interchangeable() {
        // Size / Radius は同一の DimensionValue を共有するため、値そのものでは
        // 取り違えを検出できない。TokenValue のバリアント一致で判別できることを
        // 確認する（schema.rs の as_size/as_radius と同じ観点）。
        let mut base = TokenSet::new();
        base.insert(
            path("radius.md"),
            value_def(TokenValue::Radius(RadiusValue::new(8.0).expect("正当な値"))),
        );
        let resolver = TokenResolver::new(base, TokenSet::new());

        let radius_ref: RadiusTokenRef =
            TypedTokenRef::from_dotted("radius.md").expect("正当なパス");
        assert_eq!(
            radius_ref
                .resolve(&resolver)
                .expect("radius として解決できる")
                .value(),
            8.0
        );

        let size_ref: SizeTokenRef = TypedTokenRef::from_dotted("radius.md").expect("正当なパス");
        let err = size_ref
            .resolve(&resolver)
            .expect_err("Size として解決しようとすると取り違えが検出される");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }

    #[test]
    fn undefined_reference_propagates() {
        let resolver = TokenResolver::new(TokenSet::new(), TokenSet::new());
        let token_ref: ColorTokenRef =
            TypedTokenRef::from_dotted("color.missing").expect("正当なパス");
        let err = token_ref
            .resolve(&resolver)
            .expect_err("未定義参照はそのまま伝播する");
        assert!(matches!(err, TokenError::UndefinedReference { .. }));
    }

    #[test]
    fn from_dotted_rejects_invalid_path() {
        let err = ColorTokenRef::from_dotted("a/b").expect_err("不正なパスは拒否される");
        assert!(matches!(err, TokenError::InvalidPath { .. }));
    }

    #[test]
    fn from_ref_and_as_ref_round_trip() {
        let raw = TokenRef::new(path("color.primary"));
        let typed = ColorTokenRef::from_ref(raw.clone());
        assert_eq!(typed.as_ref(), &raw);
        assert_eq!(typed.into_ref(), raw);
    }
}
