//! `token` モジュールの公開 API 経由の結合テスト（TASK-8.4-S3、#159）。
//!
//! `docs/architecture/token-resolution-architecture.md` §6「S21 が対象とすべき
//! テスト観点（最小限）」から、本 issue のスコープで固定化すべき最小ケースを
//! 引き継ぐ。網羅的なユニットテストは `resolver.rs` / `schema.rs` / `error.rs` の
//! inline テストが担う（Issue #177）。本ファイルは公開 API
//! （`fandhe_core::token::*` の再エクスポート）が意図どおり機能することの
//! 結合テストに留める。

use fandhe_core::{
    AnimationValue, BorderStyleValue, BorderValue, ColorValue, DurationValue, EasingValue,
    FontStyleValue, FontWeightValue, ShadowValue, SpacingValue, TokenCategory, TokenDefinition,
    TokenPath, TokenRef, TokenResolver, TokenSet, TokenValue, TypographyValue, border_token_set,
    palette_token_set, resolve_border_color, resolve_shadow_color, shadow_token_set,
};

fn path(dotted: &str) -> TokenPath {
    TokenPath::from_dotted(dotted).expect("テスト用の正当なパス")
}

#[test]
fn resolves_direct_value_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x11, 0x22, 0x33)),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("color.primary")))
        .expect("直接値は公開 API から解決できる");

    assert_eq!(
        resolved.value(),
        &TokenValue::Color(ColorValue::rgb(0x11, 0x22, 0x33))
    );
}

#[test]
fn resolves_multi_step_alias_chain_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("button.background"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("color.brand")),
        },
    );
    base.insert(
        path("color.brand"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("color.primary")),
        },
    );
    base.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0xab, 0xcd, 0xef)),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("button.background")))
        .expect("2〜3 段のエイリアスチェーンは公開 API から解決できる");

    assert_eq!(
        resolved.value(),
        &TokenValue::Color(ColorValue::rgb(0xab, 0xcd, 0xef))
    );
}

#[test]
fn theme_override_takes_precedence_and_changes_alias_target() {
    let mut base = TokenSet::new();
    base.insert(
        path("button.background"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("color.primary")),
        },
    );
    base.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01)),
        },
    );

    let mut dark = TokenSet::new();
    dark.insert(
        path("button.background"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("color.primary.dark")),
        },
    );
    dark.insert(
        path("color.primary.dark"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02)),
        },
    );

    let resolver = TokenResolver::new(base, dark);
    let resolved = resolver
        .resolve(&TokenRef::new(path("button.background")))
        .expect("テーマ差分によるエイリアス先切替が解決できる（設計文書 §3.3 の順序契約）");

    assert_eq!(
        resolved.value(),
        &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
    );
}

#[test]
fn circular_reference_does_not_panic() {
    let mut base = TokenSet::new();
    base.insert(
        path("a"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("b")),
        },
    );
    base.insert(
        path("b"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("a")),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let error = resolver
        .resolve(&TokenRef::new(path("a")))
        .expect_err("間接循環はエラーとして返る");

    assert!(matches!(
        error,
        fandhe_core::TokenError::CircularReference { .. }
    ));
}

#[test]
fn undefined_reference_via_public_api() {
    let resolver = TokenResolver::new(TokenSet::new(), TokenSet::new());
    let error = resolver
        .resolve(&TokenRef::new(path("no.such.token")))
        .expect_err("未定義参照はエラーとして返る");
    assert!(matches!(
        error,
        fandhe_core::TokenError::UndefinedReference { .. }
    ));
}

