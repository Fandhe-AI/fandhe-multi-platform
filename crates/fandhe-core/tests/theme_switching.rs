//! テーマ切替・適用のユニットテスト（TASK-8.4-S22、#178）。
//!
//! `docs/architecture/theme-switching-architecture.md`（S4、#160）§6 が定める
//! 「S22（#178）が対象とすべきテスト観点」6 項目を固定化する結合テストである。
//! `fandhe_core::token::*` の公開 API のみを経由し（`token_resolver.rs` の慣例に
//! 従う）、`token::resolver` の内部実装には依存しない。
//!
//! ## テスト構成
//!
//! 観点 1・2・3・6 は低レベル API（`TokenResolver::set_theme_overrides` +
//! `resolve_all`）で検証する（下記 `light_to_dark...` 等、後方互換の回帰確認を
//! 兼ねる）。観点 4（`theme_generation` の冪等性）・観点 5（`ThemeVariant` 伝搬）は
//! `TokenResolver::with_themes` / `set_active_theme` / `theme_generation` /
//! `ThemePreference::resolved_with` 経由で検証する（Issue #161）。

use fandhe_core::token::{
    ColorTokenRef, ColorValue, SemanticColorRole, Theme, ThemePlatform, ThemePreference,
    ThemeVariant, TokenCategory, TokenDefinition, TokenPath, TokenRef, TokenResolver, TokenSet,
    TokenValue, platform_base_token_set, platform_dark_overrides, platform_theme,
    resolve_semantic_color,
};

fn path(dotted: &str) -> TokenPath {
    TokenPath::from_dotted(dotted).expect("テスト用の正当なパス")
}

fn value_def(value: TokenValue) -> TokenDefinition {
    TokenDefinition::Value { value }
}

fn reference_def(category: TokenCategory, target: &str) -> TokenDefinition {
    TokenDefinition::Reference {
        category,
        reference: TokenRef::new(path(target)),
    }
}

// --- 設計文書 §6 観点 1: ダーク差分で上書きされたキーが、切替後に新値を返す ---

#[test]
fn light_to_dark_switch_changes_overridden_value() {
    let mut base = TokenSet::new();
    base.insert(
        path("button.background"),
        value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
    );

    let mut resolver = TokenResolver::new(base, TokenSet::new());
    resolver
        .resolve_all()
        .expect("ライトテーマは一括解決できる");
    let light = resolver
        .resolve(&TokenRef::new(path("button.background")))
        .expect("ライトテーマの値を解決できる");
    assert_eq!(
        light.value(),
        &TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))
    );

    let mut dark_overrides = TokenSet::new();
    dark_overrides.insert(
        path("button.background"),
        value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
    );
    resolver.set_theme_overrides(dark_overrides);
    resolver
        .resolve_all()
        .expect("ダークテーマへの切替後も一括解決できる");

    let dark = resolver
        .resolve(&TokenRef::new(path("button.background")))
        .expect("切替後は新テーマの値を解決できる");
    assert_eq!(
        dark.value(),
        &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
    );
}

// --- 設計文書 §6 観点 2: 差分未定義キーは切替後もベース値（フォールバック意味論） ---

#[test]
fn undefined_key_in_theme_falls_back_to_base() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.secondary"),
        value_def(TokenValue::Color(ColorValue::rgb(0x8e, 0x8e, 0x93))),
    );

    let mut resolver = TokenResolver::new(base, TokenSet::new());
    resolver
        .resolve_all()
        .expect("ライトテーマは一括解決できる");

    // ダーク差分には color.secondary を意図的に含めない
    // （§2.3「ダークテーマがベースの全キーを再定義することを強制しない」）。
    let dark_overrides = TokenSet::new();
    resolver.set_theme_overrides(dark_overrides);
    resolver
        .resolve_all()
        .expect("ダークテーマも一括解決できる");

    let resolved = resolver
        .resolve(&TokenRef::new(path("color.secondary")))
        .expect("差分未定義キーはベース値へフォールバックする");
    assert_eq!(
        resolved.value(),
        &TokenValue::Color(ColorValue::rgb(0x8e, 0x8e, 0x93))
    );
}

