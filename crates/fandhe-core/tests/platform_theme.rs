//! `token::platform_theme` の公開 API 経由の結合テスト（TASK-8.4-S16、#172）。
//!
//! `crates/fandhe-core/tests/token_resolver.rs` の前例に倣い、
//! `fandhe_core::*` の再エクスポートのみを経由して
//! `ThemePlatform` / `platform_base_token_set` / `platform_dark_overrides` の
//! 公開契約（iOS/Android × ライト/ダークの既定テーマが `TokenResolver` から
//! 正しく解決できること）を固定化する。プラットフォーム別具体値の網羅検証・
//! フォールバック意味論の単体検証は `crates/fandhe-core/src/token/platform_theme.rs`
//! の `#[cfg(test)] mod tests` が担い、本ファイルは「公開 API 経由で一連の流れが
//! 通ること」の結合確認に留める。

use fandhe_core::{
    SemanticColorRole, ThemePlatform, TokenResolver, TokenSet, platform_base_token_set,
    platform_dark_overrides, resolve_semantic_color,
};

#[test]
fn ios_base_theme_resolves_all_semantic_roles_via_public_api() {
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS ベーステーマを構築できる");
    let resolver = TokenResolver::new(base, TokenSet::new());

    for role in SemanticColorRole::ALL {
        resolve_semantic_color(&resolver, role)
            .expect("iOS ベーステーマは全セマンティック役割を解決できる");
    }
}

#[test]
fn android_base_theme_resolves_all_semantic_roles_via_public_api() {
    let base =
        platform_base_token_set(ThemePlatform::Android).expect("Android ベーステーマを構築できる");
    let resolver = TokenResolver::new(base, TokenSet::new());

    for role in SemanticColorRole::ALL {
        resolve_semantic_color(&resolver, role)
            .expect("Android ベーステーマは全セマンティック役割を解決できる");
    }
}

#[test]
fn ios_dark_theme_changes_primary_color_via_public_api() {
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS ベーステーマを構築できる");
    let light_resolver = TokenResolver::new(base.clone(), TokenSet::new());
    let light_primary = resolve_semantic_color(&light_resolver, SemanticColorRole::Primary)
        .expect("ライトの primary は解決できる");

    let dark_overrides =
        platform_dark_overrides(ThemePlatform::Ios).expect("iOS ダーク差分を構築できる");
    let dark_resolver = TokenResolver::new(base, dark_overrides);
    let dark_primary = resolve_semantic_color(&dark_resolver, SemanticColorRole::Primary)
        .expect("ダークの primary は解決できる");

    assert_ne!(
        light_primary, dark_primary,
        "iOS の primary はライト/ダークで異なる具体値へ解決される必要がある"
    );
}

#[test]
fn android_dark_theme_changes_primary_color_via_public_api() {
    let base =
        platform_base_token_set(ThemePlatform::Android).expect("Android ベーステーマを構築できる");
    let light_resolver = TokenResolver::new(base.clone(), TokenSet::new());
    let light_primary = resolve_semantic_color(&light_resolver, SemanticColorRole::Primary)
        .expect("ライトの primary は解決できる");

    let dark_overrides =
        platform_dark_overrides(ThemePlatform::Android).expect("Android ダーク差分を構築できる");
    let dark_resolver = TokenResolver::new(base, dark_overrides);
    let dark_primary = resolve_semantic_color(&dark_resolver, SemanticColorRole::Primary)
        .expect("ダークの primary は解決できる");

    assert_ne!(
        light_primary, dark_primary,
        "Android の primary はライト/ダークで異なる具体値へ解決される必要がある"
    );
}

#[test]
fn ios_dark_theme_falls_back_to_base_for_keys_absent_from_dark_diff_via_public_api() {
    // secondary（ios-gray）はライト/ダークで値が変わらないため、ダーク差分に
    // 意図的に含めていない（theme-switching-architecture.md §2.3 のフォールバック
    // 意味論）。公開 API 経由でも同じ挙動になることを確認する。
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS ベーステーマを構築できる");
    let light_resolver = TokenResolver::new(base.clone(), TokenSet::new());
    let light_secondary = resolve_semantic_color(&light_resolver, SemanticColorRole::Secondary)
        .expect("ライトの secondary は解決できる");

    let dark_overrides =
        platform_dark_overrides(ThemePlatform::Ios).expect("iOS ダーク差分を構築できる");
    let dark_resolver = TokenResolver::new(base, dark_overrides);
    let dark_secondary = resolve_semantic_color(&dark_resolver, SemanticColorRole::Secondary)
        .expect("ダークの secondary はベース値へフォールバックして解決できる");

    assert_eq!(
        light_secondary, dark_secondary,
        "ダーク差分に含まれない secondary はベース値のまま解決される必要がある"
    );
}