#[test]
fn type_mismatch_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("spacing.md")),
        },
    );
    base.insert(
        path("spacing.md"),
        TokenDefinition::Value {
            value: TokenValue::Dimension(8.0),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let error = resolver
        .resolve(&TokenRef::new(path("color.primary")))
        .expect_err("色トークンがスペーシングトークンを参照すると型不一致になる");

    assert!(matches!(
        error,
        fandhe_core::TokenError::TypeMismatch { .. }
    ));
}

#[test]
fn max_depth_exceeded_terminates_without_hanging() {
    let mut base = TokenSet::new();
    let chain_len = fandhe_core::MAX_ALIAS_DEPTH as usize + 8;
    for i in 0..chain_len {
        base.insert(
            path(&format!("t{i}")),
            TokenDefinition::Reference {
                category: TokenCategory::Color,
                reference: TokenRef::new(path(&format!("t{}", i + 1))),
            },
        );
    }
    base.insert(
        path(&format!("t{chain_len}")),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x03, 0x03, 0x03)),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let error = resolver
        .resolve(&TokenRef::new(path("t0")))
        .expect_err("上限を超えるチェーンは MaxDepthExceeded で打ち切られる");

    assert!(matches!(
        error,
        fandhe_core::TokenError::MaxDepthExceeded { .. }
    ));
}

#[test]
fn resolves_direct_typography_value_via_public_api() {
    let typography = TypographyValue::new(
        "sans-serif",
        16.0,
        24.0,
        FontWeightValue::REGULAR,
        FontStyleValue::Normal,
    )
    .expect("テスト用の正当なタイポグラフィ値");

    let mut base = TokenSet::new();
    base.insert(
        path("typography.body"),
        TokenDefinition::Value {
            value: TokenValue::Typography(typography.clone()),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("typography.body")))
        .expect("直接値は公開 API から解決できる（TASK-8.4-S9、#165）");

    assert_eq!(resolved.value(), &TokenValue::Typography(typography));
}

#[test]
fn resolves_typography_alias_via_public_api() {
    let typography = TypographyValue::new(
        "sans-serif",
        20.0,
        28.0,
        FontWeightValue::BOLD,
        FontStyleValue::Italic,
    )
    .expect("テスト用の正当なタイポグラフィ値");

    let mut base = TokenSet::new();
    base.insert(
        path("typography.heading"),
        TokenDefinition::Reference {
            category: TokenCategory::Typography,
            reference: TokenRef::new(path("typography.base")),
        },
    );
    base.insert(
        path("typography.base"),
        TokenDefinition::Value {
            value: TokenValue::Typography(typography.clone()),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("typography.heading")))
        .expect("エイリアス経由のタイポグラフィ値は公開 API から解決できる");

    assert_eq!(resolved.value(), &TokenValue::Typography(typography));
}

