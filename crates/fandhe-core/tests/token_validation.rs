//! `token::validate_token_sets` / `TokenResolver::validated` の公開 API 経由の
//! 結合テスト（TASK-8.4-S18、#174）。
//!
//! `docs/architecture/token-resolution-architecture.md` §5.3 が定める
//! 「S17 → S18 → 解決エンジン」データフロー契約のうち、S18（本 issue）が
//! 提供するゲートウェイを公開 API から確認する。網羅的なユニットテスト拡充は
//! S21/S22（#177/#178）のスコープであり、本ファイルは既定トークンセット
//! （`platform_base_token_set` / `platform_dark_overrides`）が検証を通過する
//! ことと、壊れたセットで集約レポートが返ることの結合テストに留める。

use fandhe_core::{
    ColorValue, ThemePlatform, TokenCategory, TokenDefinition, TokenError, TokenPath, TokenRef,
    TokenResolver, TokenSet, TokenValue, platform_base_token_set, platform_dark_overrides,
    validate_token_sets,
};

fn path(dotted: &str) -> TokenPath {
    TokenPath::from_dotted(dotted).expect("テスト用の正当なパス")
}

#[test]
fn platform_default_token_sets_pass_validation() {
    // S16（#172）が提供する既定トークンセット（プラットフォーム準拠の具体値）が
    // S18 の検証ゲートを通過することを確認する（§5.3 契約の実例）。
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS 既定ベースは構築できる");
    let overrides =
        platform_dark_overrides(ThemePlatform::Ios).expect("iOS ダークテーマ差分は構築できる");

    assert!(validate_token_sets(&base, &overrides).is_ok());
}

#[test]
fn platform_default_token_sets_construct_a_validated_resolver() {
    let base =
        platform_base_token_set(ThemePlatform::Android).expect("Android 既定ベースは構築できる");
    let overrides = platform_dark_overrides(ThemePlatform::Android)
        .expect("Android ダークテーマ差分は構築できる");

    let mut resolver = TokenResolver::validated(base, overrides)
        .expect("検証を通過した既定セットは validated 経由で構築できる");

    // 既定セットの全キーが実際に解決可能であることも合わせて確認する
    // （§5.3 契約: 検証を通過したセットは解決エンジンで問題なく解決できる）。
    assert!(
        resolver.resolve_all().is_ok(),
        "検証を通過した既定セットは resolve_all も成功する"
    );
}

#[test]
fn broken_token_set_returns_aggregated_report_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("color.primary"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("color.missing")),
        },
    );

    let report = validate_token_sets(&base, &TokenSet::new())
        .expect_err("未定義参照を含むセットはレポートを返す");
    assert_eq!(report.len(), 1);
    assert!(matches!(
        report.issues()[0].error(),
        TokenError::UndefinedReference { .. }
    ));
}

#[test]
fn validated_rejects_broken_set_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(path("a"), {
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("a")),
        }
    });

    let report = TokenResolver::validated(base, TokenSet::new())
        .expect_err("自己参照を含むセットは validated が構築を拒否する");
    assert_eq!(report.len(), 1);
}

#[test]
fn base_side_breakage_masked_by_a_valid_override_is_still_rejected() {
    // レビュー指摘の repro（TASK-8.4-S18 review、#174）: ベース側で x が未定義
    // 参照（missing）を指しているが、同一キー x をテーマ差分が有効な値で
    // 上書きしている場合、テーマ差分優先ルックアップだけでは検証をすり抜けて
    // しまう。ベース単独でも解決できることまで検証することで、この組み合わせ
    // 自体を validated 構築時点で拒否できることを確認する。
    let mut base = TokenSet::new();
    base.insert(
        path("x"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("missing")),
        },
    );

    let mut overrides = TokenSet::new();
    overrides.insert(
        path("x"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x20, 0x20, 0x20)),
        },
    );

    let report = validate_token_sets(&base, &overrides)
        .expect_err("override に隠れたベース側の未定義参照も検証で検出される");
    assert!(
        report
            .issues()
            .iter()
            .any(|issue| matches!(issue.error(), TokenError::UndefinedReference { .. }))
    );

    TokenResolver::validated(base, overrides)
        .expect_err("同じ組み合わせは validated 経由でも構築を拒否される");
}