// --- 設計文書 §6 観点 3: 切替前にキャッシュ済みの結果が切替後に再計算される
//     （エイリアス参照先自体がライト/ダークで異なるケースを含む） ---

#[test]
fn switch_invalidates_cache_including_alias_retarget() {
    let mut base = TokenSet::new();
    base.insert(
        path("button.background"),
        reference_def(TokenCategory::Color, "color.primary"),
    );
    base.insert(
        path("color.primary"),
        value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
    );

    let mut resolver = TokenResolver::new(base, TokenSet::new());
    resolver
        .resolve_all()
        .expect("ライトテーマは一括解決できる");
    // 切替前にキャッシュへ充填しておく。
    let cached_light = resolver
        .resolve(&TokenRef::new(path("button.background")))
        .expect("切替前のキャッシュ値を解決できる");
    assert_eq!(
        cached_light.value(),
        &TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))
    );

    // ダーク差分ではエイリアス参照先自体が異なる（color.primary.dark）。
    let mut dark_overrides = TokenSet::new();
    dark_overrides.insert(
        path("button.background"),
        reference_def(TokenCategory::Color, "color.primary.dark"),
    );
    dark_overrides.insert(
        path("color.primary.dark"),
        value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
    );
    resolver.set_theme_overrides(dark_overrides);
    resolver
        .resolve_all()
        .expect("エイリアス先が変わるダークテーマも一括解決できる");

    let cached_dark = resolver
        .resolve(&TokenRef::new(path("button.background")))
        .expect("切替後は再計算された新テーマの値を解決できる");
    assert_eq!(
        cached_dark.value(),
        &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
    );
}

// --- 設計文書 §6 観点 6: 未定義参照・循環参照を含むテーマ差分で
//     panic せず Err(Vec<TokenError>) を返す ---

#[test]
fn switch_with_broken_theme_returns_err_without_panic() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
    );

    let mut resolver = TokenResolver::new(base, TokenSet::new());
    resolver
        .resolve_all()
        .expect("ライトテーマは一括解決できる");

    // 未定義参照を含む壊れたテーマ差分へ切り替える。
    let mut broken_undefined = TokenSet::new();
    broken_undefined.insert(
        path("color.primary"),
        reference_def(TokenCategory::Color, "color.missing"),
    );
    resolver.set_theme_overrides(broken_undefined);
    let errors = resolver
        .resolve_all()
        .expect_err("未定義参照を含むテーマは Err を返す（panic しない）");
    assert!(
        !errors.is_empty(),
        "未定義参照エラーが 1 件以上集約されている必要がある"
    );

    // 循環参照を含む壊れたテーマ差分へ切り替える。
    let mut broken_cycle = TokenSet::new();
    broken_cycle.insert(
        path("color.primary"),
        reference_def(TokenCategory::Color, "color.primary"),
    );
    resolver.set_theme_overrides(broken_cycle);
    let errors = resolver
        .resolve_all()
        .expect_err("循環参照を含むテーマは Err を返す（panic しない）");
    assert!(
        !errors.is_empty(),
        "循環参照エラーが 1 件以上集約されている必要がある"
    );

    // 個別の resolve 呼び出しも panic せず Err を返すことを確認する
    // （resolve_all だけでなく resolve 単体の契約も固定化する）。
    let err = resolver
        .resolve(&TokenRef::new(path("color.primary")))
        .expect_err("壊れたテーマでの個別 resolve も panic せず Err を返す");
    assert!(matches!(
        err,
        fandhe_core::token::TokenError::CircularReference { .. }
    ));
}

// --- 適用: S16 の platform_dark_overrides を切替経由で適用する ---