#[test]
fn color_referencing_typography_is_type_mismatch() {
    let typography = TypographyValue::new(
        "sans-serif",
        16.0,
        24.0,
        FontWeightValue::REGULAR,
        FontStyleValue::Normal,
    )
    .expect("テスト用の正当なタイポグラフィ値");

    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("typography.body")),
        },
    );
    base.insert(
        path("typography.body"),
        TokenDefinition::Value {
            value: TokenValue::Typography(typography),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let error = resolver
        .resolve(&TokenRef::new(path("color.primary")))
        .expect_err("色トークンがタイポグラフィトークンを参照すると型不一致になる");

    assert!(matches!(
        error,
        fandhe_core::TokenError::TypeMismatch { .. }
    ));
}

#[test]
fn resolves_border_token_via_public_api() {
    // TASK-8.4-S12（#168）: border カテゴリのトークンも公開 API から解決できること。
    let border = BorderValue::new(
        1.0,
        BorderStyleValue::Solid,
        TokenRef::new(path("color.primary")),
    )
    .expect("テスト用の正当なボーダー値");

    let mut base = TokenSet::new();
    base.insert(
        path("border.default"),
        TokenDefinition::Value {
            value: TokenValue::Border(border.clone()),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("border.default")))
        .expect("直接値は公開 API から解決できる");

    assert_eq!(resolved.value(), &TokenValue::Border(border));
}

#[test]
fn resolves_border_color_reference_via_public_api() {
    // border 値が保持する色参照が、パレットの色まで解決できることの確認
    // （設計文書 §4.4「色を要求するフィールドは color カテゴリのみ参照可」）。
    let base = palette_token_set().expect("パレット構築は成功する");
    let resolver = TokenResolver::new(base, TokenSet::new());

    let border = BorderValue::new(
        1.0,
        BorderStyleValue::Solid,
        TokenRef::new(path("color.palette.gray-300")),
    )
    .expect("テスト用の正当なボーダー値");

    let resolved = resolve_border_color(&resolver, &border)
        .expect("ボーダーの色参照はパレットの色へ解決できる");
    assert_eq!(resolved.category(), TokenCategory::Color);
}

#[test]
fn border_color_reference_to_non_color_category_is_type_mismatch() {
    let mut base = TokenSet::new();
    base.insert(
        path("spacing.md"),
        TokenDefinition::Value {
            value: TokenValue::Dimension(8.0),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let border = BorderValue::new(
        1.0,
        BorderStyleValue::Solid,
        TokenRef::new(path("spacing.md")),
    )
    .expect("テスト用の正当なボーダー値");

    let error = resolve_border_color(&resolver, &border)
        .expect_err("色以外のカテゴリを参照した場合は型不一致になる");
    assert!(matches!(
        error,
        fandhe_core::TokenError::TypeMismatch { .. }
    ));
}

#[test]
fn resolves_shadow_token_via_public_api() {
    // TASK-8.4-S12（#168）: shadow カテゴリのトークンも公開 API から解決できること。
    let shadow = ShadowValue::new(0.0, 4.0, 8.0, TokenRef::new(path("color.primary")))
        .expect("テスト用の正当なシャドウ値");

    let mut base = TokenSet::new();
    base.insert(
        path("shadow.md"),
        TokenDefinition::Value {
            value: TokenValue::Shadow(shadow.clone()),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("shadow.md")))
        .expect("直接値は公開 API から解決できる");

    assert_eq!(resolved.value(), &TokenValue::Shadow(shadow));
}

#[test]
fn resolves_shadow_color_reference_via_public_api() {
    let base = palette_token_set().expect("パレット構築は成功する");
    let resolver = TokenResolver::new(base, TokenSet::new());

    let shadow = ShadowValue::new(0.0, 4.0, 8.0, TokenRef::new(path("color.palette.black")))
        .expect("テスト用の正当なシャドウ値");

    let resolved = resolve_shadow_color(&resolver, &shadow)
        .expect("シャドウの色参照はパレットの色へ解決できる");
    assert_eq!(resolved.category(), TokenCategory::Color);
}

#[test]
fn shadow_color_reference_to_non_color_category_is_type_mismatch() {
    let mut base = TokenSet::new();
    base.insert(
        path("spacing.md"),
        TokenDefinition::Value {
            value: TokenValue::Dimension(8.0),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let shadow = ShadowValue::new(0.0, 0.0, 1.0, TokenRef::new(path("spacing.md")))
        .expect("テスト用の正当なシャドウ値");

    let error = resolve_shadow_color(&resolver, &shadow)
        .expect_err("色以外のカテゴリを参照した場合は型不一致になる");
    assert!(matches!(
        error,
        fandhe_core::TokenError::TypeMismatch { .. }
    ));
}

#[test]
fn theme_overrides_can_replace_border_definition() {
    // テーマ差分で border/shadow 定義を上書きできることの確認（§6 検証方法）。
    let mut base = TokenSet::new();
    let light_border = BorderValue::new(
        1.0,
        BorderStyleValue::Solid,
        TokenRef::new(path("color.primary")),
    )
    .expect("正当な値");
    base.insert(
        path("border.default"),
        TokenDefinition::Value {
            value: TokenValue::Border(light_border),
        },
    );

    let mut dark_overrides = TokenSet::new();
    let dark_border = BorderValue::new(
        2.0,
        BorderStyleValue::Dashed,
        TokenRef::new(path("color.primary")),
    )
    .expect("正当な値");
    dark_overrides.insert(
        path("border.default"),
        TokenDefinition::Value {
            value: TokenValue::Border(dark_border.clone()),
        },
    );

    let resolver = TokenResolver::new(base, dark_overrides);
    let resolved = resolver
        .resolve(&TokenRef::new(path("border.default")))
        .expect("テーマ差分側の定義が優先して解決される");
    assert_eq!(resolved.value(), &TokenValue::Border(dark_border));
}

#[test]
fn resolves_direct_spacing_value_via_public_api() {
    let spacing = SpacingValue::new(16.0).expect("テスト用の正当なスペーシング値");

    let mut base = TokenSet::new();
    base.insert(
        path("spacing.md"),
        TokenDefinition::Value {
            value: TokenValue::Spacing(spacing),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("spacing.md")))
        .expect("直接値は公開 API から解決できる（TASK-8.4-S10、#166）");

    assert_eq!(resolved.value(), &TokenValue::Spacing(spacing));
}

#[test]
fn resolves_spacing_alias_via_public_api() {
    let spacing = SpacingValue::new(16.0).expect("テスト用の正当なスペーシング値");

    let mut base = TokenSet::new();
    base.insert(
        path("spacing.gutter"),
        TokenDefinition::Reference {
            category: TokenCategory::Spacing,
            reference: TokenRef::new(path("spacing.md")),
        },
    );
    base.insert(
        path("spacing.md"),
        TokenDefinition::Value {
            value: TokenValue::Spacing(spacing),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("spacing.gutter")))
        .expect("エイリアス経由のスペーシング値は公開 API から解決できる");

    assert_eq!(resolved.value(), &TokenValue::Spacing(spacing));
}

#[test]
fn spacing_referencing_color_is_type_mismatch() {
    let mut base = TokenSet::new();
    base.insert(
        path("spacing.md"),
        TokenDefinition::Reference {
            category: TokenCategory::Spacing,
            reference: TokenRef::new(path("color.primary")),
        },
    );
    base.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x11, 0x22, 0x33)),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let error = resolver
        .resolve(&TokenRef::new(path("spacing.md")))
        .expect_err("スペーシングトークンが色トークンを参照すると型不一致になる");

    assert!(matches!(
        error,
        fandhe_core::TokenError::TypeMismatch { .. }
    ));
}

#[test]
fn resolves_direct_duration_value_via_public_api() {
    let duration = DurationValue::new(250.0).expect("テスト用の正当な duration 値");

    let mut base = TokenSet::new();
    base.insert(
        path("duration.normal"),
        TokenDefinition::Value {
            value: TokenValue::Duration(duration),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("duration.normal")))
        .expect("直接値は公開 API から解決できる（TASK-8.4-S13、#169）");

    assert_eq!(resolved.value(), &TokenValue::Duration(duration));
}

#[test]
fn resolves_duration_alias_via_public_api() {
    let duration = DurationValue::new(100.0).expect("テスト用の正当な duration 値");

    let mut base = TokenSet::new();
    base.insert(
        path("duration.tooltip"),
        TokenDefinition::Reference {
            category: TokenCategory::Duration,
            reference: TokenRef::new(path("duration.fast")),
        },
    );
    base.insert(
        path("duration.fast"),
        TokenDefinition::Value {
            value: TokenValue::Duration(duration),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("duration.tooltip")))
        .expect("エイリアス経由の duration 値は公開 API から解決できる");

    assert_eq!(resolved.value(), &TokenValue::Duration(duration));
}

#[test]
fn duration_referencing_color_is_type_mismatch() {
    let mut base = TokenSet::new();
    base.insert(
        path("duration.normal"),
        TokenDefinition::Reference {
            category: TokenCategory::Duration,
            reference: TokenRef::new(path("color.primary")),
        },
    );
    base.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x11, 0x22, 0x33)),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let error = resolver
        .resolve(&TokenRef::new(path("duration.normal")))
        .expect_err("duration トークンが色トークンを参照すると型不一致になる");

    assert!(matches!(
        error,
        fandhe_core::TokenError::TypeMismatch { .. }
    ));
}

#[test]
fn resolves_direct_animation_value_via_public_api() {
    let duration = DurationValue::new(250.0).expect("テスト用の正当な duration 値");
    let animation = AnimationValue::new(duration, EasingValue::EaseInOut);

    let mut base = TokenSet::new();
    base.insert(
        path("animation.standard"),
        TokenDefinition::Value {
            value: TokenValue::Animation(animation),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("animation.standard")))
        .expect("直接値は公開 API から解決できる（TASK-8.4-S13、#169）");

    assert_eq!(resolved.value(), &TokenValue::Animation(animation));
}

#[test]
fn resolves_animation_alias_via_public_api() {
    let duration = DurationValue::new(400.0).expect("テスト用の正当な duration 値");
    let animation = AnimationValue::new(duration, EasingValue::EaseInOut);

    let mut base = TokenSet::new();
    base.insert(
        path("animation.dialog"),
        TokenDefinition::Reference {
            category: TokenCategory::Animation,
            reference: TokenRef::new(path("animation.emphasized")),
        },
    );
    base.insert(
        path("animation.emphasized"),
        TokenDefinition::Value {
            value: TokenValue::Animation(animation),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let resolved = resolver
        .resolve(&TokenRef::new(path("animation.dialog")))
        .expect("エイリアス経由のアニメーション値は公開 API から解決できる");

    assert_eq!(resolved.value(), &TokenValue::Animation(animation));
}

#[test]
fn animation_referencing_duration_is_type_mismatch() {
    let duration = DurationValue::new(250.0).expect("テスト用の正当な duration 値");

    let mut base = TokenSet::new();
    base.insert(
        path("animation.standard"),
        TokenDefinition::Reference {
            category: TokenCategory::Animation,
            reference: TokenRef::new(path("duration.normal")),
        },
    );
    base.insert(
        path("duration.normal"),
        TokenDefinition::Value {
            value: TokenValue::Duration(duration),
        },
    );
    let resolver = TokenResolver::new(base, TokenSet::new());

    let error = resolver
        .resolve(&TokenRef::new(path("animation.standard")))
        .expect_err("animation トークンが duration トークンを参照すると型不一致になる");

    assert!(matches!(
        error,
        fandhe_core::TokenError::TypeMismatch { .. }
    ));
}

#[test]
fn invalid_reference_syntax_rejected_at_construction() {
    let error =
        TokenPath::new(vec!["..".to_string()]).expect_err("`..` セグメントは構築時に拒否される");
    assert!(matches!(error, fandhe_core::TokenError::InvalidPath { .. }));

    let error = TokenPath::new(vec!["a b".to_string()])
        .expect_err("許可文字集合外の文字を含むセグメントは構築時に拒否される");
    assert!(matches!(error, fandhe_core::TokenError::InvalidPath { .. }));
}

#[test]
fn default_border_and_shadow_token_sets_resolve_via_public_api() {
    // レビュー指摘対応: border_token_set() / shadow_token_set() が返す既定
    // トークンセットをそのまま palette_token_set() と合成し、消費者が最初に
    // 触る「既定トークンセットを丸ごと使う」経路を結合テストとして固定化する
    // （手組みの BorderValue/ShadowValue のみを使う既存テストでは、
    // border.strong 等のキー名や参照先パレットキーの変更に対する回帰検出が
    // 漏れていた）。
    let mut base = palette_token_set().expect("パレット構築は成功する");
    for (key, definition) in border_token_set().expect("ボーダートークン構築は成功する")
    {
        base.insert(key, definition);
    }
    for (key, definition) in shadow_token_set().expect("シャドウトークン構築は成功する")
    {
        base.insert(key, definition);
    }
    let resolver = TokenResolver::new(base, TokenSet::new());

    for key in [
        "border.default",
        "border.strong",
        "border.dashed",
        "shadow.sm",
        "shadow.md",
        "shadow.lg",
    ] {
        let resolved = resolver
            .resolve(&TokenRef::new(path(key)))
            .unwrap_or_else(|err| panic!("既定トークン {key} の解決に失敗しました: {err}"));

        match resolved.value() {
            TokenValue::Border(border) => {
                let color = resolve_border_color(&resolver, border)
                    .unwrap_or_else(|err| panic!("{key} の色参照解決に失敗しました: {err}"));
                assert_eq!(color.category(), TokenCategory::Color);
            }
            TokenValue::Shadow(shadow) => {
                let color = resolve_shadow_color(&resolver, shadow)
                    .unwrap_or_else(|err| panic!("{key} の色参照解決に失敗しました: {err}"));
                assert_eq!(color.category(), TokenCategory::Color);
            }
            other => panic!("{key} は border/shadow カテゴリを期待しましたが {other:?} でした"),
        }
    }
}

// 以下、TASK-8.4-S21（#177）が追加する結合テスト。`resolve_all` /
// `set_theme_overrides` は resolver.rs 側の inline テストで内部契約を検証済み
// だが、公開 API（`fandhe_core::` の再エクスポート）経由でも同じ契約を満たす
// ことを検収する（再エクスポートの取りこぼしを検知するための結合テスト）。

#[test]
fn resolve_all_then_resolve_returns_value_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x0a, 0x0b, 0x0c)),
        },
    );
    let mut resolver = TokenResolver::new(base, TokenSet::new());

    resolver
        .resolve_all()
        .expect("正常なトークンセットは公開 API から一括解決できる");
    let resolved = resolver
        .resolve(&TokenRef::new(path("color.primary")))
        .expect("resolve_all 後の値は公開 API から取得できる");
    assert_eq!(
        resolved.value(),
        &TokenValue::Color(ColorValue::rgb(0x0a, 0x0b, 0x0c))
    );
}