#[test]
fn set_validated_theme_overrides_revalidates_before_switching() {
    // set_theme_overrides は再検証しないが、set_validated_theme_overrides は
    // 新しい組み合わせを再検証してから差し替える契約を公開 API 経由で確認する。
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

    let mut resolver = TokenResolver::validated(base, TokenSet::new())
        .expect("検証を通過したセットは validated 経由で構築できる");

    let mut broken_overrides = TokenSet::new();
    broken_overrides.insert(
        path("button.background"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("color.missing")),
        },
    );

    resolver
        .set_validated_theme_overrides(broken_overrides)
        .expect_err("未定義参照を持ち込むテーマ差分への切替は拒否される");

    // 正当なテーマ差分への切替は成功し、以後は新テーマの値で解決できる。
    let mut valid_overrides = TokenSet::new();
    valid_overrides.insert(
        path("color.primary"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x02, 0x02, 0x02)),
        },
    );
    resolver
        .set_validated_theme_overrides(valid_overrides)
        .expect("正当なテーマ差分への切替は許可される");
    resolver.resolve_all().expect("切替後も一括解決できる");
}

#[test]
fn platform_default_dark_theme_switch_stays_validated_after_switching_back_to_light() {
    // Issue #177 / #178 の一般契約テストを補う実例として、既定の
    // プラットフォームセットで「ダーク → ライト（テーマ差分を空に戻す）」という
    // 正当なテーマ切替を経ても resolve が壊れないことを set_validated_theme_overrides
    // 経由で確認する（platform_theme のセットが本 PR の base 単独検証を通過する
    // ことの実例確認も兼ねる）。
    let base = platform_base_token_set(ThemePlatform::Ios).expect("iOS 既定ベースは構築できる");
    let dark_overrides =
        platform_dark_overrides(ThemePlatform::Ios).expect("iOS ダークテーマ差分は構築できる");

    let mut resolver = TokenResolver::validated(base, dark_overrides)
        .expect("既定セットはダークテーマ込みで validated 経由で構築できる");
    resolver
        .resolve_all()
        .expect("ダークテーマは一括解決できる");

    resolver
        .set_validated_theme_overrides(TokenSet::new())
        .expect("ライトテーマ（差分無し）への切替も再検証を通過する");
    resolver
        .resolve_all()
        .expect("ライトテーマへ切り替えた後も一括解決できる");
}

#[test]
fn theme_override_category_mismatch_is_reported_via_public_api() {
    let mut base = TokenSet::new();
    base.insert(
        path("shared.token"),
        TokenDefinition::Value {
            value: TokenValue::Color(ColorValue::rgb(0x10, 0x10, 0x10)),
        },
    );

    let mut overrides = TokenSet::new();
    overrides.insert(
        path("shared.token"),
        TokenDefinition::Value {
            value: TokenValue::Dimension(4.0),
        },
    );

    let report = validate_token_sets(&base, &overrides)
        .expect_err("テーマ差分のカテゴリ不一致はレポートに含まれる");
    assert!(report.issues().iter().any(|issue| matches!(
        issue.error(),
        TokenError::ThemeOverrideCategoryMismatch { .. }
    )));
}

#[test]
fn base_undefined_reference_is_reported_even_when_hidden_by_category_mismatch() {
    // レビュー指摘の再現ケース（#174, TASK-8.4-S18）: ベースの `x` が未定義
    // 参照（`missing` を指す `Reference`）で壊れており、同一キー `x` を
    // テーマ差分が異なるカテゴリの `Value` で上書きしている。この組み合わせ
    // では 1. のテーマ差分優先ルックアップは上書き後の値が解決に使われるため
    // `ThemeOverrideCategoryMismatch` のみを検出し、`UndefinedReference` を
    // 検出しない。`flagged_origins` を `resolve_chain_in` 由来の問題に限定した
    // ことで、3. のベース単独チェックが `ThemeOverrideCategoryMismatch` のみ
    // 検出済みの起点をスキップせず、ベース側の破損も報告されることを確認する。
    let mut base = TokenSet::new();
    base.insert(
        path("x"),
        TokenDefinition::Reference {
            category: TokenCategory::Color,
            reference: TokenRef::new(path("missing")),
        },
    );

    let mut overrides = TokenSet::new();
    overrides.insert(
        path("x"),
        TokenDefinition::Value {
            value: TokenValue::Dimension(4.0),
        },
    );

    let report = validate_token_sets(&base, &overrides)
        .expect_err("カテゴリ不一致とベース未定義参照の両方が検証で検出される");

    assert!(
        report.issues().iter().any(|issue| matches!(
            issue.error(),
            TokenError::ThemeOverrideCategoryMismatch { .. }
        )),
        "テーマ差分のカテゴリ不一致が報告されること"
    );
    assert!(
        report
            .issues()
            .iter()
            .any(|issue| matches!(issue.error(), TokenError::UndefinedReference { .. })),
        "ベース単独では解決できない未定義参照も報告されること（見落とし修正の回帰確認）"
    );
}