#[test]
fn platform_dark_overrides_apply_via_switch() {
    for platform in [ThemePlatform::Ios, ThemePlatform::Android] {
        let base = platform_base_token_set(platform).expect("プラットフォーム既定値は構築できる");
        let mut resolver = TokenResolver::new(base, TokenSet::new());
        resolver
            .resolve_all()
            .expect("ライトテーマは一括解決できる");

        let light_primary = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
            .expect("ライトテーマの primary は解決できる");

        let dark_overrides = platform_dark_overrides(platform).expect("ダーク差分は構築できる");
        resolver.set_theme_overrides(dark_overrides);
        resolver
            .resolve_all()
            .expect("切替後のダークテーマも一括解決できる");

        let dark_primary = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
            .expect("ダークテーマの primary は解決できる");

        assert_ne!(
            light_primary, dark_primary,
            "{platform:?}: 切替によって primary の解決値が変わる必要がある"
        );
    }
}

// --- 適用: resolve_semantic_color がライト/ダークで異なるパレット値を返す ---

#[test]
fn semantic_color_resolves_per_theme() {
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS 既定値は構築できる");
    let dark_overrides =
        platform_dark_overrides(ThemePlatform::Ios).expect("iOS ダーク差分は構築できる");

    let light_resolver = TokenResolver::new(base.clone(), TokenSet::new());
    let light = resolve_semantic_color(&light_resolver, SemanticColorRole::Background)
        .expect("ライトの background は解決できる");

    let dark_resolver = TokenResolver::new(base, dark_overrides);
    let dark = resolve_semantic_color(&dark_resolver, SemanticColorRole::Background)
        .expect("ダークの background は解決できる");

    assert_ne!(
        light, dark,
        "ライト/ダークで background の解決値は異なる必要がある"
    );
}

// --- 適用: 型付き参照（ColorTokenRef）経由でも切替後の値が観測できる ---

#[test]
fn typed_ref_sees_switched_value() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
    );
    let mut resolver = TokenResolver::new(base, TokenSet::new());
    resolver
        .resolve_all()
        .expect("ライトテーマは一括解決できる");

    let token_ref: ColorTokenRef = ColorTokenRef::from_dotted("color.primary").expect("正当なパス");
    let light = resolver
        .resolve_typed(&token_ref)
        .expect("型付き参照はライトテーマで解決できる");
    assert_eq!(light, ColorValue::rgb(0x01, 0x01, 0x01));

    let mut dark_overrides = TokenSet::new();
    dark_overrides.insert(
        path("color.primary"),
        value_def(TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))),
    );
    resolver.set_theme_overrides(dark_overrides);
    resolver
        .resolve_all()
        .expect("ダークテーマも一括解決できる");

    let dark = resolver
        .resolve_typed(&token_ref)
        .expect("型付き参照は切替後もダークテーマの値を解決できる");
    assert_eq!(dark, ColorValue::rgb(0x02, 0x02, 0x02));
}

// --- 設計文書 §6 観点 4: 同一 ThemeVariant への再切替は冪等であり、
//     theme_generation が進まないこと（Issue #161） ---

#[test]
fn same_variant_re_switch_is_idempotent_and_keeps_generation() {
    for platform in [ThemePlatform::Ios, ThemePlatform::Android] {
        let base = platform_base_token_set(platform).expect("プラットフォーム既定値は構築できる");
        let light = platform_theme(platform, ThemeVariant::Light).expect("ライト構築は成功する");
        let dark = platform_theme(platform, ThemeVariant::Dark).expect("ダーク構築は成功する");

        let mut resolver = TokenResolver::with_themes(base, light, dark, ThemeVariant::Light);
        assert_eq!(resolver.theme_generation(), 0);

        resolver
            .set_active_theme(ThemeVariant::Light)
            .expect("同一テーマへの再切替は no-op として成功する");
        assert_eq!(
            resolver.theme_generation(),
            0,
            "{platform:?}: 同一テーマへの再切替は theme_generation を進めない"
        );
        assert_eq!(resolver.active_theme(), ThemeVariant::Light);
    }
}