#[test]
fn resolve_all_error_aggregation_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("ok"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x0d, 0x0e, 0x0f)),
        },
    );
    base.insert(
        path("undef"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("no.such.token")),
        },
    );
    let mut resolver = TokenResolver::new(base, TokenSet::new());

    let errors = resolver
        .resolve_all()
        .expect_err("一部キーが未定義参照のため部分的に失敗する");
    // 失敗するキーは undef の 1 件のみ（ok は成功するため含まない）。
    assert_eq!(errors.len(), 1);
    assert!(
        errors
            .iter()
            .all(|e| matches!(e, fandhe_core::TokenError::UndefinedReference { .. }))
    );

    let resolved = resolver
        .resolve(&TokenRef::new(path("ok")))
        .expect("成功キーの値は部分失敗後も公開 API から取得できる");
    assert_eq!(
        resolved.value(),
        &TokenValue::Color(ColorValue::rgb(0x0d, 0x0e, 0x0f))
    );
}

#[test]
fn set_theme_overrides_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("button.background"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("color.primary")),
        },
    );
    base.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x01, 0x01, 0x01)),
        },
    );
    let mut resolver = TokenResolver::new(base, TokenSet::new());
    resolver
        .resolve_all()
        .expect("初期テーマは公開 API から一括解決できる");

    let mut dark_overrides = TokenSet::new();
    dark_overrides.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02)),
        },
    );
    resolver.set_theme_overrides(dark_overrides);
    resolver
        .resolve_all()
        .expect("テーマ差し替え後も公開 API から一括解決できる");

    let resolved = resolver
        .resolve(&TokenRef::new(path("button.background")))
        .expect("テーマ差し替え後の値が公開 API から取得できる");
    assert_eq!(
        resolved.value(),
        &TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02))
    );
}
