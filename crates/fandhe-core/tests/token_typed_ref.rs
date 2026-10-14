//! `token::typed`（型付きトークン参照 API）の公開 API 経由の結合テスト
//! （TASK-8.4-S19、#175）。
//!
//! `crates/fandhe-core/tests/platform_theme.rs` の前例に倣い、`fandhe_core::*` の
//! 再エクスポートのみを経由して `TypedTokenRef<K>` / `SemanticColorRole::typed_ref`
//! の公開契約（ライト/ダークテーマ切替・エイリアスチェーン越しの型付き解決が
//! `platform_base_token_set` / `platform_dark_overrides`（S16、#172）と組み合わせて
//! 正しく動作すること）を固定化する。カテゴリ単位の単体検証は
//! `crates/fandhe-core/src/token/typed.rs` の `#[cfg(test)] mod tests` が担い、
//! 本ファイルは「公開 API 経由で一連の流れが通ること」の結合確認に留める。

use fandhe_core::{
    ColorTokenRef, SemanticColorRole, SpacingTokenRef, ThemePlatform, TokenResolver, TypedTokenRef,
    platform_base_token_set, platform_dark_overrides,
};

#[test]
fn semantic_color_typed_ref_switches_between_light_and_dark_via_public_api() {
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS ベーステーマを構築できる");
    let light_resolver = TokenResolver::new(base.clone(), Default::default());

    let typed_ref: ColorTokenRef = SemanticColorRole::Primary
        .typed_ref()
        .expect("セマンティックロールから型付き参照を生成できる");
    let light_primary = typed_ref
        .resolve(&light_resolver)
        .expect("ライトの primary は型付き参照経由で解決できる");

    let dark_overrides =
        platform_dark_overrides(ThemePlatform::Ios).expect("iOS ダーク差分を構築できる");
    let dark_resolver = TokenResolver::new(base, dark_overrides);
    let dark_primary = typed_ref
        .resolve(&dark_resolver)
        .expect("ダークの primary は型付き参照経由で解決できる");

    assert_ne!(
        light_primary, dark_primary,
        "型付き参照経由でもライト/ダークで異なる具体値へ解決される必要がある"
    );
}

#[test]
fn resolve_typed_and_typed_token_ref_resolve_agree_via_public_api() {
    let base =
        platform_base_token_set(ThemePlatform::Android).expect("Android ベーステーマを構築できる");
    let resolver = TokenResolver::new(base, Default::default());

    let typed_ref: ColorTokenRef = SemanticColorRole::OnPrimary
        .typed_ref()
        .expect("型付き参照を生成できる");

    let via_typed_ref = typed_ref
        .resolve(&resolver)
        .expect("TypedTokenRef::resolve 経由で解決できる");
    let via_resolver = resolver
        .resolve_typed(&typed_ref)
        .expect("TokenResolver::resolve_typed 経由でも同じ結果を得られる");

    assert_eq!(
        via_typed_ref, via_resolver,
        "2 つの入口（TypedTokenRef::resolve / TokenResolver::resolve_typed）は同一の実体へ委譲する"
    );
}

#[test]
fn spacing_typed_ref_rejects_size_category_mismatch_via_public_api() {
    // 公開 API 経由でもカテゴリ取り違えが TypeMismatch として検出されることを確認する
    // （platform_base_token_set が含む spacing カテゴリのトークンを SpacingTokenRef で
    // 解決できる一方、無関係なキーは UndefinedReference になる境界を押さえる）。
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS ベーステーマを構築できる");
    let resolver = TokenResolver::new(base, Default::default());

    let spacing_ref: SpacingTokenRef =
        TypedTokenRef::from_dotted("spacing.md").expect("正当なパス");
    spacing_ref
        .resolve(&resolver)
        .expect("プラットフォーム既定テーマの spacing.md は型付き参照で解決できる");

    // 取り違え経路: "spacing.md" は実際には Spacing カテゴリだが、これを
    // ColorTokenRef（Color カテゴリを期待する型付き参照）で解決しようとすると、
    // 実カテゴリと期待カテゴリの不一致が TokenError::TypeMismatch として検出
    // される必要がある（typed.rs の TypedTokenRef::resolve 契約、上記コメントが
    // 主張する「型不一致の拒否」経路の実体）。
    let mismatched_color_ref: ColorTokenRef =
        TypedTokenRef::from_dotted("spacing.md").expect("正当なパス");
    let err = mismatched_color_ref
        .resolve(&resolver)
        .expect_err("Spacing カテゴリの実体を ColorTokenRef で解決すると TypeMismatch になる");
    match err {
        fandhe_core::TokenError::TypeMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, "Color");
            assert_eq!(actual, "Spacing");
        }
        other => panic!("TypeMismatch を期待したが {other:?} だった"),
    }

    // 取り違えとは別の境界: プラットフォーム既定テーマに存在しないキーを
    // 参照した場合は UndefinedReference として区別される（TypeMismatch との
    // 混同を防ぐ境界確認）。
    let undefined_ref: SpacingTokenRef =
        TypedTokenRef::from_dotted("spacing.does-not-exist").expect("正当なパス");
    let err = undefined_ref
        .resolve(&resolver)
        .expect_err("未定義のトークン参照は UndefinedReference になる");
    assert!(matches!(
        err,
        fandhe_core::TokenError::UndefinedReference { .. }
    ));
}