#[test]
fn light_dark_light_round_trip_advances_generation_by_two() {
    // Light -> Dark -> Light の往復で世代が 2 進むことを確認する
    // （実装計画 §3.5 の明記事項）。
    let base = platform_base_token_set(ThemePlatform::Ios).expect("ベース構築は成功する");
    let light =
        platform_theme(ThemePlatform::Ios, ThemeVariant::Light).expect("ライト構築は成功する");
    let dark =
        platform_theme(ThemePlatform::Ios, ThemeVariant::Dark).expect("ダーク構築は成功する");

    let mut resolver = TokenResolver::with_themes(base, light, dark, ThemeVariant::Light);
    resolver
        .set_active_theme(ThemeVariant::Dark)
        .expect("Dark への切替は成功する");
    resolver
        .set_active_theme(ThemeVariant::Light)
        .expect("Light への切替も成功する");

    assert_eq!(resolver.theme_generation(), 2);
    assert_eq!(resolver.active_theme(), ThemeVariant::Light);
}

// --- 設計文書 §6 観点 5: ThemePreference::System が解決した ThemeVariant が
//     set_active_theme 経由で正しく反映されること（Issue #161） ---

#[test]
fn theme_preference_system_resolved_variant_is_reflected_via_set_active_theme() {
    let base = platform_base_token_set(ThemePlatform::Android).expect("ベース構築は成功する");
    let light =
        platform_theme(ThemePlatform::Android, ThemeVariant::Light).expect("ライト構築は成功する");
    let dark =
        platform_theme(ThemePlatform::Android, ThemeVariant::Dark).expect("ダーク構築は成功する");

    let mut resolver = TokenResolver::with_themes(base, light, dark, ThemeVariant::Light);

    // プラットフォームレイヤが OS 判定した現在値（ここでは Dark）を
    // ThemePreference::System 経由で解決し、set_active_theme へ渡すシナリオ。
    let os_reported_variant = ThemeVariant::Dark;
    let resolved = ThemePreference::System.resolved_with(os_reported_variant);
    resolver
        .set_active_theme(resolved)
        .expect("解決済み ThemeVariant による切替は成功する");

    assert_eq!(resolver.active_theme(), ThemeVariant::Dark);
    let primary = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
        .expect("切替後の primary は解決できる");
    let expected_dark_light_resolver = {
        let base = platform_base_token_set(ThemePlatform::Android).expect("比較用ベース構築");
        let dark_overrides =
            platform_dark_overrides(ThemePlatform::Android).expect("比較用ダーク差分構築");
        TokenResolver::new(base, dark_overrides)
    };
    let expected_primary =
        resolve_semantic_color(&expected_dark_light_resolver, SemanticColorRole::Primary)
            .expect("比較対象の primary は解決できる");
    assert_eq!(
        primary, expected_primary,
        "set_active_theme 経由の切替は低レベル API 直接構築と同じ解決値になる"
    );
}

// --- 壊れたテーマへの set_active_theme 経由の切替も panic せず Err を返すこと ---

#[test]
fn set_active_theme_with_broken_dark_theme_returns_err_without_panic() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        value_def(TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01))),
    );

    let mut broken_dark_overrides = TokenSet::new();
    broken_dark_overrides.insert(
        path("color.primary"),
        reference_def(TokenCategory::Color, "color.missing"),
    );

    let mut resolver = TokenResolver::with_themes(
        base,
        Theme::new(ThemeVariant::Light, TokenSet::new()),
        Theme::new(ThemeVariant::Dark, broken_dark_overrides),
        ThemeVariant::Light,
    );

    let errors = resolver
        .set_active_theme(ThemeVariant::Dark)
        .expect_err("壊れたダークテーマへの切替は panic せず Err を返す");
    assert!(!errors.is_empty());
    // テーマ自体はすでに切り替わっている契約（`set_active_theme` doc 参照）。
    assert_eq!(resolver.active_theme(), ThemeVariant::Dark);
    assert_eq!(resolver.theme_generation(), 1);
}
